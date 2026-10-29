//! Animates wheel ticks into continuous scroll events, posted at display rate.
//!
//! [`ScrollPhysics`] decides what to send; this module owns the clock and the emitter thread
//! that posts the events. The thread sleeps on a condition variable whenever nothing is
//! animating, so smooth scrolling costs no CPU at rest.

use parking_lot::{Condvar, Mutex};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use objc2_core_graphics::{
    CGDirectDisplayID, CGDisplayCopyDisplayMode, CGDisplayMode, CGDisplayPixelsHigh,
    CGDisplayPixelsWide, CGEvent, CGEventField, CGEventFlags, CGEventTapLocation, CGMainDisplayID,
    CGMomentumScrollPhase, CGScrollEventUnit, CGScrollPhase,
};

use super::SYNTHETIC_EVENT_MARKER;
use super::scroll_physics::{Frame, Phase, ScrollPhysics};
use crate::config::SmoothScrollConfig;
use crate::display::keys::display_under_cursor;

const FALLBACK_REFRESH_HZ: f64 = 60.0;

/// Owns its own emitter thread. All methods are cheap and safe to call from the tap thread.
pub(crate) struct SmoothScroller {
    shared: Arc<Shared>,
    thread: Option<JoinHandle<()>>,
}

struct Shared {
    state: Mutex<State>,
    wake: Condvar,
}

struct State {
    physics: ScrollPhysics,
    /// Closing events from `stop()`, posted by the emitter so the tap thread never posts.
    outbox: Vec<Frame>,
    shutdown: bool,
}

impl SmoothScroller {
    pub fn new(config: SmoothScrollConfig) -> Self {
        let shared = Arc::new(Shared {
            state: Mutex::new(State {
                physics: ScrollPhysics::new(&config),
                outbox: Vec::new(),
                shutdown: false,
            }),
            wake: Condvar::new(),
        });
        let thread = thread::Builder::new()
            .name("jaycast-smooth-scroll".into())
            .spawn({
                let shared = Arc::clone(&shared);
                move || run_emitter(&shared)
            })
            .expect("failed to spawn the smooth scroll thread");
        Self {
            shared,
            thread: Some(thread),
        }
    }

    pub fn set_config(&self, config: SmoothScrollConfig) {
        self.shared.state.lock().physics.set_config(&config);
    }

    /// One wheel tick from a mouse (never a trackpad or our own events). Deltas are in lines,
    /// with macOS's sign convention, after the tap has applied direction and axis modifiers.
    /// Pass the raw tick count (`kCGScrollWheelEventDeltaAxis*`, normally ±1), not
    /// macOS-accelerated values: acceleration is applied here.
    /// `precise` asks for small, unsmoothed steps.
    pub fn feed(&self, vertical: f64, horizontal: f64, precise: bool) {
        let now = clock();
        let display = pointer_display();
        let (width, height) = (CGDisplayPixelsWide(display), CGDisplayPixelsHigh(display));
        let mut state = self.shared.state.lock();
        state.physics.set_screen(width as f64, height as f64);
        state.physics.feed(now, vertical, horizontal, precise);
        drop(state);
        self.shared.wake.notify_one();
    }

    /// Cancels any animation in flight, e.g. when the user clicks or input stops.
    pub fn stop(&self) {
        let mut state = self.shared.state.lock();
        let frames = state.physics.cancel();
        state.outbox.extend(frames);
        drop(state);
        self.shared.wake.notify_one();
    }
}

