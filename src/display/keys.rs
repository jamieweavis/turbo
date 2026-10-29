//! The brightness keys: a session-level tap on system-defined events, on its own thread and
//! run loop. A key press adjusts the external display under the cursor through
//! [`DisplayManager::adjust`]; on the built-in display, or one Jaycast can't control, the
//! key passes through so macOS handles it as usual.
//!
//! Key presses are rare, so the hot path may take the config lock briefly; it still never
//! waits on DDC, because `adjust` only queues the write.

use parking_lot::Mutex;
use std::ffi::c_void;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::ptr::{self, NonNull};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::thread::{self, JoinHandle};

use objc2_core_foundation::{
    CFMachPort, CFRetained, CFRunLoop, kCFRunLoopCommonModes, kCFRunLoopDefaultMode,
};
use objc2_core_graphics::{
    CGDirectDisplayID, CGDisplayIsBuiltin, CGError, CGEvent, CGEventField, CGEventFlags,
    CGEventMask, CGEventTapLocation, CGEventTapOptions, CGEventTapPlacement, CGEventTapProxy,
    CGEventType, CGGetDisplaysWithPoint,
};

use super::DisplayManager;
use crate::error::CoreError;

#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    fn AXIsProcessTrusted() -> u8;
}

/// `NX_SYSDEFINED` (IOLLEvent.h); CoreGraphics has no named `CGEventType` for it.
const SYSTEM_DEFINED: CGEventType = CGEventType(14);
/// Undocumented `CGEventField`s holding a system-defined event's `subtype` and `data1`, the
/// values `NSEvent` reports for them (the round-trip test pins this).
const FIELD_SUBTYPE: CGEventField = CGEventField(99);
const FIELD_DATA1: CGEventField = CGEventField(149);
/// `NX_SUBTYPE_AUX_CONTROL_BUTTONS` (IOLLEvent.h): media, volume and brightness keys.
const SUBTYPE_AUX_CONTROL_BUTTONS: i16 = 8;
/// `NX_KEYTYPE_BRIGHTNESS_UP` / `_DOWN` (ev_keymap.h).
const KEYTYPE_BRIGHTNESS_UP: isize = 2;
const KEYTYPE_BRIGHTNESS_DOWN: isize = 3;
/// Key state in bits 8–15 of `data1`: `NX_KEYDOWN` / `NX_KEYUP` (IOLLEvent.h).
const KEY_STATE_DOWN: isize = 0xA;
const KEY_STATE_UP: isize = 0xB;

/// One press moves 1/16 of the range, like the built-in display; ⌥⇧ gives quarter steps.
const STEP: f64 = 1.0 / 16.0;
const FINE_STEP: f64 = 1.0 / 64.0;

pub(crate) struct BrightnessKeyTap {
    displays: Arc<DisplayManager>,
    /// `start` was called (and `stop` hasn't been since).
    started: Mutex<bool>,
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

impl BrightnessKeyTap {
    pub fn new(displays: Arc<DisplayManager>) -> Self {
        Self {
            displays,
            started: Mutex::new(false),
            worker: Mutex::new(None),
        }
    }

    /// Fails with `PermissionDenied` without Accessibility. A no-op while running.
    pub fn start(&self) -> Result<(), CoreError> {
        let mut started = self.started.lock();
        // SAFETY: no arguments; reads the process's TCC state.
        if unsafe { AXIsProcessTrusted() } == 0 {
            return Err(CoreError::PermissionDenied);
        }
        *started = true;
        let result = self.sync(*started);
        if result.is_err() {
            *started = false;
        }
        result
    }

    pub fn stop(&self) {
        *self.started.lock() = false;
        self.stop_tap();
    }

    /// Call after the monitor settings change: the tap only runs while monitor control and
    /// the brightness keys are both on, so otherwise key presses never pass through Jaycast.
    pub fn refresh(&self) -> Result<(), CoreError> {
        let started = self.started.lock();
        self.sync(*started)
    }

