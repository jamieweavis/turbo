//! The CGEventTap: a session-level, head-inserted tap on a dedicated thread with its own
//! run loop. It routes mouse buttons through [`ButtonMachine`] and wheel ticks through
//! [`scroll::route`], and owns the [`SmoothScroller`].
//!
//! The callback runs on the hot path of every mouse event system-wide, so it never blocks:
//! config changes are picked up through a generation counter and a `try_lock`.

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
    CFAbsoluteTimeGetCurrent, CFMachPort, CFRetained, CFRunLoop, CFRunLoopTimer,
    CFRunLoopTimerContext, kCFRunLoopCommonModes, kCFRunLoopDefaultMode,
};
use objc2_core_graphics::{
    CGEvent, CGEventField, CGEventFlags, CGEventMask, CGEventTapLocation, CGEventTapOptions,
    CGEventTapPlacement, CGEventTapProxy, CGEventType,
};

use super::SYNTHETIC_EVENT_MARKER;
use super::actions;
use super::buttons::{ButtonMachine, Disposition, Effect};
use super::scroll::{self, Axes, ScrollDecision, WheelEvent};
use super::smooth_scroll::SmoothScroller;
use crate::config::{ButtonBinding, Modifier, MouseConfig};
use crate::error::CoreError;

#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    fn AXIsProcessTrusted() -> u8;
}

unsafe extern "C" {
    fn pthread_set_qos_class_self_np(qos_class: u32, relative_priority: i32) -> i32;
}
const QOS_CLASS_USER_INTERACTIVE: u32 = 0x21;

/// Far enough in the future to mean "never"; the button timer idles here.
const NEVER: f64 = 1.0e12;

const EVENTS_OF_INTEREST: [CGEventType; 6] = [
    CGEventType::LeftMouseDown,
    CGEventType::RightMouseDown,
    CGEventType::OtherMouseDown,
    CGEventType::OtherMouseUp,
    CGEventType::OtherMouseDragged,
    CGEventType::ScrollWheel,
];

/// State shared between the API (any thread) and the tap thread.
struct Shared {
    config: Mutex<Arc<MouseConfig>>,
    /// Bumped on every config change so the tap only takes the lock when something changed.
    generation: AtomicU64,
    scroller: SmoothScroller,
}

pub(crate) struct EventTap {
    shared: Arc<Shared>,
    worker: Mutex<Option<Worker>>,
}

struct Worker {
    run_loop: RunLoopHandle,
    stop: Arc<AtomicBool>,
    thread: JoinHandle<()>,
}

/// `CFRunLoopStop` and `CFRunLoopWakeUp` are documented as safe to call from any thread,
/// which is all the API thread does with the tap thread's run loop.
struct RunLoopHandle(CFRetained<CFRunLoop>);
unsafe impl Send for RunLoopHandle {}

impl EventTap {
    pub fn new(config: MouseConfig) -> Self {
        let scroller = SmoothScroller::new(config.smooth_scroll.clone());
        Self {
            shared: Arc::new(Shared {
                config: Mutex::new(Arc::new(config)),
                generation: AtomicU64::new(0),
                scroller,
            }),
            worker: Mutex::new(None),
        }
    }

    pub fn set_config(&self, config: MouseConfig) {
        self.shared
            .scroller
            .set_config(config.smooth_scroll.clone());
        let mut current = self.shared.config.lock();
        *current = Arc::new(config);
        self.shared.generation.fetch_add(1, Ordering::Release);
    }

