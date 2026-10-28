//! The drag tap: an active, session-level tap on left and right button events, on its own
//! thread and run loop. A press with exactly the chosen modifiers starts a drag; that press,
//! its drags and its release are swallowed, so the app under the cursor never sees a click,
//! and handed to the worker, which does the (possibly slow) Accessibility calls.
//!
//! Every click and drag system-wide passes through the callback, so it never blocks:
//! config changes are picked up through a generation counter and a `try_lock`.

use parking_lot::Mutex;
use std::ffi::c_void;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::ptr::{self, NonNull};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Sender};
use std::thread::{self, JoinHandle};

use objc2_core_foundation::{
    CFMachPort, CFRetained, CFRunLoop, kCFRunLoopCommonModes, kCFRunLoopDefaultMode,
};
use objc2_core_graphics::{
    CGEvent, CGEventField, CGEventFlags, CGEventMask, CGEventTapLocation, CGEventTapOptions,
    CGEventTapPlacement, CGEventTapProxy, CGEventType,
};

use super::super::WindowManager;
use super::geometry::Mode;
use super::worker::{self, Message};
use crate::config::{Modifier, WindowDragConfig};
use crate::error::CoreError;
use crate::input::{SYNTHETIC_EVENT_MARKER, modifier_flag};

const EVENTS_OF_INTEREST: [CGEventType; 6] = [
    CGEventType::LeftMouseDown,
    CGEventType::LeftMouseUp,
    CGEventType::LeftMouseDragged,
    CGEventType::RightMouseDown,
    CGEventType::RightMouseUp,
    CGEventType::RightMouseDragged,
];

/// State shared between the API (any thread) and the tap thread.
struct Shared {
    config: Mutex<Arc<WindowDragConfig>>,
    /// Bumped on every config change so the tap only takes the lock when something changed.
    generation: AtomicU64,
}

pub(super) struct DragTap {
    shared: Arc<Shared>,
    windows: Arc<WindowManager>,
    threads: Mutex<Option<Threads>>,
}

struct Threads {
    run_loop: RunLoopHandle,
    stop: Arc<AtomicBool>,
    tap: JoinHandle<()>,
    worker: JoinHandle<()>,
}

/// `CFRunLoopStop` is documented as safe to call from any thread, which is all the API
/// thread does with the tap thread's run loop.
struct RunLoopHandle(CFRetained<CFRunLoop>);
unsafe impl Send for RunLoopHandle {}

impl DragTap {
    pub fn new(config: WindowDragConfig, windows: Arc<WindowManager>) -> Self {
        Self {
            shared: Arc::new(Shared {
                config: Mutex::new(Arc::new(config)),
                generation: AtomicU64::new(0),
            }),
            windows,
            threads: Mutex::new(None),
        }
    }

    pub fn set_config(&self, config: WindowDragConfig) {
        *self.shared.config.lock() = Arc::new(config);
        self.shared.generation.fetch_add(1, Ordering::Release);
    }

    /// Idempotent. The caller has checked Accessibility; failing to create the tap anyway
    /// is reported as `PermissionDenied`.
    pub fn start(&self) -> Result<(), CoreError> {
        let mut threads = self.threads.lock();
        if threads.is_some() {
            return Ok(());
        }
        // The worker runs until the tap thread drops its sender.
        let (messages, inbox) = mpsc::channel();
        let windows = Arc::clone(&self.windows);
        let worker = thread::Builder::new()
            .name("jaycast-window-drag".into())
            .spawn(move || worker::run(inbox, &windows))?;

        let stop = Arc::new(AtomicBool::new(false));
        let (ready_tx, ready_rx) = mpsc::sync_channel(1);
        let shared = Arc::clone(&self.shared);
        let thread_stop = Arc::clone(&stop);
        let tap = thread::Builder::new()
            .name("jaycast-window-drag-tap".into())
            .spawn(move || run_tap_thread(shared, messages, thread_stop, ready_tx));
        let tap = match tap {
            Ok(tap) => tap,
            Err(error) => {
                let _ = worker.join();
                return Err(error.into());
            }
        };

        match ready_rx.recv() {
            Ok(Ok(run_loop)) => {
                *threads = Some(Threads {
                    run_loop,
                    stop,
                    tap,
                    worker,
                });
                Ok(())
            }
            Ok(Err(error)) => {
                let _ = tap.join();
                let _ = worker.join();
                Err(error)
            }
            Err(_) => {
                let _ = tap.join();
                let _ = worker.join();
                Err(CoreError::Other {
                    message: "window drag thread exited during startup".into(),
                })
            }
        }
    }

