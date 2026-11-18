//! The Hyper tap: an active, HID-level tap on key and mouse button events, on its own
//! thread and run loop. HID level comes before global hotkeys and other apps' taps, so
//! they all see Hyper as ordinary ⌃⌥⇧⌘.
//!
//! Every key press system-wide passes through the callback, so it never blocks: config
//! changes are picked up through a generation counter and a `try_lock`.

use parking_lot::Mutex;
use std::ffi::c_void;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::ptr::{self, NonNull};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc;
use std::thread::{self, JoinHandle};
use std::time::Instant;

use objc2_core_foundation::{
    CFMachPort, CFRetained, CFRunLoop, kCFRunLoopCommonModes, kCFRunLoopDefaultMode,
};
use objc2_core_graphics::{
    CGEvent, CGEventField, CGEventFlags, CGEventMask, CGEventTapLocation, CGEventTapOptions,
    CGEventTapPlacement, CGEventTapProxy, CGEventType,
};

use super::machine::{Input, Machine, Outcome};
use super::power::{self, PowerNotifier};
use super::remap::{self, HYPER_KEY_CODE};
use crate::config::{HyperKeyConfig, QuickPress};
use crate::error::CoreError;
use crate::input::SYNTHETIC_EVENT_MARKER;

/// `kVK_Escape`.
const KEY_ESCAPE: u16 = 0x35;

const EVENTS_OF_INTEREST: [CGEventType; 11] = [
    CGEventType::KeyDown,
    CGEventType::KeyUp,
    CGEventType::FlagsChanged,
    CGEventType::LeftMouseDown,
    CGEventType::LeftMouseUp,
    CGEventType::LeftMouseDragged,
    CGEventType::RightMouseDown,
    CGEventType::RightMouseUp,
    CGEventType::RightMouseDragged,
    CGEventType::OtherMouseDown,
    CGEventType::OtherMouseUp,
];

/// State shared between the API (any thread) and the tap thread.
struct Shared {
    config: Mutex<Arc<HyperKeyConfig>>,
    /// Bumped on every config change so the tap only takes the lock when something changed.
    generation: AtomicU64,
}

pub(super) struct HyperTap {
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

impl HyperTap {
    pub fn new(config: HyperKeyConfig) -> Self {
        Self {
            shared: Arc::new(Shared {
                config: Mutex::new(Arc::new(config)),
                generation: AtomicU64::new(0),
            }),
            worker: Mutex::new(None),
        }
    }

    pub fn set_config(&self, config: HyperKeyConfig) {
        *self.shared.config.lock() = Arc::new(config);
        self.shared.generation.fetch_add(1, Ordering::Release);
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
            .name("backpack-hyper-tap".into())
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
                    message: "Hyper key thread exited during startup".into(),
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

impl Drop for HyperTap {
    fn drop(&mut self) {
        self.stop();
    }
}

fn run_tap_thread(
    shared: Arc<Shared>,
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
        machine: Machine::default(),
        port: None,
        power: None,
    }));

    let mask: CGEventMask = EVENTS_OF_INTEREST
        .iter()
        .fold(0, |mask, event_type| mask | (1 << event_type.0));
    // SAFETY: `tap_callback` matches the callback ABI; `state` outlives the tap.
    let port = unsafe {
        CGEvent::tap_create(
            CGEventTapLocation::HIDEventTap,
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
            message: "could not create the Hyper key tap's run loop source".into(),
        }));
        return;
    };

    // SAFETY: the mode constants are immutable CF globals.
    let common_modes = unsafe { kCFRunLoopCommonModes };
    run_loop.add_source(Some(&source), common_modes);
    // SAFETY: `state` outlives the notifier (it owns it), and the callback runs on this
    // run loop's thread.
    let power = unsafe { PowerNotifier::register(&run_loop, state.cast(), power_callback) };
    if power.is_none() {
        log::warn!("couldn't register for sleep/wake; a Hyper press cut short by sleep may stick");
    }
    // SAFETY: still the only user of `state`; the run loop isn't running yet.
    unsafe {
        (*state).port = Some(port.clone());
        (*state).power = power;
    }
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
    // SAFETY: the tap and source are invalidated, and dropping the state deregisters the
    // power notifier, so no callback can run again.
    drop(unsafe { Box::from_raw(state) });
}