    pub fn start(&self) -> Result<(), CoreError> {
        let mut worker = self.worker.lock();
        if worker.is_some() {
            return Ok(());
        }
        // SAFETY: no arguments; reads the process's TCC state.
        if unsafe { AXIsProcessTrusted() } == 0 {
            return Err(CoreError::PermissionDenied);
        }

        let stop = Arc::new(AtomicBool::new(false));
        let (ready_tx, ready_rx) = mpsc::sync_channel(1);
        let shared = Arc::clone(&self.shared);
        let thread_stop = Arc::clone(&stop);
        let thread = thread::Builder::new()
            .name("jaycast-event-tap".into())
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
                    message: "event tap thread exited during startup".into(),
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

impl Drop for EventTap {
    fn drop(&mut self) {
        self.stop();
    }
}

fn run_tap_thread(
    shared: Arc<Shared>,
    stop: Arc<AtomicBool>,
    ready: mpsc::SyncSender<Result<RunLoopHandle, CoreError>>,
) {
    // SAFETY: adjusts only the calling thread's QoS.
    unsafe { pthread_set_qos_class_self_np(QOS_CLASS_USER_INTERACTIVE, 0) };

    // The state lives in a Box so the callbacks can hold a stable pointer to it; it is
    // only ever touched from this thread, and freed after the run loop has stopped.
    let state = Box::into_raw(Box::new(TapState::new(Arc::clone(&shared))));

    let mask = EVENTS_OF_INTEREST
        .iter()
        .fold(0 as CGEventMask, |mask, t| mask | (1 << t.0));
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
            message: "could not create the event tap's run loop source".into(),
        }));
        return;
    };

    let mut context = CFRunLoopTimerContext {
        version: 0,
        info: state.cast(),
        retain: None,
        release: None,
        copyDescription: None,
    };
    // SAFETY: `timer_callback` matches the callback ABI; CF copies the context struct and
    // `state` outlives the timer.
    let timer = unsafe {
        CFRunLoopTimer::new(None, NEVER, NEVER, 0, 0, Some(timer_callback), &mut context)
    };
    let Some(timer) = timer else {
        drop(unsafe { Box::from_raw(state) });
        let _ = ready.send(Err(CoreError::Other {
            message: "could not create the button timer".into(),
        }));
        return;
    };

    // SAFETY: the mode constants are immutable CF globals.
    let common_modes = unsafe { kCFRunLoopCommonModes };
    run_loop.add_source(Some(&source), common_modes);
    run_loop.add_timer(Some(&timer), common_modes);
    // SAFETY: still the only user of `state`; the run loop isn't running yet.
    unsafe {
        (*state).port = Some(port.clone());
        (*state).timer = Some(timer.clone());
    }
    CGEvent::tap_enable(&port, true);
    let _ = ready.send(Ok(RunLoopHandle(run_loop.clone())));

    // The timeout is a backstop in case a stop request lands between checks.
    let default_mode = unsafe { kCFRunLoopDefaultMode };
    while !stop.load(Ordering::Acquire) {
        CFRunLoop::run_in_mode(default_mode, 0.5, false);
    }

    CGEvent::tap_enable(&port, false);
    timer.invalidate();
    source.invalidate();
    port.invalidate();
    shared.scroller.stop();
    // SAFETY: the tap, source and timer are invalidated, so no callback can run again.
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
    let disposition = catch_unwind(AssertUnwindSafe(|| state.handle(event_type, event_ref)))
        .unwrap_or(Disposition::PassThrough);
    match disposition {
        Disposition::PassThrough => event.as_ptr(),
        Disposition::Swallow => ptr::null_mut(),
    }
}

unsafe extern "C-unwind" fn timer_callback(_timer: *mut CFRunLoopTimer, info: *mut c_void) {
    // SAFETY: `info` is the TapState pointer, used only on this thread.
    let state = unsafe { &mut *info.cast::<TapState>() };
    let _ = catch_unwind(AssertUnwindSafe(|| state.fire_deadlines()));
}

/// Everything the tap thread owns.
struct TapState {
    shared: Arc<Shared>,
    config: Arc<MouseConfig>,
    generation: u64,
    buttons: ButtonMachine,
    port: Option<CFRetained<CFMachPort>>,
    timer: Option<CFRetained<CFRunLoopTimer>>,
}

impl TapState {
    fn new(shared: Arc<Shared>) -> Self {
        let config = Arc::clone(&shared.config.lock());
        let generation = shared.generation.load(Ordering::Acquire);
        Self {
            buttons: ButtonMachine::new(active_bindings(&config).to_vec()),
            shared,
            config,
            generation,
            port: None,
            timer: None,
        }
    }