    /// Started, whether or not the tap itself is running (it doesn't while turned off).
    pub fn is_running(&self) -> bool {
        *self.started.lock()
    }

    fn sync(&self, started: bool) -> Result<(), CoreError> {
        let config = self.displays.config();
        if started && config.enabled && config.brightness_keys {
            self.start_tap()
        } else {
            self.stop_tap();
            Ok(())
        }
    }

    fn start_tap(&self) -> Result<(), CoreError> {
        let mut worker = self.worker.lock();
        if worker.is_some() {
            return Ok(());
        }

        let stop = Arc::new(AtomicBool::new(false));
        let (ready_tx, ready_rx) = mpsc::sync_channel(1);
        let displays = Arc::clone(&self.displays);
        let thread_stop = Arc::clone(&stop);
        let thread = thread::Builder::new()
            .name("jaycast-brightness-keys".into())
            .spawn(move || run_tap_thread(displays, thread_stop, ready_tx))?;

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
                    message: "brightness key thread exited during startup".into(),
                })
            }
        }
    }

    fn stop_tap(&self) {
        let Some(worker) = self.worker.lock().take() else {
            return;
        };
        worker.stop.store(true, Ordering::Release);
        worker.run_loop.0.stop();
        let _ = worker.thread.join();
    }
}

impl Drop for BrightnessKeyTap {
    fn drop(&mut self) {
        self.stop();
    }
}

fn run_tap_thread(
    displays: Arc<DisplayManager>,
    stop: Arc<AtomicBool>,
    ready: mpsc::SyncSender<Result<RunLoopHandle, CoreError>>,
) {
    // The state lives in a Box so the callback can hold a stable pointer to it; it is only
    // ever touched from this thread, and freed after the run loop has stopped.
    let state = Box::into_raw(Box::new(TapState {
        displays,
        presses: PressTracker::default(),
        port: None,
    }));

    let mask: CGEventMask = 1 << SYSTEM_DEFINED.0;
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
            message: "could not create the brightness key tap's run loop source".into(),
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
    // A panic must never unwind into CoreGraphics or eat the user's key press.
    let disposition = catch_unwind(AssertUnwindSafe(|| state.handle(event_type, event_ref)))
        .unwrap_or(Disposition::PassThrough);
    match disposition {
        Disposition::PassThrough => event.as_ptr(),
        Disposition::Swallow => ptr::null_mut(),
    }
}

/// Everything the tap thread owns.
struct TapState {
    displays: Arc<DisplayManager>,
    presses: PressTracker,
    port: Option<CFRetained<CFMachPort>>,
}

impl TapState {
    fn handle(&mut self, event_type: CGEventType, event: &CGEvent) -> Disposition {
        if event_type == CGEventType::TapDisabledByTimeout
            || event_type == CGEventType::TapDisabledByUserInput
        {
            log::warn!("brightness key tap disabled by the system ({event_type:?}); re-enabling");
            // We may have missed key-ups while disabled, so held presses are stale.
            if let Some(port) = &self.port {
                CGEvent::tap_enable(port, true);
            }
            self.presses.reset();
            return Disposition::PassThrough;
        }
        if event_type != SYSTEM_DEFINED {
            return Disposition::PassThrough;
        }
        let Some((subtype, data1)) = aux_fields(event) else {
            return Disposition::PassThrough;
        };
        let Some(key) = decode(subtype, data1) else {
            return Disposition::PassThrough;
        };

        if !key.pressed {
            return self.presses.up(key.direction);
        }
        let delta = key.direction.sign() * step(CGEvent::flags(Some(event)));
        let displays = &self.displays;
        let enabled = || {
            let config = displays.config();
            config.enabled && config.brightness_keys
        };
        let try_adjust = || match display_under_cursor() {
            Some(id) if !CGDisplayIsBuiltin(id) => displays.adjust(id, delta).is_some(),
            _ => false,
        };
        self.presses
            .down(key.direction, key.repeat, enabled, try_adjust)
    }
}

