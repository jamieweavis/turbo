//! Clipboard history: a monitor thread records copies into a SQLite store, and items can be
//! put back on the clipboard (and pasted) from the palette.
//!
//! - `capture`: what a copy contains and whether to record it (pure).
//! - `board`: reading from and writing to the pasteboard.
//! - `store`: the database and image files.
//!
//! The pasteboard has no change notification, so the monitor polls its change counter,
//! which is a cheap read. While history is off it doesn't poll at all. Copies marked concealed or transient by their source (password
//! managers do this) and copies made in ignored apps are never recorded.

mod board;
mod capture;
mod store;

use parking_lot::{Condvar, Mutex};
use std::path::Path;
use std::sync::{Arc, Weak};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use objc2_core_graphics::{CGEvent, CGEventField, CGEventFlags, CGEventTapLocation};

use crate::config::ClipboardConfig;
use crate::error::CoreError;
use crate::input::SYNTHETIC_EVENT_MARKER;
use crate::launcher::LauncherObserver;
use store::{Store, Stored};

#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    fn AXIsProcessTrusted() -> u8;
}

const POLL_INTERVAL: Duration = Duration::from_millis(400);
/// Expiry also runs on every copy; this catches items aging out while nothing is copied.
const PRUNE_INTERVAL: Duration = Duration::from_secs(60 * 60);
/// Lets the palette close and focus settle on the target app before ⌘V lands.
const PASTE_DELAY: Duration = Duration::from_millis(100);
/// `kVK_ANSI_V`.
const KEY_V: u16 = 0x09;

#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct ClipboardItem {
    pub id: i64,
    pub kind: ClipboardKind,
    /// The text (for text and links), or file names (for files), or a description of the
    /// image ("PNG image, 1200 × 800").
    pub text: String,
    /// A stored copy of the image, for previews (images only).
    pub image_path: Option<String>,
    /// Copied file paths (files only).
    pub file_paths: Vec<String>,
    /// The app that was frontmost when it was copied.
    pub source_app: Option<String>,
    pub source_bundle_id: Option<String>,
    /// Seconds since 1970; the most recent copy for re-copied items.
    pub copied_at: f64,
    /// Pinned items never expire and sort first.
    pub pinned: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum ClipboardKind {
    Text,
    Link,
    Image,
    Files,
}

pub(crate) struct ClipboardHistory {
    /// `None` if the database couldn't be opened; history is then unavailable but the
    /// rest of the app carries on.
    store: Option<Store>,
    config: Mutex<ClipboardConfig>,
    observer: Mutex<Option<Arc<dyn LauncherObserver>>>,
    monitor: Arc<MonitorSignal>,
}

impl ClipboardHistory {
    /// Opens (or creates) the store in `data_dir` and starts monitoring on a background
    /// thread if enabled. Returns immediately.
    pub fn new(data_dir: &Path, config: ClipboardConfig) -> Arc<Self> {
        let store = match Store::open(&data_dir.join("clipboard")) {
            Ok(store) => Some(store),
            Err(error) => {
                log::warn!("clipboard history unavailable: {error}");
                None
            }
        };
        let monitor = Arc::new(MonitorSignal::default());
        monitor.set_enabled(config.enabled);
        let history = Arc::new(Self {
            store,
            config: Mutex::new(config),
            observer: Mutex::new(None),
            monitor: Arc::clone(&monitor),
        });
        history.prune();

        let weak = Arc::downgrade(&history);
        thread::Builder::new()
            .name("turbo-clipboard".into())
            .spawn(move || run_monitor(&monitor, &weak))
            .expect("failed to spawn the clipboard thread");
        history
    }

    /// Applies limits and the enabled switch live (pruning straight away if limits shrink).
    pub fn set_config(&self, config: ClipboardConfig) {
        self.monitor.set_enabled(config.enabled);
        *self.config.lock() = config;
        if self.prune() > 0 {
            self.notify();
        }
    }

    /// Receives `clipboard_changed`.
    pub fn set_observer(&self, observer: Option<Arc<dyn LauncherObserver>>) {
        *self.observer.lock() = observer;
    }

    /// Pinned first, then newest first; every word of `query` must appear in the text or
    /// the source app's name. Fast: no pasteboard access.
    pub fn items(&self, query: &str, limit: usize) -> Vec<ClipboardItem> {
        let Some(store) = &self.store else {
            return Vec::new();
        };
        store.items(query, limit).unwrap_or_else(|error| {
            log::error!("couldn't read clipboard history: {error}");
            Vec::new()
        })
    }