    fn handle(&mut self, event_type: CGEventType, event: &CGEvent) -> Disposition {
        if event_type == CGEventType::TapDisabledByTimeout
            || event_type == CGEventType::TapDisabledByUserInput
        {
            log::warn!("mouse tap disabled by the system ({event_type:?}); re-enabling");
            // We missed events while disabled, so any half-tracked press is stale.
            if let Some(port) = &self.port {
                CGEvent::tap_enable(port, true);
            }
            self.buttons.reset();
            self.shared.scroller.stop();
            self.schedule_timer();
            return Disposition::PassThrough;
        }
        if CGEvent::integer_value_field(Some(event), CGEventField::EventSourceUserData)
            == SYNTHETIC_EVENT_MARKER
        {
            return Disposition::PassThrough;
        }
        self.refresh_config();

        match event_type {
            CGEventType::LeftMouseDown | CGEventType::RightMouseDown => {
                self.shared.scroller.stop();
                Disposition::PassThrough
            }
            CGEventType::OtherMouseDown => {
                self.shared.scroller.stop();
                let location = CGEvent::location(Some(event));
                let decision = self.buttons.down(
                    button_number(event),
                    Instant::now(),
                    (location.x, location.y),
                );
                self.apply(decision.effects);
                decision.disposition
            }
            CGEventType::OtherMouseUp => {
                let decision = self.buttons.up(button_number(event), Instant::now());
                self.apply(decision.effects);
                decision.disposition
            }
            CGEventType::OtherMouseDragged => {
                let location = CGEvent::location(Some(event));
                let decision = self
                    .buttons
                    .dragged(button_number(event), (location.x, location.y));
                self.apply(decision.effects);
                decision.disposition
            }
            CGEventType::ScrollWheel => self.handle_scroll(event),
            _ => Disposition::PassThrough,
        }
    }

    fn handle_scroll(&mut self, event: &CGEvent) -> Disposition {
        let int = |field| CGEvent::integer_value_field(Some(event), field);
        let double = |field| CGEvent::double_value_field(Some(event), field);
        let wheel = WheelEvent {
            continuous: int(CGEventField::ScrollWheelEventIsContinuous) != 0,
            lines: Axes {
                vertical: int(CGEventField::ScrollWheelEventDeltaAxis1),
                horizontal: int(CGEventField::ScrollWheelEventDeltaAxis2),
            },
            fixed: Axes {
                vertical: double(CGEventField::ScrollWheelEventFixedPtDeltaAxis1),
                horizontal: double(CGEventField::ScrollWheelEventFixedPtDeltaAxis2),
            },
            points: Axes {
                vertical: int(CGEventField::ScrollWheelEventPointDeltaAxis1),
                horizontal: int(CGEventField::ScrollWheelEventPointDeltaAxis2),
            },
            modifiers: held_modifiers(CGEvent::flags(Some(event))),
        };

        match scroll::route(&wheel, &self.config) {
            ScrollDecision::PassThrough => Disposition::PassThrough,
            ScrollDecision::Swallow => Disposition::Swallow,
            ScrollDecision::Rewrite {
                lines,
                fixed,
                points,
                clear_modifiers,
            } => {
                let set_int = |field, value| {
                    CGEvent::set_integer_value_field(Some(event), field, value);
                };
                let set_double = |field, value| {
                    CGEvent::set_double_value_field(Some(event), field, value);
                };
                // Line deltas first: setting them can recompute the others.
                set_int(CGEventField::ScrollWheelEventDeltaAxis1, lines.vertical);
                set_int(CGEventField::ScrollWheelEventDeltaAxis2, lines.horizontal);
                set_double(
                    CGEventField::ScrollWheelEventFixedPtDeltaAxis1,
                    fixed.vertical,
                );
                set_double(
                    CGEventField::ScrollWheelEventFixedPtDeltaAxis2,
                    fixed.horizontal,
                );
                set_int(
                    CGEventField::ScrollWheelEventPointDeltaAxis1,
                    points.vertical,
                );
                set_int(
                    CGEventField::ScrollWheelEventPointDeltaAxis2,
                    points.horizontal,
                );
                if !clear_modifiers.is_empty() {
                    let flags = CGEvent::flags(Some(event)) & !actions::flags_for(&clear_modifiers);
                    CGEvent::set_flags(Some(event), flags);
                }
                Disposition::PassThrough
            }
            ScrollDecision::Smooth {
                vertical,
                horizontal,
                precise,
            } => {
                self.shared.scroller.feed(vertical, horizontal, precise);
                Disposition::Swallow
            }
            ScrollDecision::Zoom { zoom_in } => {
                actions::post_zoom(zoom_in);
                Disposition::Swallow
            }
        }
    }

