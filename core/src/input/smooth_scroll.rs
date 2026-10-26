//! Animates wheel ticks into continuous, phased scroll events, posted at display rate.
//!
//! [`ScrollPhysics`] decides what to send; this module owns the clock and the emitter thread
//! that posts the events. The thread sleeps on a condition variable whenever nothing is
//! animating, so smooth scrolling costs no CPU at rest.

use std::sync::{Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use objc2_core_graphics::{
    CGDisplayCopyDisplayMode, CGDisplayMode, CGEvent, CGEventField, CGEventFlags,
    CGEventTapLocation, CGMainDisplayID, CGMomentumScrollPhase, CGScrollEventUnit, CGScrollPhase,
};

use super::SYNTHETIC_EVENT_MARKER;
use super::scroll_physics::{Frame, Phase, ScrollPhysics};
use crate::config::SmoothScrollConfig;

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
        self.shared
            .state
            .lock()
            .unwrap()
            .physics
            .set_config(&config);
    }

    /// One wheel tick from a mouse (never a trackpad or our own events). Deltas are in lines,
    /// with macOS's sign convention, after the tap has applied direction and axis modifiers.
    /// Pass the raw tick count (`kCGScrollWheelEventDeltaAxis*`, normally ±1), not
    /// macOS-accelerated values: acceleration is applied here.
    /// `precise` asks for small, unsmoothed, momentum-free steps.
    pub fn feed(&self, vertical: f64, horizontal: f64, precise: bool) {
        let now = clock();
        self.shared
            .state
            .lock()
            .unwrap()
            .physics
            .feed(now, vertical, horizontal, precise);
        self.shared.wake.notify_one();
    }

    /// Cancels any animation in flight, e.g. when the user clicks or input stops.
    pub fn stop(&self) {
        let mut state = self.shared.state.lock().unwrap();
        let frames = state.physics.cancel();
        state.outbox.extend(frames);
        drop(state);
        self.shared.wake.notify_one();
    }
}

impl Drop for SmoothScroller {
    fn drop(&mut self) {
        self.stop();
        self.shared.state.lock().unwrap().shutdown = true;
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
        let mut state = shared.state.lock().unwrap();
        while state.physics.is_idle() && state.outbox.is_empty() && !state.shutdown {
            state = shared.wake.wait(state).unwrap();
        }
        // Flush closing events before honouring shutdown, so no gesture is left open.
        if state.shutdown && state.outbox.is_empty() {
            return;
        }
        drop(state);

        // Animate until idle, then go back to sleep. The refresh rate is re-read per burst
        // so moving between a 60 Hz and a 120 Hz display is picked up.
        let interval = Duration::from_secs_f64(1.0 / refresh_rate());
        let mut deadline = Instant::now();
        loop {
            let mut state = shared.state.lock().unwrap();
            let mut frames = std::mem::take(&mut state.outbox);
            frames.extend(state.physics.tick(clock()));
            let idle = state.physics.is_idle();
            drop(state);

            for frame in &frames {
                post(frame);
            }
            if idle {
                break;
            }

            deadline += interval;
            let now = Instant::now();
            if deadline <= now {
                // Fell behind (system busy): resync rather than bursting to catch up.
                deadline = now + interval;
            }
            thread::sleep(deadline - now);
        }
    }
}

fn refresh_rate() -> f64 {
    let mode = CGDisplayCopyDisplayMode(CGMainDisplayID());
    let hz = CGDisplayMode::refresh_rate(mode.as_deref());
    // Some panels (including built-in ProMotion displays) report 0.
    if (30.0..=240.0).contains(&hz) {
        hz
    } else {
        FALLBACK_REFRESH_HZ
    }
}

/// Builds the CGEvent for one frame: a continuous (trackpad-style) pixel scroll.
fn make_event(frame: &Frame) -> Option<objc2_core_foundation::CFRetained<CGEvent>> {
    // Axis 1 is vertical, axis 2 horizontal.
    let event =
        CGEvent::new_scroll_wheel_event2(None, CGScrollEventUnit::Pixel, 2, frame.dy, frame.dx, 0)?;
    let e = Some(&*event);
    let set = |field, value: i64| CGEvent::set_integer_value_field(e, field, value);

    // Pixel-unit creation already fills every delta field consistently (point = px,
    // fixed-point = px / 10 lines, line = rounded away from zero) and marks the event
    // continuous; the flag is set again so that doesn't rest on undocumented behaviour.
    set(CGEventField::ScrollWheelEventIsContinuous, 1);

    let (scroll, momentum) = phase_fields(frame.phase);
    set(CGEventField::ScrollWheelEventScrollPhase, scroll.0 as i64);
    set(
        CGEventField::ScrollWheelEventMomentumPhase,
        momentum.0 as i64,
    );
    set(CGEventField::EventSourceUserData, SYNTHETIC_EVENT_MARKER);
    // No modifiers: the tap has already applied the user's horizontal/precise modifier, and
    // a held Shift would make AppKit swap the axes a second time.
    CGEvent::set_flags(e, CGEventFlags(0));
    Some(event)
}

fn post(frame: &Frame) {
    if let Some(event) = make_event(frame) {
        CGEvent::post(CGEventTapLocation::SessionEventTap, Some(&event));
    }
}

/// Gesture events carry no momentum phase and momentum events no gesture phase.
fn phase_fields(phase: Phase) -> (CGScrollPhase, CGMomentumScrollPhase) {
    let none = CGScrollPhase(0);
    match phase {
        Phase::Began => (CGScrollPhase::Began, CGMomentumScrollPhase::None),
        Phase::Changed => (CGScrollPhase::Changed, CGMomentumScrollPhase::None),
        Phase::Ended => (CGScrollPhase::Ended, CGMomentumScrollPhase::None),
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
            phase: Phase::MomentumChanged,
        };
        let event = make_event(&frame).expect("event");
        let e = Some(&*event);
        let get = |field| CGEvent::integer_value_field(e, field);

        assert_eq!(get(CGEventField::ScrollWheelEventIsContinuous), 1);
        assert_eq!(get(CGEventField::ScrollWheelEventPointDeltaAxis1), 17);
        assert_eq!(get(CGEventField::ScrollWheelEventPointDeltaAxis2), -3);
        let fixed = CGEvent::double_value_field(e, CGEventField::ScrollWheelEventFixedPtDeltaAxis1);
        assert!(
            (fixed - 1.7).abs() < 0.01,
            "fixed-point delta is in lines: {fixed}"
        );
        assert_eq!(get(CGEventField::ScrollWheelEventScrollPhase), 0);
        assert_eq!(get(CGEventField::ScrollWheelEventMomentumPhase), 2);
        assert_eq!(
            get(CGEventField::EventSourceUserData),
            SYNTHETIC_EVENT_MARKER
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