impl Drop for SmoothScroller {
    fn drop(&mut self) {
        self.stop();
        self.shared.state.lock().shutdown = true;
        self.shared.wake.notify_one();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// Seconds since an arbitrary process-wide epoch: the physics only needs differences.
fn clock() -> f64 {
    static EPOCH: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
    EPOCH.get_or_init(Instant::now).elapsed().as_secs_f64()
}

fn run_emitter(shared: &Shared) {
    loop {
        let mut state = shared.state.lock();
        while state.physics.is_idle() && state.outbox.is_empty() && !state.shutdown {
            shared.wake.wait(&mut state);
        }
        // Flush closing events before honouring shutdown, so no gesture is left open.
        if state.shutdown && state.outbox.is_empty() {
            return;
        }
        drop(state);

        // Animate until idle, then go back to sleep. The refresh rate is re-read per burst,
        // from the display being scrolled, so mixed 60 Hz and 120 Hz setups both stay smooth.
        let hz = refresh_rate(pointer_display());
        let interval = Duration::from_secs_f64(1.0 / hz);
        let mut lines = LineCarry::default();
        let started = Instant::now();
        let mut deadline = started;
        let mut posted = 0;
        let mut resyncs = 0;
        log::debug!("scroll burst started at {hz} Hz");
        loop {
            let mut state = shared.state.lock();
            let mut frames = std::mem::take(&mut state.outbox);
            frames.extend(state.physics.tick(clock()));
            let idle = state.physics.is_idle();
            drop(state);

            for frame in &frames {
                post(frame, &mut lines);
            }
            posted += frames.len();
            if idle {
                log::debug!(
                    "scroll burst ended: {posted} frames in {:?}, {resyncs} resyncs",
                    started.elapsed()
                );
                break;
            }

            deadline += interval;
            let now = Instant::now();
            if deadline <= now {
                // Fell behind (system busy): resync rather than bursting to catch up.
                deadline = now + interval;
                resyncs += 1;
            }
            thread::sleep(deadline - now);
        }
    }
}

/// The display under the pointer, which is the one being scrolled.
fn pointer_display() -> CGDirectDisplayID {
    display_under_cursor().unwrap_or_else(|| CGMainDisplayID())
}

fn refresh_rate(display: CGDirectDisplayID) -> f64 {
    let mode = CGDisplayCopyDisplayMode(display);
    let hz = CGDisplayMode::refresh_rate(mode.as_deref());
    // Some panels (including built-in ProMotion displays) report 0.
    if (30.0..=240.0).contains(&hz) {
        hz
    } else {
        FALLBACK_REFRESH_HZ
    }
}

/// Pixels per line, as macOS converts between the two for continuous scrolling.
const PX_PER_LINE: i64 = 10;

/// Pixels not yet reported as whole lines, so the integer line deltas add up to the pixels
/// actually scrolled instead of claiming a whole line for every few-pixel frame.
#[derive(Debug, Default)]
struct LineCarry {
    x: i64,
    y: i64,
}

impl LineCarry {
    fn take(&mut self, dx: i32, dy: i32) -> (i64, i64) {
        self.x += dx as i64;
        self.y += dy as i64;
        let whole = (self.x / PX_PER_LINE, self.y / PX_PER_LINE);
        self.x %= PX_PER_LINE;
        self.y %= PX_PER_LINE;
        whole
    }
}

// Undocumented event fields and values, as real trackpad events carry them.
const FIELD_EVENT_TYPE: CGEventField = CGEventField(55);
const FIELD_GESTURE_SUBTYPE: CGEventField = CGEventField(110);
const FIELD_GESTURE_SCROLL_X: CGEventField = CGEventField(116);
const FIELD_GESTURE_SCROLL_Y: CGEventField = CGEventField(119);
const FIELD_GESTURE_PHASE: CGEventField = CGEventField(132);
/// `NSEvent.isDirectionInvertedFromDevice`: which way swipe actions (e.g. Mail's swipe to
/// mark unread) follow the fingers.
const FIELD_INVERTED_FROM_DEVICE: CGEventField = CGEventField(137);
/// `NSEventTypeGesture`, with the `kIOHIDEventTypeScroll` subtype.
const GESTURE_EVENT_TYPE: i64 = 29;
const GESTURE_SUBTYPE_SCROLL: i64 = 6;
/// Gesture deltas relative to the scroll's pixels: higher makes swiping between pages in
/// Safari and Preview easier to trigger.
const GESTURE_DELTA_SCALE: f64 = 1.6;

/// Builds the scroll event for one frame: a continuous (trackpad-style) pixel scroll.
fn make_event(
    frame: &Frame,
    lines: &mut LineCarry,
) -> Option<objc2_core_foundation::CFRetained<CGEvent>> {
    // Axis 1 is vertical, axis 2 horizontal.
    let event =
        CGEvent::new_scroll_wheel_event2(None, CGScrollEventUnit::Pixel, 2, frame.dy, frame.dx, 0)?;
    let e = Some(&*event);
    let set = |field, value: i64| CGEvent::set_integer_value_field(e, field, value);

    if matches!(frame.phase, Phase::Began | Phase::MomentumBegan) {
        *lines = LineCarry::default();
    }
    let (line_x, line_y) = lines.take(frame.dx, frame.dy);
    // Lines first: setting them can rewrite the other deltas, so those are set after.
    set(CGEventField::ScrollWheelEventDeltaAxis1, line_y);
    set(CGEventField::ScrollWheelEventDeltaAxis2, line_x);
    set(
        CGEventField::ScrollWheelEventPointDeltaAxis1,
        frame.dy as i64,
    );
    set(
        CGEventField::ScrollWheelEventPointDeltaAxis2,
        frame.dx as i64,
    );
    CGEvent::set_double_value_field(
        e,
        CGEventField::ScrollWheelEventFixedPtDeltaAxis1,
        frame.dy as f64 / PX_PER_LINE as f64,
    );
    CGEvent::set_double_value_field(
        e,
        CGEventField::ScrollWheelEventFixedPtDeltaAxis2,
        frame.dx as f64 / PX_PER_LINE as f64,
    );
    set(CGEventField::ScrollWheelEventIsContinuous, 1);

    let (scroll, momentum) = phase_fields(frame.phase);
    set(CGEventField::ScrollWheelEventScrollPhase, scroll.0 as i64);
    set(
        CGEventField::ScrollWheelEventMomentumPhase,
        momentum.0 as i64,
    );
    if frame.phase != Phase::None {
        set(FIELD_INVERTED_FROM_DEVICE, 1);
    }
    set(CGEventField::EventSourceUserData, SYNTHETIC_EVENT_MARKER);
    // No modifiers: the tap has already applied the user's horizontal/precise modifier, and
    // a held Shift would make AppKit swap the axes a second time.
    CGEvent::set_flags(e, CGEventFlags(0));
    Some(event)
}

/// The gesture event a trackpad sends alongside each scroll event while fingers are down.
/// It is what drives swipe navigation (Safari, Preview) and swipe actions (Mail).
fn make_gesture_event(frame: &Frame) -> Option<objc2_core_foundation::CFRetained<CGEvent>> {
    let (scroll, _) = phase_fields(frame.phase);
    if scroll.0 == 0 {
        return None;
    }
    let event = CGEvent::new(None)?;
    let e = Some(&*event);
    let set = |field, value: i64| CGEvent::set_integer_value_field(e, field, value);
    set(FIELD_EVENT_TYPE, GESTURE_EVENT_TYPE);
    set(FIELD_GESTURE_SUBTYPE, GESTURE_SUBTYPE_SCROLL);
    CGEvent::set_double_value_field(
        e,
        FIELD_GESTURE_SCROLL_X,
        frame.dx as f64 * GESTURE_DELTA_SCALE,
    );
    CGEvent::set_double_value_field(
        e,
        FIELD_GESTURE_SCROLL_Y,
        frame.dy as f64 * GESTURE_DELTA_SCALE,
    );
    set(FIELD_GESTURE_PHASE, scroll.0 as i64);
    set(CGEventField::EventSourceUserData, SYNTHETIC_EVENT_MARKER);
    CGEvent::set_flags(e, CGEventFlags(0));
    Some(event)
}

fn post(frame: &Frame, lines: &mut LineCarry) {
    let Some(event) = make_event(frame, lines) else {
        return;
    };
    // The gesture goes first, at the same timestamp: the other order makes page swipes
    // stutter.
    if let Some(gesture) = make_gesture_event(frame) {
        CGEvent::set_timestamp(Some(&gesture), CGEvent::timestamp(Some(&event)));
        CGEvent::post(CGEventTapLocation::SessionEventTap, Some(&gesture));
    }
    CGEvent::post(CGEventTapLocation::SessionEventTap, Some(&event));
}

fn phase_fields(phase: Phase) -> (CGScrollPhase, CGMomentumScrollPhase) {
    let none = CGScrollPhase(0);
    match phase {
        Phase::None => (none, CGMomentumScrollPhase::None),
        Phase::Began => (CGScrollPhase::Began, CGMomentumScrollPhase::None),
        Phase::Changed => (CGScrollPhase::Changed, CGMomentumScrollPhase::None),
        Phase::Ended => (CGScrollPhase::Ended, CGMomentumScrollPhase::None),
        Phase::MayBegin => (CGScrollPhase::MayBegin, CGMomentumScrollPhase::None),
        Phase::Cancelled => (CGScrollPhase::Cancelled, CGMomentumScrollPhase::None),
        Phase::MomentumBegan => (none, CGMomentumScrollPhase::Begin),
        Phase::MomentumChanged => (none, CGMomentumScrollPhase::Continue),
        Phase::MomentumEnded => (none, CGMomentumScrollPhase::End),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn events_carry_deltas_phases_and_marker() {
        let frame = Frame {
            dx: -3,
            dy: 17,
            phase: Phase::Changed,
        };
        let mut lines = LineCarry::default();
        let event = make_event(&frame, &mut lines).expect("event");
        let e = Some(&*event);
        let get = |field| CGEvent::integer_value_field(e, field);

        assert_eq!(get(CGEventField::ScrollWheelEventIsContinuous), 1);
        assert_eq!(get(CGEventField::ScrollWheelEventPointDeltaAxis1), 17);
        assert_eq!(get(CGEventField::ScrollWheelEventPointDeltaAxis2), -3);
        assert_eq!(get(CGEventField::ScrollWheelEventDeltaAxis1), 1);
        assert_eq!(get(CGEventField::ScrollWheelEventDeltaAxis2), 0);
        let fixed = CGEvent::double_value_field(e, CGEventField::ScrollWheelEventFixedPtDeltaAxis1);
        assert!(
            (fixed - 1.7).abs() < 0.01,
            "fixed-point delta is in lines: {fixed}"
        );
        assert_eq!(get(CGEventField::ScrollWheelEventScrollPhase), 2);
        assert_eq!(get(CGEventField::ScrollWheelEventMomentumPhase), 0);
        assert_eq!(get(FIELD_INVERTED_FROM_DEVICE), 1);
        assert_eq!(
            get(CGEventField::EventSourceUserData),
            SYNTHETIC_EVENT_MARKER
        );

        let gesture = make_gesture_event(&frame).expect("gesture");
        let g = Some(&*gesture);
        assert_eq!(CGEvent::integer_value_field(g, FIELD_EVENT_TYPE), 29);
        assert_eq!(CGEvent::integer_value_field(g, FIELD_GESTURE_PHASE), 2);
        let gesture_dy = CGEvent::double_value_field(g, FIELD_GESTURE_SCROLL_Y);
        assert!((gesture_dy - 17.0 * GESTURE_DELTA_SCALE).abs() < 0.01);
    }

    #[test]
    fn line_deltas_add_up_across_small_frames() {
        let mut lines = LineCarry::default();
        let total: i64 = (0..10).map(|_| lines.take(0, 3).1).sum();
        assert_eq!(total, 3, "30 px is 3 lines, not one per frame");
    }

    #[test]
    fn momentum_and_unphased_events_have_no_gesture() {
        for phase in [Phase::None, Phase::MomentumChanged] {
            let frame = Frame {
                dx: 0,
                dy: 5,
                phase,
            };
            assert!(make_gesture_event(&frame).is_none());
        }
        let unphased = Frame {
            dx: 0,
            dy: 5,
            phase: Phase::None,
        };
        let event = make_event(&unphased, &mut LineCarry::default()).expect("event");
        let e = Some(&*event);
        assert_eq!(
            CGEvent::integer_value_field(e, CGEventField::ScrollWheelEventScrollPhase),
            0
        );
        assert_eq!(
            CGEvent::integer_value_field(e, FIELD_INVERTED_FROM_DEVICE),
            0
        );
    }

    #[test]
    fn idle_scroller_starts_and_shuts_down_cleanly() {
        let scroller = SmoothScroller::new(SmoothScrollConfig::default());
        scroller.set_config(SmoothScrollConfig::default());
        scroller.stop();
        drop(scroller);
    }
}
