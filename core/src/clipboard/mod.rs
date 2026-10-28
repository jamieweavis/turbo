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
use store::Store;

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
                eprintln!("jaycast: clipboard history unavailable: {error}");
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
            .name("jaycast-clipboard".into())
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
            eprintln!("jaycast: {error}");
            Vec::new()
        })
    }

    /// Puts the item back on the clipboard and, if `paste`, sends ⌘V to the frontmost app
    /// (needs Accessibility). The shell closes the palette first. Returns at once; the
    /// keystroke follows shortly after on its own thread.
    pub fn paste(&self, id: i64, paste: bool) -> Result<(), CoreError> {
        // SAFETY: no arguments; reads the process's TCC state.
        if paste && unsafe { AXIsProcessTrusted() } == 0 {
            return Err(CoreError::PermissionDenied);
        }
        let stored = self.store()?.stored(id)?.ok_or_else(|| CoreError::Other {
            message: "that clipboard item no longer exists".into(),
        })?;
        // The monitor sees this write and bumps the item to the top, like any re-copy.
        board::write(&stored)?;
        if paste {
            thread::spawn(|| {
                thread::sleep(PASTE_DELAY);
                post_command_v();
            });
        }
        Ok(())
    }

    pub fn delete(&self, id: i64) -> Result<(), CoreError> {
        let deleted = self.store()?.delete(id)?;
        if deleted {
            self.notify();
        }
        Ok(())
    }

    pub fn set_pinned(&self, id: i64, pinned: bool) -> Result<(), CoreError> {
        let changed = self.store()?.set_pinned(id, pinned)?;
        if changed {
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
            Err(error) => eprintln!("jaycast: couldn't record a copy: {error}"),
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
            eprintln!("jaycast: couldn't prune clipboard history: {error}");
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