/// Sleep and wake: forget a held Hyper press, whose key-up can be lost while asleep.
unsafe extern "C-unwind" fn power_callback(
    refcon: *mut c_void,
    _service: u32,
    message: u32,
    argument: *mut c_void,
) {
    // SAFETY: `refcon` is the TapState pointer, used only on this thread.
    let state = unsafe { &mut *refcon.cast::<TapState>() };
    let _ = catch_unwind(AssertUnwindSafe(|| {
        if let Some(event) = power::classify(message) {
            if state.machine.is_held() {
                log::info!("Hyper released on {event:?}; its key-up may never arrive");
            }
            state.machine.reset();
        }
    }));
    // Always answer, even after a panic, or sleep waits 30 seconds for us.
    if let Some(power) = &state.power {
        power.acknowledge(message, argument);
    }
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
    let outcome = catch_unwind(AssertUnwindSafe(|| state.handle(event_type, event_ref)))
        .unwrap_or(Outcome::PassThrough);
    match outcome {
        Outcome::PassThrough => event.as_ptr(),
        Outcome::AddFlags(flags) => {
            let current = CGEvent::flags(Some(event_ref));
            CGEvent::set_flags(Some(event_ref), current | flags);
            event.as_ptr()
        }
        Outcome::Swallow => ptr::null_mut(),
        Outcome::SwallowAndQuickPress(action) => {
            quick_press(action);
            ptr::null_mut()
        }
    }
}

/// Everything the tap thread owns.
struct TapState {
    shared: Arc<Shared>,
    generation: u64,
    config: Arc<HyperKeyConfig>,
    machine: Machine,
    port: Option<CFRetained<CFMachPort>>,
    power: Option<PowerNotifier>,
}

impl TapState {
    fn handle(&mut self, event_type: CGEventType, event: &CGEvent) -> Outcome {
        if event_type == CGEventType::TapDisabledByTimeout
            || event_type == CGEventType::TapDisabledByUserInput
        {
            log::warn!("Hyper key tap disabled by the system ({event_type:?}); re-enabling");
            // We may have missed the Hyper key's key-up while disabled; better to drop
            // Hyper than to leave every key stuck with four modifiers.
            if let Some(port) = &self.port {
                CGEvent::tap_enable(port, true);
            }
            self.machine.reset();
            return Outcome::PassThrough;
        }
        if CGEvent::integer_value_field(Some(event), CGEventField::EventSourceUserData)
            == SYNTHETIC_EVENT_MARKER
        {
            return Outcome::PassThrough;
        }
        self.refresh_config();

        let is_hyper_key = || {
            CGEvent::integer_value_field(Some(event), CGEventField::KeyboardEventKeycode)
                == i64::from(HYPER_KEY_CODE)
        };
        let input = match event_type {
            CGEventType::KeyDown if is_hyper_key() => Input::HyperDown {
                repeat: CGEvent::integer_value_field(
                    Some(event),
                    CGEventField::KeyboardEventAutorepeat,
                ) != 0,
            },
            CGEventType::KeyUp if is_hyper_key() => Input::HyperUp,
            CGEventType::KeyDown
            | CGEventType::LeftMouseDown
            | CGEventType::RightMouseDown
            | CGEventType::OtherMouseDown => Input::Press,
            _ => Input::Other,
        };
        self.machine.handle(input, &self.config, Instant::now())
    }

    fn refresh_config(&mut self) {
        let generation = self.shared.generation.load(Ordering::Acquire);
        if generation == self.generation {
            return;
        }
        // Never wait on the API thread; pick the change up on the next event instead.
        if let Some(config) = self.shared.config.try_lock() {
            self.config = Arc::clone(&config);
            self.generation = generation;
        }
    }
}

fn quick_press(action: QuickPress) {
    match action {
        QuickPress::Nothing => {}
        QuickPress::Escape => {
            for down in [true, false] {
                if let Some(event) = CGEvent::new_keyboard_event(None, KEY_ESCAPE, down) {
                    CGEvent::set_flags(Some(&event), CGEventFlags::empty());
                    CGEvent::set_integer_value_field(
                        Some(&event),
                        CGEventField::EventSourceUserData,
                        SYNTHETIC_EVENT_MARKER,
                    );
                    CGEvent::post(CGEventTapLocation::HIDEventTap, Some(&event));
                }
            }
        }
        // IOKit round trips; keep them off the input path.
        QuickPress::ToggleCapsLock => {
            thread::spawn(remap::toggle_caps_lock);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_mask_covers_keys_and_buttons_but_not_scrolling() {
        let mask: CGEventMask = EVENTS_OF_INTEREST
            .iter()
            .fold(0, |mask, event_type| mask | (1 << event_type.0));
        assert_ne!(mask & (1 << CGEventType::KeyDown.0), 0);
        assert_ne!(mask & (1 << CGEventType::FlagsChanged.0), 0);
        assert_ne!(mask & (1 << CGEventType::OtherMouseUp.0), 0);
        assert_eq!(mask & (1 << CGEventType::ScrollWheel.0), 0);
        assert_eq!(mask & (1 << CGEventType::MouseMoved.0), 0);
    }
}
