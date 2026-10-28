//! The expansion tap: a listen-only, session-level tap on key-downs (and mouse-downs, which
//! move the caret) on its own thread and run loop, plus a worker thread that does the
//! erasing and pasting.
//!
//! Listen-only means a stalled callback can't delay anyone's typing, and the tap can't eat
//! a key by mistake. The callback still does as little as possible: it feeds the
//! [`Matcher`] and, on a completed keyword, hands the snippet to the worker over a channel.

use parking_lot::Mutex;
use std::ffi::c_void;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::ptr::NonNull;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc;
use std::thread::{self, JoinHandle};
use std::time::Duration;

use objc2::rc::autoreleasepool;
use objc2_app_kit::NSWorkspace;
use objc2_core_foundation::{
    CFMachPort, CFRetained, CFRunLoop, kCFRunLoopCommonModes, kCFRunLoopDefaultMode,
};
use objc2_core_graphics::{
    CGEvent, CGEventField, CGEventFlags, CGEventMask, CGEventTapLocation, CGEventTapOptions,
    CGEventTapPlacement, CGEventTapProxy, CGEventType,
};

use super::insert;
use super::matcher::{self, Matcher};
use crate::config::SnippetsConfig;
use crate::error::CoreError;
use crate::input::SYNTHETIC_EVENT_MARKER;

#[link(name = "Carbon", kind = "framework")]
unsafe extern "C" {
    /// True while any app has secure input on (a password field, Terminal's Secure
    /// Keyboard Entry).
    fn IsSecureEventInputEnabled() -> u8;
}

const EVENTS_OF_INTEREST: [CGEventType; 4] = [
    CGEventType::KeyDown,
    CGEventType::LeftMouseDown,
    CGEventType::RightMouseDown,
    CGEventType::OtherMouseDown,
];

/// UTF-16 units read per key-down; one key produces one or two (a surrogate pair).
const MAX_KEY_TEXT: usize = 8;

/// State shared between the API (any thread) and the tap thread.
struct Shared {
    config: Mutex<Arc<SnippetsConfig>>,
    /// Bumped on every config change so the tap only takes the lock when something changed.
    generation: AtomicU64,
    /// The palette is a non-activating panel, so typing in it leaves another app frontmost;
    /// the shell reports it instead.
    palette_visible: AtomicBool,
}

pub(super) struct ExpansionTap {
    shared: Arc<Shared>,
    worker: Mutex<Option<Worker>>,
}

struct Worker {
    run_loop: RunLoopHandle,
    stop: Arc<AtomicBool>,
    thread: JoinHandle<()>,
}

/// `CFRunLoopStop` is documented as safe to call from any thread, which is all the API
/// thread does with the tap thread's run loop.
struct RunLoopHandle(CFRetained<CFRunLoop>);
unsafe impl Send for RunLoopHandle {}

/// A completed keyword, for the worker: erase it, then insert the snippet.
struct Job {
    text: String,
    erase: usize,
}

impl ExpansionTap {
    pub fn new(config: SnippetsConfig) -> Self {
        Self {
            shared: Arc::new(Shared {
                config: Mutex::new(Arc::new(config)),
                generation: AtomicU64::new(0),
                palette_visible: AtomicBool::new(false),
            }),
            worker: Mutex::new(None),
        }
    }

    pub fn set_config(&self, config: SnippetsConfig) {
        *self.shared.config.lock() = Arc::new(config);
        self.shared.generation.fetch_add(1, Ordering::Release);
    }

    pub fn set_palette_visible(&self, visible: bool) {
        self.shared
            .palette_visible
            .store(visible, Ordering::Relaxed);
    }

    /// Idempotent. The caller has checked Accessibility; failing to create the tap anyway
    /// is reported as `PermissionDenied`.
    pub fn start(&self) -> Result<(), CoreError> {
        let mut worker = self.worker.lock();
        if worker.is_some() {
            return Ok(());
        }
        let stop = Arc::new(AtomicBool::new(false));
        let (ready_tx, ready_rx) = mpsc::sync_channel(1);
        let shared = Arc::clone(&self.shared);
        let thread_stop = Arc::clone(&stop);
        let thread = thread::Builder::new()
            .name("jaycast-snippet-tap".into())
            .spawn(move || run_tap_thread(shared, thread_stop, ready_tx))?;

        match ready_rx.recv() {
            Ok(Ok(run_loop)) => {
                *worker = Some(Worker {
                    run_loop,
                    stop,
                    thread,
                });
                Ok(())
            }
            Ok(Err(error)) => {
                let _ = thread.join();
                Err(error)
            }
            Err(_) => {
                let _ = thread.join();
                Err(CoreError::Other {
                    message: "snippet tap thread exited during startup".into(),
                })
            }
        }
    }