/// `subtype` and `data1` of a system-defined event, read straight from the event's
/// compound-data fields. CoreGraphics names neither field; `NSEvent` would decode them,
/// but converting a caps-lock event off the main thread trips HIToolbox's main-queue
/// assertion and kills the process.
fn aux_fields(event: &CGEvent) -> Option<(i16, isize)> {
    let subtype = CGEvent::integer_value_field(Some(event), FIELD_SUBTYPE);
    let data1 = CGEvent::integer_value_field(Some(event), FIELD_DATA1);
    Some((i16::try_from(subtype).ok()?, data1 as isize))
}

/// The display containing the mouse cursor. An event created without a source carries the
/// current cursor position, which is more reliable than a keyboard event's own location.
pub(crate) fn display_under_cursor() -> Option<CGDirectDisplayID> {
    let location = CGEvent::location(CGEvent::new(None).as_deref());
    let mut display: CGDirectDisplayID = 0;
    let mut count: u32 = 0;
    // SAFETY: both pointers are valid for one element, matching `max_displays`.
    let error = unsafe { CGGetDisplaysWithPoint(location, 1, &mut display, &mut count) };
    (error == CGError::Success && count > 0).then_some(display)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Disposition {
    PassThrough,
    Swallow,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Direction {
    Up,
    Down,
}

impl Direction {
    fn sign(self) -> f64 {
        match self {
            Direction::Up => 1.0,
            Direction::Down => -1.0,
        }
    }

    fn index(self) -> usize {
        match self {
            Direction::Up => 0,
            Direction::Down => 1,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct BrightnessKey {
    direction: Direction,
    /// Key down (including auto-repeat) rather than key up.
    pressed: bool,
    repeat: bool,
}

/// Decodes an aux-control-button event; `None` for anything that isn't a brightness key.
/// `data1` packs the key type in bits 16–31, the key state in bits 8–15 and the repeat
/// flag in bit 0.
fn decode(subtype: i16, data1: isize) -> Option<BrightnessKey> {
    if subtype != SUBTYPE_AUX_CONTROL_BUTTONS {
        return None;
    }
    let direction = match (data1 & 0xFFFF_0000) >> 16 {
        KEYTYPE_BRIGHTNESS_UP => Direction::Up,
        KEYTYPE_BRIGHTNESS_DOWN => Direction::Down,
        _ => return None,
    };
    let pressed = match (data1 & 0xFF00) >> 8 {
        KEY_STATE_DOWN => true,
        KEY_STATE_UP => false,
        _ => return None,
    };
    Some(BrightnessKey {
        direction,
        pressed,
        repeat: data1 & 0x1 != 0,
    })
}

fn step(flags: CGEventFlags) -> f64 {
    let fine = CGEventFlags::MaskAlternate | CGEventFlags::MaskShift;
    if flags.contains(fine) {
        FINE_STEP
    } else {
        STEP
    }
}

/// Decides per press, not per event: the first key-down settles whether Jaycast owns the
/// press, and its auto-repeats and key-up follow. macOS never sees half a press.
#[derive(Debug, Default)]
struct PressTracker {
    /// Per direction: `Some(true)` while a press Jaycast swallowed is held, `Some(false)`
    /// while one it passed through is held.
    held: [Option<bool>; 2],
}

impl PressTracker {
    /// `enabled` is only consulted when a press starts; `try_adjust` changes the brightness
    /// and reports whether this display is Jaycast's to control.
    fn down(
        &mut self,
        direction: Direction,
        repeat: bool,
        enabled: impl FnOnce() -> bool,
        try_adjust: impl FnOnce() -> bool,
    ) -> Disposition {
        let slot = &mut self.held[direction.index()];
        match (*slot, repeat) {
            // A repeat of a press we own: keep adjusting, even if the cursor has moved to a
            // display we can't control, so macOS doesn't see a repeat without its key-down.
            (Some(true), true) => {
                try_adjust();
                Disposition::Swallow
            }
            (Some(false), true) => Disposition::PassThrough,
            // A new press, or a repeat whose key-down we missed.
            _ => {
                let owned = enabled() && try_adjust();
                *slot = Some(owned);
                if owned {
                    Disposition::Swallow
                } else {
                    Disposition::PassThrough
                }
            }
        }
    }

    fn up(&mut self, direction: Direction) -> Disposition {
        match self.held[direction.index()].take() {
            Some(true) => Disposition::Swallow,
            _ => Disposition::PassThrough,
        }
    }

    fn reset(&mut self) {
        self.held = [None; 2];
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::MonitorConfig;

    fn data1(key_type: isize, state: isize, repeat: bool) -> isize {
        (key_type << 16) | (state << 8) | isize::from(repeat)
    }

    #[test]
    fn decodes_brightness_keys() {
        assert_eq!(
            decode(8, data1(2, 0xA, false)),
            Some(BrightnessKey {
                direction: Direction::Up,
                pressed: true,
                repeat: false
            })
        );
        assert_eq!(
            decode(8, data1(3, 0xB, false)),
            Some(BrightnessKey {
                direction: Direction::Down,
                pressed: false,
                repeat: false
            })
        );
        assert_eq!(decode(8, data1(3, 0xA, true)).map(|k| k.repeat), Some(true));
    }

    #[test]
    fn ignores_other_keys_and_subtypes() {
        // NX_KEYTYPE_SOUND_UP and NX_KEYTYPE_PLAY.
        assert_eq!(decode(8, data1(0, 0xA, false)), None);
        assert_eq!(decode(8, data1(16, 0xA, false)), None);
        // Not an aux-control-button event (e.g. subtype 7, the power key family).
        assert_eq!(decode(7, data1(2, 0xA, false)), None);
        // Unknown key state.
        assert_eq!(decode(8, data1(2, 0x3, false)), None);
    }

    #[test]
    fn option_shift_selects_fine_steps() {
        assert_eq!(step(CGEventFlags::empty()), STEP);
        assert_eq!(step(CGEventFlags::MaskShift), STEP);
        assert_eq!(step(CGEventFlags::MaskAlternate), STEP);
        assert_eq!(
            step(CGEventFlags::MaskAlternate | CGEventFlags::MaskShift),
            FINE_STEP
        );
        assert_eq!(
            step(CGEventFlags::MaskAlternate | CGEventFlags::MaskShift | CGEventFlags::MaskCommand),
            FINE_STEP
        );
    }

    #[test]
    fn owned_press_swallows_down_repeats_and_up() {
        let mut presses = PressTracker::default();
        let mut adjusts = 0;
        let mut adjust = || {
            adjusts += 1;
            true
        };
        assert_eq!(
            presses.down(Direction::Up, false, || true, &mut adjust),
            Disposition::Swallow
        );
        assert_eq!(
            presses.down(Direction::Up, true, || true, &mut adjust),
            Disposition::Swallow
        );
        assert_eq!(presses.up(Direction::Up), Disposition::Swallow);
        assert_eq!(adjusts, 2);
        // The next press starts fresh.
        assert_eq!(presses.up(Direction::Up), Disposition::PassThrough);
    }

    #[test]
    fn uncontrollable_display_passes_the_whole_press_through() {
        let mut presses = PressTracker::default();
        assert_eq!(
            presses.down(Direction::Down, false, || true, || false),
            Disposition::PassThrough
        );
        // The cursor moving onto an external display mid-press doesn't steal the repeats.
        let mut adjusted = false;
        assert_eq!(
            presses.down(
                Direction::Down,
                true,
                || true,
                || {
                    adjusted = true;
                    true
                }
            ),
            Disposition::PassThrough
        );
        assert!(!adjusted);
        assert_eq!(presses.up(Direction::Down), Disposition::PassThrough);
    }

    #[test]
    fn owned_press_keeps_repeats_when_display_becomes_uncontrollable() {
        let mut presses = PressTracker::default();
        presses.down(Direction::Up, false, || true, || true);
        assert_eq!(
            presses.down(Direction::Up, true, || true, || false),
            Disposition::Swallow
        );
        assert_eq!(presses.up(Direction::Up), Disposition::Swallow);
    }

    #[test]
    fn disabled_passes_through_without_adjusting() {
        let mut presses = PressTracker::default();
        let mut adjusted = false;
        assert_eq!(
            presses.down(
                Direction::Up,
                false,
                || false,
                || {
                    adjusted = true;
                    true
                }
            ),
            Disposition::PassThrough
        );
        assert!(!adjusted);
        assert_eq!(presses.up(Direction::Up), Disposition::PassThrough);
    }

    #[test]
    fn repeat_without_seen_down_starts_a_press() {
        let mut presses = PressTracker::default();
        assert_eq!(
            presses.down(Direction::Down, true, || true, || true),
            Disposition::Swallow
        );
        assert_eq!(presses.up(Direction::Down), Disposition::Swallow);
    }

    #[test]
    fn directions_are_tracked_separately() {
        let mut presses = PressTracker::default();
        presses.down(Direction::Up, false, || true, || true);
        presses.down(Direction::Down, false, || true, || false);
        assert_eq!(presses.up(Direction::Down), Disposition::PassThrough);
        assert_eq!(presses.up(Direction::Up), Disposition::Swallow);
    }

    #[test]
    fn reset_forgets_held_presses() {
        let mut presses = PressTracker::default();
        presses.down(Direction::Up, false, || true, || true);
        presses.reset();
        assert_eq!(presses.up(Direction::Up), Disposition::PassThrough);
    }

    /// Round-trips a real system-defined event through CoreGraphics to check that
    /// `aux_fields` recovers `subtype` and `data1`.
    #[test]
    fn aux_fields_reads_a_real_system_defined_event() {
        use objc2::runtime::AnyObject;
        use objc2::{ClassType, msg_send, rc::Retained};
        use objc2_app_kit::NSEvent;
        use objc2_core_foundation::CGPoint;

        let packed = data1(KEYTYPE_BRIGHTNESS_DOWN, KEY_STATE_DOWN, true);
        let context: *const AnyObject = ptr::null();
        let ns_event: Option<Retained<NSEvent>> = unsafe {
            msg_send![
                NSEvent::class(),
                otherEventWithType: 14usize,
                location: CGPoint::new(0.0, 0.0),
                modifierFlags: 0usize,
                timestamp: 0.0f64,
                windowNumber: 0isize,
                context: context,
                subtype: SUBTYPE_AUX_CONTROL_BUTTONS,
                data1: packed,
                data2: -1isize
            ]
        };
        let cg_event = ns_event.unwrap().CGEvent().unwrap();
        let (subtype, data1) = aux_fields(&cg_event).unwrap();
        assert_eq!((subtype, data1), (SUBTYPE_AUX_CONTROL_BUTTONS, packed));
        assert_eq!(
            decode(subtype, data1),
            Some(BrightnessKey {
                direction: Direction::Down,
                pressed: true,
                repeat: true
            })
        );
    }

    #[test]
    fn event_mask_is_system_defined_only() {
        let mask: CGEventMask = 1 << SYSTEM_DEFINED.0;
        assert_eq!(mask, 1 << 14);
    }

    /// Creates a real tap, so it needs Accessibility for the test runner's host app.
    /// Run with `cargo test -p jaycast -- --ignored brightness_tap_starts_and_stops`.
    #[test]
    #[ignore]
    fn brightness_tap_starts_and_stops() {
        let tap = BrightnessKeyTap::new(DisplayManager::new(MonitorConfig::default()));
        match tap.start() {
            Err(CoreError::PermissionDenied) => {
                eprintln!("skipped: not trusted for Accessibility");
                return;
            }
            other => other.unwrap(),
        }
        assert!(tap.is_running());
        tap.start().unwrap();
        thread::sleep(std::time::Duration::from_millis(200));
        tap.stop();
        assert!(!tap.is_running());
        tap.stop();
        tap.start().unwrap();
        drop(tap);
    }
}