    fn fire_deadlines(&mut self) {
        let effects = self.buttons.tick(Instant::now());
        self.apply(effects);
    }

    fn apply(&mut self, effects: Vec<Effect>) {
        for effect in effects {
            match effect {
                Effect::Perform(action) => actions::perform(&action),
                Effect::ReplayClick(button) => actions::post_click(button),
                Effect::ReplayDown(button) => actions::post_button(button, true),
            }
        }
        self.schedule_timer();
    }

    /// Points the single button timer at the machine's next deadline (or at "never").
    fn schedule_timer(&self) {
        let Some(timer) = &self.timer else { return };
        let fire_at = match self.buttons.next_deadline() {
            Some(deadline) => {
                let wait = deadline.saturating_duration_since(Instant::now());
                CFAbsoluteTimeGetCurrent() + wait.as_secs_f64()
            }
            None => NEVER,
        };
        timer.set_next_fire_date(fire_at);
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
        drop(config);
        if self.buttons_changed() {
            self.buttons = ButtonMachine::new(active_bindings(&self.config).to_vec());
            self.schedule_timer();
        }
    }

    fn buttons_changed(&self) -> bool {
        // Rebuilding drops in-flight presses, so only do it when bindings really changed.
        !self
            .buttons
            .has_same_bindings(active_bindings(&self.config))
    }
}

/// With buttons turned off the machine gets no bindings, so every button passes through.
fn active_bindings(config: &MouseConfig) -> &[ButtonBinding] {
    if config.buttons_enabled {
        &config.buttons
    } else {
        &[]
    }
}

fn button_number(event: &CGEvent) -> u32 {
    CGEvent::integer_value_field(Some(event), CGEventField::MouseEventButtonNumber) as u32
}

fn held_modifiers(flags: CGEventFlags) -> Vec<Modifier> {
    [
        Modifier::Shift,
        Modifier::Control,
        Modifier::Option,
        Modifier::Command,
    ]
    .into_iter()
    .filter(|m| flags.contains(actions::modifier_flag(*m)))
    .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn held_modifiers_reads_flags() {
        let flags = CGEventFlags::MaskShift | CGEventFlags::MaskCommand;
        assert_eq!(
            held_modifiers(flags),
            vec![Modifier::Shift, Modifier::Command]
        );
        assert!(held_modifiers(CGEventFlags::empty()).is_empty());
    }

    #[test]
    fn event_mask_covers_wheel_and_other_buttons() {
        let mask = EVENTS_OF_INTEREST
            .iter()
            .fold(0 as CGEventMask, |mask, t| mask | (1 << t.0));
        assert_ne!(mask & (1 << CGEventType::ScrollWheel.0), 0);
        assert_ne!(mask & (1 << CGEventType::OtherMouseDragged.0), 0);
        assert_eq!(mask & (1 << CGEventType::MouseMoved.0), 0);
    }

    /// Creates a real tap, so it needs Accessibility for the test runner's host app.
    /// Run with `cargo test -p jaycast-core -- --ignored tap_starts_and_stops`.
    #[test]
    #[ignore]
    fn tap_starts_and_stops() {
        let tap = EventTap::new(MouseConfig::default());
        match tap.start() {
            Err(CoreError::PermissionDenied) => {
                eprintln!("skipped: not trusted for Accessibility");
                return;
            }
            other => other.unwrap(),
        }
        assert!(tap.is_running());
        tap.start().unwrap();
        tap.set_config(MouseConfig {
            buttons_enabled: false,
            ..MouseConfig::default()
        });
        thread::sleep(std::time::Duration::from_millis(200));
        tap.stop();
        assert!(!tap.is_running());
        tap.stop();
        tap.start().unwrap();
        drop(tap);
    }
}