    pub fn stop(&self) {
        let Some(worker) = self.worker.lock().take() else {
            return;
        };
        worker.stop.store(true, Ordering::Release);
        worker.run_loop.0.stop();
        let _ = worker.thread.join();
    }

    #[cfg(test)]
    pub fn is_running(&self) -> bool {
        self.worker.lock().is_some()
    }
}

impl Drop for ExpansionTap {
    fn drop(&mut self) {
        self.stop();
    }
}

fn run_tap_thread(
    shared: Arc<Shared>,
    stop: Arc<AtomicBool>,
    ready: mpsc::SyncSender<Result<RunLoopHandle, CoreError>>,
) {
    // The worker ends when the tap state (holding the only sender) is dropped, after
    // finishing any insertion in progress so the clipboard is always restored.
    let (jobs, queue) = mpsc::channel::<Job>();
    let spawned = thread::Builder::new()
        .name("jaycast-snippet-insert".into())
        .spawn(move || {
            for job in queue {
                insert::run(&job.text, job.erase, Duration::ZERO);
            }
        });
    if let Err(error) = spawned {
        let _ = ready.send(Err(error.into()));
        return;
    }

    // The state lives in a Box so the callback can hold a stable pointer to it; it is only
    // ever touched from this thread, and freed after the run loop has stopped.
    let state = Box::into_raw(Box::new(TapState::new(shared, jobs)));

    let mask = EVENTS_OF_INTEREST
        .iter()
        .fold(0 as CGEventMask, |mask, t| mask | (1 << t.0));
    // SAFETY: `tap_callback` matches the callback ABI; `state` outlives the tap.
    let port = unsafe {
        CGEvent::tap_create(
            CGEventTapLocation::SessionEventTap,
            CGEventTapPlacement::TailAppendEventTap,
            CGEventTapOptions::ListenOnly,
            mask,
            Some(tap_callback),
            state.cast(),
        )
    };
    let (Some(port), Some(run_loop)) = (port, CFRunLoop::current()) else {
        // SAFETY: nothing else holds the pointer; the tap was never created.
        drop(unsafe { Box::from_raw(state) });
        let _ = ready.send(Err(CoreError::PermissionDenied));
        return;
    };
    let Some(source) = CFMachPort::new_run_loop_source(None, Some(&port), 0) else {
        // SAFETY: as above; the tap was never added to a run loop.
        drop(unsafe { Box::from_raw(state) });
        let _ = ready.send(Err(CoreError::Other {
            message: "could not create the snippet tap's run loop source".into(),
        }));
        return;
    };

    // SAFETY: the mode constants are immutable CF globals.
    let common_modes = unsafe { kCFRunLoopCommonModes };
    run_loop.add_source(Some(&source), common_modes);
    // SAFETY: still the only user of `state`; the run loop isn't running yet.
    unsafe { (*state).port = Some(port.clone()) };
    CGEvent::tap_enable(&port, true);
    let _ = ready.send(Ok(RunLoopHandle(run_loop.clone())));

    // The timeout is a backstop in case a stop request lands between checks.
    // SAFETY: as above.
    let default_mode = unsafe { kCFRunLoopDefaultMode };
    while !stop.load(Ordering::Acquire) {
        CFRunLoop::run_in_mode(default_mode, 0.5, false);
    }

    CGEvent::tap_enable(&port, false);
    source.invalidate();
    port.invalidate();
    // SAFETY: the tap and source are invalidated, so no callback can run again.
    drop(unsafe { Box::from_raw(state) });
}

unsafe extern "C-unwind" fn tap_callback(
    _proxy: CGEventTapProxy,
    event_type: CGEventType,
    event: NonNull<CGEvent>,
    user_info: *mut c_void,
) -> *mut CGEvent {
    // SAFETY: `user_info` is the TapState pointer, used only on this thread.
    let state = unsafe { &mut *user_info.cast::<TapState>() };
    // SAFETY: CG hands us a valid event for the duration of the callback.
    let event_ref = unsafe { event.as_ref() };
    // A panic must never unwind into CoreGraphics. If one happens mid-key, forget the
    // buffer rather than trust it.
    if catch_unwind(AssertUnwindSafe(|| state.handle(event_type, event_ref))).is_err() {
        state.matcher.reset();
    }
    // Ignored for a listen-only tap, but the event is passed back unchanged regardless.
    event.as_ptr()
}

/// Everything the tap thread owns.
struct TapState {
    shared: Arc<Shared>,
    config: Arc<SnippetsConfig>,
    generation: u64,
    matcher: Matcher,
    /// The app the buffered characters were typed into.
    frontmost: Option<i32>,
    jobs: mpsc::Sender<Job>,
    port: Option<CFRetained<CFMachPort>>,
}