    /// Ends a drag in progress where it is.
    pub fn stop(&self) {
        let Some(threads) = self.threads.lock().take() else {
            return;
        };
        threads.stop.store(true, Ordering::Release);
        threads.run_loop.0.stop();
        let _ = threads.tap.join();
        let _ = threads.worker.join();
    }

    #[cfg(test)]
    pub fn is_running(&self) -> bool {
        self.threads.lock().is_some()
    }
}

impl Drop for DragTap {
    fn drop(&mut self) {
        self.stop();
    }
}

fn run_tap_thread(
    shared: Arc<Shared>,
    messages: Sender<Message>,
    stop: Arc<AtomicBool>,
    ready: mpsc::SyncSender<Result<RunLoopHandle, CoreError>>,
) {
    let generation = shared.generation.load(Ordering::Acquire);
    let config = Arc::clone(&shared.config.lock());
    // The state lives in a Box so the callback can hold a stable pointer to it; it is only
    // ever touched from this thread, and freed after the run loop has stopped.
    let state = Box::into_raw(Box::new(TapState {
        shared,
        generation,
        config,
        grab: Grab::default(),
        messages,
        port: None,
    }));

    let mask: CGEventMask = EVENTS_OF_INTEREST
        .iter()
        .fold(0, |mask, event_type| mask | (1 << event_type.0));
    // SAFETY: `tap_callback` matches the callback ABI; `state` outlives the tap.
    let port = unsafe {
        CGEvent::tap_create(
            CGEventTapLocation::SessionEventTap,
            CGEventTapPlacement::HeadInsertEventTap,
            CGEventTapOptions::Default,
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
        drop(unsafe { Box::from_raw(state) });
        let _ = ready.send(Err(CoreError::Other {
            message: "could not create the window drag tap's run loop source".into(),
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
    let default_mode = unsafe { kCFRunLoopDefaultMode };
    while !stop.load(Ordering::Acquire) {
        CFRunLoop::run_in_mode(default_mode, 0.5, false);
    }

    CGEvent::tap_enable(&port, false);
    source.invalidate();
    port.invalidate();
    // SAFETY: the tap and source are invalidated, so no callback can run again. Dropping
    // the state drops the sender, which ends the worker.
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
    // A panic must never unwind into CoreGraphics or eat the user's input.
    let swallow =
        catch_unwind(AssertUnwindSafe(|| state.handle(event_type, event_ref))).unwrap_or(false);
    if swallow {
        ptr::null_mut()
    } else {
        event.as_ptr()
    }
}

/// Everything the tap thread owns.
struct TapState {
    shared: Arc<Shared>,
    generation: u64,
    config: Arc<WindowDragConfig>,
    grab: Grab,
    messages: Sender<Message>,
    port: Option<CFRetained<CFMachPort>>,
}

impl TapState {
    /// True to swallow the event.
    fn handle(&mut self, event_type: CGEventType, event: &CGEvent) -> bool {
        if event_type == CGEventType::TapDisabledByTimeout
            || event_type == CGEventType::TapDisabledByUserInput
        {
            // We may have missed the release while disabled: end the drag where it is.
            if let Some(port) = &self.port {
                CGEvent::tap_enable(port, true);
            }
            if self.grab.reset() {
                let _ = self.messages.send(Message::End);
            }
            return false;
        }
        if CGEvent::integer_value_field(Some(event), CGEventField::EventSourceUserData)
            == SYNTHETIC_EVENT_MARKER
        {
            return false;
        }
        let Some((button, phase)) = classify(event_type) else {
            return false;
        };
        self.refresh_config();

        let location = CGEvent::location(Some(event));
        let point = (location.x, location.y);
        let held = phase == Phase::Down
            && modifiers_match(CGEvent::flags(Some(event)), &self.config.modifiers);
        let (swallow, messages) =
            self.grab
                .on(button, phase, held, self.config.swap_buttons, point);
        for message in messages.into_iter().flatten() {
            let _ = self.messages.send(message);
        }
        swallow
    }

    /// Picks up a config change without ever blocking: if the setter holds the lock right
    /// now, keep the old config and try again on the next event.
    fn refresh_config(&mut self) {
        if self.shared.generation.load(Ordering::Acquire) == self.generation {
            return;
        }
        let Some(config) = self.shared.config.try_lock() else {
            return;
        };
        self.generation = self.shared.generation.load(Ordering::Acquire);
        self.config = Arc::clone(&config);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Button {
    Left,
    Right,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    Down,
    Dragged,
    Up,
}

fn classify(event_type: CGEventType) -> Option<(Button, Phase)> {
    Some(match event_type {
        CGEventType::LeftMouseDown => (Button::Left, Phase::Down),
        CGEventType::LeftMouseDragged => (Button::Left, Phase::Dragged),
        CGEventType::LeftMouseUp => (Button::Left, Phase::Up),
        CGEventType::RightMouseDown => (Button::Right, Phase::Down),
        CGEventType::RightMouseDragged => (Button::Right, Phase::Dragged),
        CGEventType::RightMouseUp => (Button::Right, Phase::Up),
        _ => return None,
    })
}

/// Exactly `wanted`, ignoring Caps Lock, Fn and the rest. Never matches an empty set.
fn modifiers_match(flags: CGEventFlags, wanted: &[Modifier]) -> bool {
    if wanted.is_empty() {
        return false;
    }
    let all = [
        Modifier::Shift,
        Modifier::Control,
        Modifier::Option,
        Modifier::Command,
    ];
    all.into_iter()
        .all(|m| flags.contains(modifier_flag(m)) == wanted.contains(&m))
}

/// Which button, if any, is dragging a window (pure, so the tap's decisions are tested).
#[derive(Debug, Default)]
struct Grab {
    held: Option<Button>,
}

impl Grab {
    /// What to do with one button event: whether to swallow it, and what to tell the worker
    /// (at most an end and a begin). `modifiers_held` is whether exactly the chosen
    /// modifiers are held; it only matters for a press. Once a drag starts, letting go of
    /// the modifiers doesn't end it: only releasing the button does.
    fn on(
        &mut self,
        button: Button,
        phase: Phase,
        modifiers_held: bool,
        swap_buttons: bool,
        point: (f64, f64),
    ) -> (bool, [Option<Message>; 2]) {
        match (self.held, phase) {
            (Some(held), Phase::Dragged) if held == button => {
                (true, [Some(Message::Drag { point }), None])
            }
            (Some(held), Phase::Up) if held == button => {
                self.held = None;
                (true, [Some(Message::End), None])
            }
            // The same button pressed again: its release was missed, so that drag is over.
            (Some(held), Phase::Down) if held == button => {
                self.held = None;
                let (swallow, [begin, _]) =
                    self.on(button, phase, modifiers_held, swap_buttons, point);
                (swallow, [Some(Message::End), begin])
            }
            // The other button during a drag is left alone.
            (Some(_), _) => (false, [None, None]),
            (None, Phase::Down) if modifiers_held => {
                self.held = Some(button);
                let mode = if (button == Button::Left) != swap_buttons {
                    Mode::Move
                } else {
                    Mode::Resize
                };
                (true, [Some(Message::Begin { point, mode }), None])
            }
            (None, _) => (false, [None, None]),
        }
    }

    /// Forgets the drag; true if there was one.
    fn reset(&mut self) -> bool {
        self.held.take().is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const AT: (f64, f64) = (10.0, 20.0);

    #[test]
    fn exact_modifiers_only() {
        let wanted = [Modifier::Control, Modifier::Command];
        let ctrl_cmd = CGEventFlags::MaskControl | CGEventFlags::MaskCommand;
        assert!(modifiers_match(ctrl_cmd, &wanted));
        assert!(modifiers_match(
            ctrl_cmd | CGEventFlags::MaskAlphaShift,
            &wanted
        ));
        assert!(!modifiers_match(
            ctrl_cmd | CGEventFlags::MaskShift,
            &wanted
        ));
        assert!(!modifiers_match(CGEventFlags::MaskCommand, &wanted));
        assert!(!modifiers_match(CGEventFlags::empty(), &[]));
    }

    #[test]
    fn a_modified_press_grabs_until_release() {
        let mut grab = Grab::default();
        let (swallow, [begin, _]) = grab.on(Button::Left, Phase::Down, true, false, AT);
        assert!(swallow);
        assert!(matches!(
            begin,
            Some(Message::Begin {
                mode: Mode::Move,
                ..
            })
        ));
        // Letting go of the modifiers mid-drag keeps dragging.
        let (swallow, [drag, _]) = grab.on(Button::Left, Phase::Dragged, false, false, AT);
        assert!(swallow);
        assert!(matches!(drag, Some(Message::Drag { .. })));
        let (swallow, [end, _]) = grab.on(Button::Left, Phase::Up, false, false, AT);
        assert!(swallow);
        assert!(matches!(end, Some(Message::End)));
        assert!(!grab.on(Button::Left, Phase::Dragged, false, false, AT).0);
    }

    #[test]
    fn right_resizes_unless_swapped() {
        let mut grab = Grab::default();
        let (_, [begin, _]) = grab.on(Button::Right, Phase::Down, true, false, AT);
        assert!(matches!(
            begin,
            Some(Message::Begin {
                mode: Mode::Resize,
                ..
            })
        ));
        let mut grab = Grab::default();
        let (_, [begin, _]) = grab.on(Button::Left, Phase::Down, true, true, AT);
        assert!(matches!(
            begin,
            Some(Message::Begin {
                mode: Mode::Resize,
                ..
            })
        ));
    }

    #[test]
    fn plain_clicks_and_the_other_button_pass_through() {
        let mut grab = Grab::default();
        assert!(!grab.on(Button::Left, Phase::Down, false, false, AT).0);
        assert!(!grab.on(Button::Left, Phase::Up, false, false, AT).0);
        grab.on(Button::Left, Phase::Down, true, false, AT);
        assert!(!grab.on(Button::Right, Phase::Down, true, false, AT).0);
        assert!(!grab.on(Button::Right, Phase::Up, true, false, AT).0);
    }

    #[test]
    fn a_missed_release_ends_the_old_drag() {
        let mut grab = Grab::default();
        grab.on(Button::Left, Phase::Down, true, false, AT);
        let (swallow, [end, begin]) = grab.on(Button::Left, Phase::Down, true, false, AT);
        assert!(swallow);
        assert!(matches!(end, Some(Message::End)));
        assert!(matches!(begin, Some(Message::Begin { .. })));
        let (swallow, [end, begin]) = grab.on(Button::Left, Phase::Down, false, false, AT);
        assert!(!swallow);
        assert!(matches!(end, Some(Message::End)));
        assert!(begin.is_none());
    }

    /// Creates a real tap, so it needs Accessibility for the test runner's host app.
    /// Run with `cargo test -p jaycast-core -- --ignored drag_tap_starts_and_stops`.
    #[test]
    #[ignore]
    fn drag_tap_starts_and_stops() {
        let windows = Arc::new(WindowManager::new(Default::default()));
        let tap = DragTap::new(WindowDragConfig::default(), windows);
        match tap.start() {
            Err(CoreError::PermissionDenied) => {
                eprintln!("skipped: not trusted for Accessibility");
                return;
            }
            other => other.unwrap(),
        }
        assert!(tap.is_running());
        tap.start().unwrap();
        tap.stop();
        assert!(!tap.is_running());
        tap.stop();
    }
}