    /// Puts the items back on the clipboard as one (see `Stored::merge`), in the order given,
    /// and, if `paste`, sends ⌘V to the frontmost app (needs Accessibility). The shell closes
    /// the palette first. Returns at once; the keystroke follows shortly after on its own
    /// thread.
    pub fn paste(&self, ids: &[i64], paste: bool) -> Result<(), CoreError> {
        // SAFETY: no arguments; reads the process's TCC state.
        if paste && unsafe { AXIsProcessTrusted() } == 0 {
            return Err(CoreError::PermissionDenied);
        }
        let store = self.store()?;
        let stored = ids
            .iter()
            .map(|&id| {
                store.stored(id)?.ok_or_else(|| CoreError::Other {
                    message: "that clipboard item no longer exists".into(),
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        // The monitor sees this write and records it (or bumps the item to the top), like
        // any copy.
        board::write(&Stored::merge(stored)?)?;
        if paste {
            thread::spawn(|| {
                thread::sleep(PASTE_DELAY);
                post_command_v();
            });
        }
        Ok(())
    }

    pub fn delete(&self, ids: &[i64]) -> Result<(), CoreError> {
        if self.store()?.delete(ids)? > 0 {
            self.notify();
        }
        Ok(())
    }

    pub fn set_pinned(&self, ids: &[i64], pinned: bool) -> Result<(), CoreError> {
        if self.store()?.set_pinned(ids, pinned)? > 0 {
            self.notify();
        }
        Ok(())
    }

    /// Deletes every unpinned item.
    pub fn clear(&self) -> Result<(), CoreError> {
        let deleted = self.store()?.clear()?;
        if deleted > 0 {
            self.notify();
        }
        Ok(())
    }

    fn store(&self) -> Result<&Store, CoreError> {
        self.store.as_ref().ok_or_else(|| CoreError::Other {
            message: "clipboard history is unavailable".into(),
        })
    }

    /// Records whatever was just copied, if it should be kept.
    fn capture(&self) {
        let Some(store) = &self.store else {
            return;
        };
        let ignored = self.config.lock().ignored_apps.clone();
        let Some((content, source)) = board::read(&ignored) else {
            return;
        };
        let result = store.insert(&content, &source, now());
        match result {
            Ok(()) => {
                self.prune();
                self.notify();
            }
            Err(error) => log::error!("couldn't record a copy: {error}"),
        }
    }

    /// Applies the retention limits; returns how many items were removed.
    fn prune(&self) -> usize {
        let Some(store) = &self.store else {
            return 0;
        };
        let (max_items, retention_days) = {
            let config = self.config.lock();
            (config.max_items, config.retention_days)
        };
        let result = store.prune(max_items, retention_days, now());
        result.unwrap_or_else(|error| {
            log::error!("couldn't prune clipboard history: {error}");
            0
        })
    }

    /// Never called with a lock held: the shell may call straight back in.
    fn notify(&self) {
        let observer = self.observer.lock().clone();
        if let Some(observer) = observer {
            observer.clipboard_changed();
        }
    }
}

impl Drop for ClipboardHistory {
    fn drop(&mut self) {
        self.monitor.shut_down();
    }
}

/// Tells the monitor thread whether to poll, and wakes it when that changes.
#[derive(Default)]
struct MonitorSignal {
    state: Mutex<MonitorState>,
    wake: Condvar,
}

#[derive(Default)]
struct MonitorState {
    enabled: bool,
    /// Bumped each time history is turned on, so the monitor takes a fresh baseline even if
    /// it never saw it off.
    generation: u64,
    shutdown: bool,
}

impl MonitorSignal {
    fn set_enabled(&self, enabled: bool) {
        let mut state = self.state.lock();
        if enabled && !state.enabled {
            state.generation += 1;
        }
        state.enabled = enabled;
        self.wake.notify_one();
    }

    fn shut_down(&self) {
        self.state.lock().shutdown = true;
        self.wake.notify_one();
    }

    /// Sleeps for `poll` while enabled; while off, sleeps until turned on or `prune_in` has
    /// passed. Returns `(enabled, generation)`, or `None` once shut down.
    fn wait(&self, poll: Duration, prune_in: Duration) -> Option<(bool, u64)> {
        let mut state = self.state.lock();
        if !state.shutdown {
            if state.enabled {
                self.wake.wait_for(&mut state, poll);
            } else {
                self.wake
                    .wait_while_for(&mut state, |s| !s.enabled && !s.shutdown, prune_in);
            }
        }
        (!state.shutdown).then_some((state.enabled, state.generation))
    }
}

/// Polls the pasteboard's change counter while enabled, and leaves the pasteboard alone
/// while not. Holds only a weak reference to the history between polls.
fn run_monitor(signal: &MonitorSignal, history: &Weak<ClipboardHistory>) {
    // `None` until a baseline is taken: what was on the clipboard before monitoring
    // started (or while it was off) isn't recorded.
    let mut last_change = None;
    let mut last_generation = None;
    let mut last_prune = Instant::now();
    loop {
        let prune_in = PRUNE_INTERVAL.saturating_sub(last_prune.elapsed());
        let Some((enabled, generation)) = signal.wait(POLL_INTERVAL, prune_in) else {
            return;
        };
        let Some(history) = history.upgrade() else {
            return;
        };
        if last_generation != Some(generation) {
            last_generation = Some(generation);
            last_change = None;
        }
        if enabled {
            let count = board::change_count();
            if last_change.is_some_and(|last| last != count) {
                history.capture();
            }
            last_change = Some(count);
        }
        if last_prune.elapsed() >= PRUNE_INTERVAL {
            last_prune = Instant::now();
            if history.prune() > 0 {
                history.notify();
            }
        }
    }
}

fn post_command_v() {
    for down in [true, false] {
        let Some(event) = CGEvent::new_keyboard_event(None, KEY_V, down) else {
            return;
        };
        CGEvent::set_flags(Some(&event), CGEventFlags::MaskCommand);
        CGEvent::set_integer_value_field(
            Some(&event),
            CGEventField::EventSourceUserData,
            SYNTHETIC_EVENT_MARKER,
        );
        CGEvent::post(CGEventTapLocation::HIDEventTap, Some(&event));
    }
}

fn now() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0.0, |d| d.as_secs_f64())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::launcher::{PaletteMode, Status};
    use capture::Content;
    use std::fs;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use store::Source;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "turbo-clipboard-history-{name}-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    /// History off, so the monitor never touches the real pasteboard.
    fn config(max_items: u32, retention_days: u32) -> ClipboardConfig {
        ClipboardConfig {
            enabled: false,
            max_items,
            retention_days,
            ..ClipboardConfig::default()
        }
    }

    #[derive(Default)]
    struct Counter(AtomicUsize);

    impl Counter {
        fn count(&self) -> usize {
            self.0.load(Ordering::SeqCst)
        }
    }

    impl LauncherObserver for Counter {
        fn toggle_palette(&self) {}
        fn apps_changed(&self) {}
        fn command_finished(&self, _: String, _: String, _: bool) {}
        fn show_mode(&self, _: PaletteMode) {}
        fn clipboard_changed(&self) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
        fn keep_awake_changed(&self, _: bool) {}
        fn pick_color(&self) {}
        fn show_status(&self, _: Status) {}
    }

    fn observed(history: &ClipboardHistory) -> Arc<Counter> {
        let counter = Arc::new(Counter::default());
        history.set_observer(Some(counter.clone()));
        counter
    }

    fn record(history: &ClipboardHistory, text: &str, at: f64) -> i64 {
        let store = history.store.as_ref().unwrap();
        store
            .insert(&Content::Text(text.into()), &Source::default(), at)
            .unwrap();
        store.items(text, 1).unwrap()[0].id
    }

    fn texts(history: &ClipboardHistory) -> Vec<String> {
        history.items("", 100).into_iter().map(|i| i.text).collect()
    }

    #[test]
    fn changes_notify_the_observer_only_when_something_changed() {
        let dir = temp_dir("notify");
        let history = ClipboardHistory::new(&dir, config(0, 0));
        let counter = observed(&history);
        let a = record(&history, "a", now());
        record(&history, "b", now());

        history.set_pinned(&[a], true).unwrap();
        assert_eq!(counter.count(), 1);
        history.set_pinned(&[a], true).unwrap();
        assert_eq!(counter.count(), 1, "pinning a pinned item changes nothing");

        history.clear().unwrap();
        assert_eq!(counter.count(), 2);
        assert_eq!(texts(&history), vec!["a"]);
        history.clear().unwrap();
        assert_eq!(counter.count(), 2, "nothing unpinned was left to clear");

        history.delete(&[a]).unwrap();
        assert_eq!(counter.count(), 3);
        history.delete(&[a]).unwrap();
        assert_eq!(
            counter.count(),
            3,
            "deleting a missing item is a quiet no-op"
        );
        assert!(texts(&history).is_empty());

        history.set_observer(None);
        record(&history, "c", now());
        history.clear().unwrap();
        assert_eq!(counter.count(), 3);
        drop(history);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn shrinking_limits_prunes_at_once_and_notifies() {
        let dir = temp_dir("limits");
        let history = ClipboardHistory::new(&dir, config(0, 0));
        let counter = observed(&history);
        for text in ["a", "b", "c"] {
            record(&history, text, now());
        }

        history.set_config(config(10, 0));
        assert_eq!(counter.count(), 0, "nothing to prune");
        history.set_config(config(2, 0));
        assert_eq!(counter.count(), 1);
        assert_eq!(texts(&history), vec!["c", "b"]);
        drop(history);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn opening_applies_retention_to_old_items() {
        let dir = temp_dir("open-prune");
        {
            let history = ClipboardHistory::new(&dir, config(0, 0));
            record(&history, "ancient", 1.0);
            record(&history, "fresh", now());
        }
        let history = ClipboardHistory::new(&dir, config(0, 1));
        assert_eq!(texts(&history), vec!["fresh"]);
        drop(history);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn query_and_limit_are_passed_through() {
        let dir = temp_dir("query");
        let history = ClipboardHistory::new(&dir, config(0, 0));
        record(&history, "apple pie", 1.0);
        record(&history, "banana", 2.0);
        record(&history, "apple tart", 3.0);
        let found: Vec<_> = history
            .items("apple", 10)
            .into_iter()
            .map(|i| i.text)
            .collect();
        assert_eq!(found, vec!["apple tart", "apple pie"]);
        assert_eq!(history.items("", 1).len(), 1);
        assert!(history.items("", 0).is_empty());
        drop(history);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn pasting_a_missing_item_errs() {
        let dir = temp_dir("paste-missing");
        let history = ClipboardHistory::new(&dir, config(0, 0));
        // Fails on the lookup, before anything is written to the pasteboard.
        assert!(matches!(
            history.paste(&[12345], false),
            Err(CoreError::Other { .. })
        ));
        drop(history);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn an_unavailable_store_degrades_instead_of_crashing() {
        let dir = temp_dir("unavailable");
        fs::create_dir_all(&dir).unwrap();
        // A file where the store's directory should be.
        fs::write(dir.join("clipboard"), "").unwrap();
        let history = ClipboardHistory::new(&dir, config(0, 0));
        assert!(history.store.is_none());
        assert!(history.items("", 10).is_empty());
        assert!(history.delete(&[1]).is_err());
        assert!(history.set_pinned(&[1], true).is_err());
        assert!(history.clear().is_err());
        assert!(history.paste(&[1], false).is_err());
        history.set_config(config(1, 1));
        drop(history);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn turning_history_on_starts_a_new_generation() {
        let signal = MonitorSignal::default();
        let short = Duration::from_millis(1);
        assert_eq!(signal.wait(short, short), Some((false, 0)));
        signal.set_enabled(true);
        assert_eq!(signal.wait(short, short), Some((true, 1)));
        signal.set_enabled(true);
        assert_eq!(signal.wait(short, short), Some((true, 1)), "already on");
        signal.set_enabled(false);
        signal.set_enabled(true);
        assert_eq!(signal.wait(short, short), Some((true, 2)));
    }

    #[test]
    fn a_waiting_monitor_wakes_when_turned_on_or_shut_down() {
        let signal = Arc::new(MonitorSignal::default());
        let long = Duration::from_secs(30);
        // A short poll, so a waiter that only starts after being turned on still returns.
        let poll = Duration::from_millis(1);
        for shut_down in [false, true] {
            let waiter = {
                let signal = Arc::clone(&signal);
                thread::spawn(move || {
                    let started = Instant::now();
                    (signal.wait(poll, long), started.elapsed())
                })
            };
            thread::sleep(Duration::from_millis(50));
            if shut_down {
                signal.shut_down();
            } else {
                signal.set_enabled(true);
            }
            let (result, waited) = waiter.join().unwrap();
            assert!(waited < Duration::from_secs(10), "the wait wasn't woken");
            if shut_down {
                assert_eq!(result, None);
            } else {
                assert_eq!(result, Some((true, 1)));
                signal.set_enabled(false);
            }
        }
        assert_eq!(signal.wait(long, long), None, "stays shut down");
    }

    /// Reads (never writes) the real pasteboard: `cargo test -- --ignored clipboard_reads`.
    #[test]
    #[ignore]
    fn clipboard_reads_the_real_pasteboard() {
        let count = board::change_count();
        let read = board::read(&[]);
        let summary = read.as_ref().map(|(content, source)| {
            let preview: String = content.text().chars().take(40).collect();
            (content.kind(), preview, source.bundle_id.clone())
        });
        println!("change count {count}; {summary:?}");
    }
}