impl TapState {
    fn new(shared: Arc<Shared>, jobs: mpsc::Sender<Job>) -> Self {
        let config = Arc::clone(&shared.config.lock());
        let generation = shared.generation.load(Ordering::Acquire);
        Self {
            matcher: matcher_for(&config),
            shared,
            config,
            generation,
            frontmost: None,
            jobs,
            port: None,
        }
    }

    fn handle(&mut self, event_type: CGEventType, event: &CGEvent) {
        if event_type == CGEventType::TapDisabledByTimeout
            || event_type == CGEventType::TapDisabledByUserInput
        {
            // Keys typed while disabled were missed, so the buffer no longer matches.
            if let Some(port) = &self.port {
                CGEvent::tap_enable(port, true);
            }
            self.matcher.reset();
            return;
        }
        // Our own Backspaces, ⌘V and arrows (and other Jaycast input).
        if CGEvent::integer_value_field(Some(event), CGEventField::EventSourceUserData)
            == SYNTHETIC_EVENT_MARKER
        {
            return;
        }
        match event_type {
            CGEventType::KeyDown => self.key_down(event),
            // A click can move the caret anywhere.
            _ => self.matcher.reset(),
        }
    }

    fn key_down(&mut self, event: &CGEvent) {
        self.refresh_config();
        // SAFETY: no arguments; reads the session's secure input state.
        if unsafe { IsSecureEventInputEnabled() } != 0 {
            // Password fields: never look at what's typed.
            self.matcher.reset();
            return;
        }
        let frontmost = frontmost_pid();
        if frontmost != self.frontmost {
            self.matcher.reset();
            self.frontmost = frontmost;
        }
        // Typing in Jaycast itself (searching the palette for `;addr`) mustn't expand.
        if frontmost == Some(std::process::id() as i32)
            || self.shared.palette_visible.load(Ordering::Relaxed)
        {
            self.matcher.reset();
            return;
        }

        let flags = CGEvent::flags(Some(event));
        let key_code =
            CGEvent::integer_value_field(Some(event), CGEventField::KeyboardEventKeycode) as u16;
        let key = matcher::classify(
            key_code,
            flags.intersects(CGEventFlags::MaskCommand | CGEventFlags::MaskControl),
            flags.contains(CGEventFlags::MaskAlternate),
            &key_text(event),
        );
        let Some(completed) = self.matcher.key(key) else {
            return;
        };
        if let Some(snippet) = self.config.snippets.get(completed.snippet) {
            let _ = self.jobs.send(Job {
                text: snippet.text.clone(),
                erase: completed.erase,
            });
        }
    }

    /// Picks up a config change without ever blocking: if the setter holds the lock right
    /// now, keep the old config and try again on the next key.
    fn refresh_config(&mut self) {
        if self.shared.generation.load(Ordering::Acquire) == self.generation {
            return;
        }
        let Some(config) = self.shared.config.try_lock() else {
            return;
        };
        self.generation = self.shared.generation.load(Ordering::Acquire);
        self.config = Arc::clone(&config);
        drop(config);
        self.matcher = matcher_for(&self.config);
    }
}

/// A matcher over every snippet's keyword (indices into `config.snippets`).
pub(super) fn matcher_for(config: &SnippetsConfig) -> Matcher {
    Matcher::new(
        config
            .snippets
            .iter()
            .enumerate()
            .map(|(i, s)| (s.keyword.as_str(), i)),
    )
}

/// The text a key-down types, with the current layout and modifiers applied.
fn key_text(event: &CGEvent) -> String {
    let mut buffer = [0u16; MAX_KEY_TEXT];
    let mut length = 0;
    // SAFETY: `buffer` holds `MAX_KEY_TEXT` units and CG writes at most that many,
    // reporting how many in `length`.
    unsafe {
        CGEvent::keyboard_get_unicode_string(
            Some(event),
            MAX_KEY_TEXT as _,
            &mut length,
            buffer.as_mut_ptr(),
        );
    }
    String::from_utf16_lossy(&buffer[..(length as usize).min(MAX_KEY_TEXT)])
}

fn frontmost_pid() -> Option<i32> {
    autoreleasepool(|_| {
        NSWorkspace::sharedWorkspace()
            .frontmostApplication()
            .map(|app| app.processIdentifier())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_mask_covers_keys_and_clicks_only() {
        let mask = EVENTS_OF_INTEREST
            .iter()
            .fold(0 as CGEventMask, |mask, t| mask | (1 << t.0));
        assert_ne!(mask & (1 << CGEventType::KeyDown.0), 0);
        assert_ne!(mask & (1 << CGEventType::LeftMouseDown.0), 0);
        assert_eq!(mask & (1 << CGEventType::KeyUp.0), 0);
        assert_eq!(mask & (1 << CGEventType::MouseMoved.0), 0);
    }
}
