//! Turns discrete wheel ticks into a trackpad-like stream of pixel deltas.
//!
//! Pure: no CoreGraphics and no clock. Callers pass `now` in seconds from any fixed epoch,
//! which keeps the curves deterministic and testable.
//!
//! Model: each tick adds distance to a per-axis `pending` amount, and every frame delivers a
//! fraction of it (exponential ease-out with time constant `glide`), so ticks blend smoothly.
//! Once the wheel has been still for [`LIFT_DELAY`] the gesture "lifts", like a finger leaving
//! a trackpad. If it was moving fast enough and momentum is on, it hands its velocity to a
//! slower-decaying momentum phase; otherwise it finishes delivering what's pending and ends.

use crate::config::{SmoothScrollConfig, Smoothness};

/// Pixels scrolled per wheel line at speed 1.0, before acceleration. Roughly three lines
/// of body text, close to what one notch scrolls in Safari with smooth scrolling off.
const PX_PER_LINE: f64 = 50.0;
/// Pixels per line in precise mode: fine positioning, independent of `speed`.
const PRECISE_PX_PER_LINE: f64 = 4.0;

/// Ticks arriving closer together than this, in the same direction, build an acceleration streak.
const ACCEL_WINDOW: f64 = 0.08;
/// Streak length before acceleration starts, so a deliberate double notch stays 1:1.
const ACCEL_GRACE: u32 = 2;
/// Extra distance multiplier per tick beyond the grace period.
const ACCEL_PER_TICK: f64 = 0.25;
const ACCEL_MAX: f64 = 4.0;

/// Wheel stillness after which the gesture counts as lifted.
const LIFT_DELAY: f64 = 0.06;
/// Minimum speed (px/s) at lift for momentum to start; one slow notch never glides.
const MOMENTUM_MIN_SPEED: f64 = 800.0;
/// Momentum ends once it slows below this (px/s).
const MOMENTUM_STOP_SPEED: f64 = 15.0;
/// Pending distance below which a gesture counts as fully delivered.
const DRAINED_PX: f64 = 0.5;
/// Longest step simulated in one frame, so a stalled emitter doesn't lurch.
const MAX_FRAME_DT: f64 = 0.05;

/// macOS scroll phases. Gesture and momentum phases go in separate event fields.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Phase {
    Began,
    Changed,
    Ended,
    MomentumBegan,
    MomentumChanged,
    MomentumEnded,
}

/// One scroll event to post: whole pixels, macOS sign convention (positive = up / left).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Frame {
    pub dx: i32,
    pub dy: i32,
    pub phase: Phase,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct Params {
    px_per_line: f64,
    /// Time constant (s) of the per-tick ease-out; higher is smoother and floatier.
    glide: f64,
    /// Time constant (s) of momentum friction.
    friction: f64,
    acceleration: bool,
    momentum: bool,
}

impl Params {
    fn new(config: &SmoothScrollConfig) -> Self {
        let (glide, friction) = match config.smoothness {
            Smoothness::Low => (0.04, 0.25),
            Smoothness::Regular => (0.075, 0.4),
            Smoothness::High => (0.13, 0.6),
        };
        Self {
            px_per_line: PX_PER_LINE * config.speed.clamp(0.1, 10.0),
            glide,
            friction,
            acceleration: config.acceleration,
            momentum: config.momentum,
        }
    }
}

#[derive(Debug, Clone, Copy, Default)]
struct Axis {
    /// Distance still to deliver in the gesture phase.
    pending: f64,
    /// Momentum velocity (px/s).
    velocity: f64,
    /// Sub-pixel remainder not yet emitted.
    carry: f64,
    /// Sign of the last tick on this axis, to detect reversals.
    last_sign: f64,
}

impl Axis {
    /// Moves `distance` into the carry and takes out the whole pixels.
    fn emit(&mut self, distance: f64) -> i32 {
        self.carry += distance;
        let whole = self.carry.trunc();
        self.carry -= whole;
        whole as i32
    }

    /// Emits everything left, rounding the last fraction, so a gesture's total is exact.
    fn flush(&mut self) -> i32 {
        let whole = (self.carry + self.pending).round();
        self.carry = 0.0;
        self.pending = 0.0;
        whole as i32
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    Idle,
    Gesture { began: bool },
    Momentum { began: bool },
}

pub(crate) struct ScrollPhysics {
    params: Params,
    state: State,
    x: Axis,
    y: Axis,
    last_tick_at: f64,
    last_frame_at: Option<f64>,
    /// Direction of the last tick as (horizontal?, sign), for acceleration streaks.
    last_direction: Option<(bool, f64)>,
    streak: u32,
    /// Deliver all pending distance on the next frame (precise mode).
    snap: bool,
    /// A tick interrupted momentum; the next frame must close it before beginning anew.
    end_momentum: bool,
}

impl ScrollPhysics {
    pub fn new(config: &SmoothScrollConfig) -> Self {
        Self {
            params: Params::new(config),
            state: State::Idle,
            x: Axis::default(),
            y: Axis::default(),
            last_tick_at: 0.0,
            last_frame_at: None,
            last_direction: None,
            streak: 0,
            snap: false,
            end_momentum: false,
        }
    }

    /// Takes effect from the next tick and frame; an animation in flight keeps going.
    pub fn set_config(&mut self, config: &SmoothScrollConfig) {
        self.params = Params::new(config);
    }

    pub fn is_idle(&self) -> bool {
        self.state == State::Idle
    }

    /// Adds one wheel tick, in lines with macOS's sign convention.
    pub fn feed(&mut self, now: f64, vertical: f64, horizontal: f64, precise: bool) {
        if vertical == 0.0 && horizontal == 0.0 {
            return;
        }

        if let State::Momentum { began } = self.state {
            // A new tick catches the glide, like a finger landing on a coasting trackpad.
            // Same-direction ticks keep its speed by turning velocity back into pending distance.
            self.end_momentum = began;
            for axis in [&mut self.x, &mut self.y] {
                axis.pending = axis.velocity * self.params.glide;
                axis.velocity = 0.0;
            }
        }
        if self.state == State::Idle {
            self.last_frame_at = Some(now);
        }
        if !matches!(self.state, State::Gesture { .. }) {
            self.state = State::Gesture { began: false };
        }

        let direction = if vertical.abs() >= horizontal.abs() {
            (false, vertical.signum())
        } else {
            (true, horizontal.signum())
        };
        let quick = now - self.last_tick_at < ACCEL_WINDOW;
        self.streak = if quick && self.last_direction == Some(direction) {
            self.streak + 1
        } else {
            0
        };
        self.last_direction = Some(direction);
        self.last_tick_at = now;

        if precise {
            for axis in [&mut self.x, &mut self.y] {
                axis.pending = 0.0;
            }
            self.streak = 0;
            self.snap = true;
        }

        let per_line = if precise {
            PRECISE_PX_PER_LINE
        } else {
            self.params.px_per_line * self.acceleration()
        };
        for (axis, lines) in [(&mut self.x, horizontal), (&mut self.y, vertical)] {
            if lines == 0.0 {
                continue;
            }
            let sign = lines.signum();
            if axis.last_sign != 0.0 && sign != axis.last_sign {
                // Reversing stops the current motion dead rather than fighting it.
                axis.pending = 0.0;
                axis.carry = 0.0;
                self.streak = 0;
            }
            axis.last_sign = sign;
            axis.pending += lines * per_line;
        }
    }

    fn acceleration(&self) -> f64 {
        if !self.params.acceleration {
            return 1.0;
        }
        let extra = self.streak.saturating_sub(ACCEL_GRACE) as f64;
        (1.0 + ACCEL_PER_TICK * extra).min(ACCEL_MAX)
    }

    /// Advances to `now` and returns the events to post: usually one, sometimes two at a
    /// phase boundary, none when idle or when nothing reached a whole pixel.
    pub fn tick(&mut self, now: f64) -> Vec<Frame> {
        let dt = self
            .last_frame_at
            .map_or(0.0, |last| (now - last).clamp(0.0, MAX_FRAME_DT));
        self.last_frame_at = Some(now);
        let mut frames = Vec::new();

        if self.end_momentum {
            self.end_momentum = false;
            frames.push(frame(0, 0, Phase::MomentumEnded));
        }

        match self.state {
            State::Idle => {}
            State::Gesture { began } => self.tick_gesture(now, dt, began, &mut frames),
            State::Momentum { began } => self.tick_momentum(dt, began, &mut frames),
        }
        frames
    }

    fn tick_gesture(&mut self, now: f64, dt: f64, began: bool, frames: &mut Vec<Frame>) {
        let fraction = if self.snap {
            1.0
        } else {
            1.0 - (-dt / self.params.glide).exp()
        };
        self.snap = false;
        let dx = take(&mut self.x, fraction);
        let dy = take(&mut self.y, fraction);
        let moving_phase = if began { Phase::Changed } else { Phase::Began };

        let lifted = now - self.last_tick_at >= LIFT_DELAY;
        let speed = self.x.pending.hypot(self.y.pending) / self.params.glide;
        let drained = self.x.pending.abs() < DRAINED_PX && self.y.pending.abs() < DRAINED_PX;

        if lifted && self.params.momentum && speed >= MOMENTUM_MIN_SPEED {
            frames.push(frame(dx, dy, moving_phase));
            frames.push(frame(0, 0, Phase::Ended));
            for axis in [&mut self.x, &mut self.y] {
                axis.velocity = axis.pending / self.params.glide;
                axis.pending = 0.0;
            }
            self.state = State::Momentum { began: false };
        } else if lifted && drained {
            let (fx, fy) = (dx + self.x.flush(), dy + self.y.flush());
            if fx != 0 || fy != 0 || !began {
                frames.push(frame(fx, fy, moving_phase));
            }
            frames.push(frame(0, 0, Phase::Ended));
            self.reset();
        } else if dx != 0 || dy != 0 || !began {
            frames.push(frame(dx, dy, moving_phase));
            self.state = State::Gesture { began: true };
        }
    }

    fn tick_momentum(&mut self, dt: f64, began: bool, frames: &mut Vec<Frame>) {
        // Exact integral of v·e^(−t/τ) over the frame, so frame rate doesn't change distance.
        let decay = (-dt / self.params.friction).exp();
        let travel = self.params.friction * (1.0 - decay);
        let mut deltas = [0; 2];
        for (delta, axis) in deltas.iter_mut().zip([&mut self.x, &mut self.y]) {
            *delta = axis.emit(axis.velocity * travel);
            axis.velocity *= decay;
        }
        let [dx, dy] = deltas;

        if self.x.velocity.hypot(self.y.velocity) < MOMENTUM_STOP_SPEED {
            if dx != 0 || dy != 0 {
                frames.push(frame(dx, dy, momentum_phase(began)));
            }
            frames.push(frame(0, 0, Phase::MomentumEnded));
            self.reset();
        } else if dx != 0 || dy != 0 || !began {
            frames.push(frame(dx, dy, momentum_phase(began)));
            self.state = State::Momentum { began: true };
        }
    }

    /// Stops immediately, returning the event that closes whatever phase was open.
    pub fn cancel(&mut self) -> Vec<Frame> {
        let mut frames = Vec::new();
        if self.end_momentum {
            frames.push(frame(0, 0, Phase::MomentumEnded));
        }
        match self.state {
            State::Gesture { began: true } => frames.push(frame(0, 0, Phase::Ended)),
            State::Momentum { began: true } => frames.push(frame(0, 0, Phase::MomentumEnded)),
            _ => {}
        }
        self.reset();
        frames
    }

    fn reset(&mut self) {
        self.state = State::Idle;
        self.x = Axis::default();
        self.y = Axis::default();
        self.last_direction = None;
        self.streak = 0;
        self.snap = false;
        self.end_momentum = false;
    }
}

/// Delivers `fraction` of the axis's pending distance and returns the whole pixels to emit.
fn take(axis: &mut Axis, fraction: f64) -> i32 {
    let distance = axis.pending * fraction;
    axis.pending -= distance;
    axis.emit(distance)
}

fn momentum_phase(began: bool) -> Phase {
    if began {
        Phase::MomentumChanged
    } else {
        Phase::MomentumBegan
    }
}

fn frame(dx: i32, dy: i32, phase: Phase) -> Frame {
    Frame { dx, dy, phase }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FRAME: f64 = 1.0 / 120.0;

    fn config(momentum: bool, acceleration: bool) -> SmoothScrollConfig {
        SmoothScrollConfig {
            momentum,
            acceleration,
            ..SmoothScrollConfig::default()
        }
    }

    /// Ticks at the given times (vertical lines each), then runs frames until idle.
    fn run(physics: &mut ScrollPhysics, ticks: &[(f64, f64)], precise: bool) -> Vec<Frame> {
        let mut frames = Vec::new();
        let mut ticks = ticks.iter().peekable();
        let mut now = 0.0;
        for _ in 0..10_000 {
            while let Some(&&(at, lines)) = ticks.peek() {
                if at > now {
                    break;
                }
                physics.feed(at, lines, 0.0, precise);
                ticks.next();
            }
            frames.extend(physics.tick(now));
            if physics.is_idle() && ticks.peek().is_none() {
                return frames;
            }
            now += FRAME;
        }
        panic!("animation never finished");
    }

    fn total_dy(frames: &[Frame]) -> i64 {
        frames.iter().map(|f| f.dy as i64).sum()
    }

    fn phases(frames: &[Frame]) -> Vec<Phase> {
        let mut phases: Vec<Phase> = frames.iter().map(|f| f.phase).collect();
        phases.dedup();
        phases
    }

    #[test]
    fn one_tick_delivers_exactly_its_distance() {
        let mut physics = ScrollPhysics::new(&config(true, true));
        let frames = run(&mut physics, &[(0.0, 1.0)], false);
        assert_eq!(total_dy(&frames), PX_PER_LINE as i64);
        assert!(frames.len() > 5, "should be spread over several frames");
    }

    #[test]
    fn distance_is_conserved_at_fractional_speeds_and_downward() {
        let mut cfg = config(false, false);
        cfg.speed = 1.37;
        let mut physics = ScrollPhysics::new(&cfg);
        let frames = run(
            &mut physics,
            &[(0.0, -1.0), (0.03, -1.0), (0.2, -1.0)],
            false,
        );
        let expected = (-3.0 * PX_PER_LINE * 1.37).round() as i64;
        assert!(
            (total_dy(&frames) - expected).abs() <= 1,
            "{} vs {expected}",
            total_dy(&frames)
        );
    }

    #[test]
    fn deltas_ease_out() {
        let mut physics = ScrollPhysics::new(&config(false, false));
        let frames = run(&mut physics, &[(0.0, 1.0)], false);
        let moving: Vec<i32> = frames.iter().map(|f| f.dy).filter(|&d| d != 0).collect();
        assert!(moving.first() > moving.last());
    }

    #[test]
    fn phases_run_began_changed_ended() {
        let mut physics = ScrollPhysics::new(&config(false, false));
        let frames = run(&mut physics, &[(0.0, 1.0)], false);
        assert_eq!(
            phases(&frames),
            [Phase::Began, Phase::Changed, Phase::Ended]
        );
        assert_eq!(frames.last().unwrap().dy, 0, "Ended carries no delta");
    }

    #[test]
    fn quick_ticks_accelerate() {
        let quick: Vec<(f64, f64)> = (0..10).map(|i| (i as f64 * 0.02, 1.0)).collect();
        let slow: Vec<(f64, f64)> = (0..10).map(|i| (i as f64 * 0.5, 1.0)).collect();

        let mut physics = ScrollPhysics::new(&config(false, true));
        let fast_total = total_dy(&run(&mut physics, &quick, false));
        let mut physics = ScrollPhysics::new(&config(false, true));
        let slow_total = total_dy(&run(&mut physics, &slow, false));

        assert_eq!(slow_total, 10 * PX_PER_LINE as i64);
        assert!(
            fast_total > slow_total * 3 / 2,
            "{fast_total} vs {slow_total}"
        );
    }

    #[test]
    fn acceleration_off_is_linear() {
        let quick: Vec<(f64, f64)> = (0..10).map(|i| (i as f64 * 0.02, 1.0)).collect();
        let mut physics = ScrollPhysics::new(&config(false, false));
        assert_eq!(
            total_dy(&run(&mut physics, &quick, false)),
            10 * PX_PER_LINE as i64
        );
    }

    #[test]
    fn acceleration_is_capped() {
        let mut physics = ScrollPhysics::new(&config(false, true));
        physics.streak = 1_000;
        assert_eq!(physics.acceleration(), ACCEL_MAX);
    }

    #[test]
    fn reversing_cancels_the_current_motion() {
        let mut physics = ScrollPhysics::new(&config(false, false));
        physics.feed(0.0, 1.0, 0.0, false);
        let mut up = 0;
        for i in 0..3 {
            up += physics
                .tick(i as f64 * FRAME)
                .iter()
                .map(|f| f.dy)
                .sum::<i32>();
        }
        assert!(up > 0 && up < PX_PER_LINE as i32);

        physics.feed(3.0 * FRAME, -1.0, 0.0, false);
        let after = physics.tick(4.0 * FRAME);
        assert!(
            after.iter().all(|f| f.dy <= 0),
            "no more upward motion after reversing"
        );
        assert_eq!(physics.y.pending.signum(), -1.0);
        assert!(
            physics.y.pending > -PX_PER_LINE,
            "old upward distance discarded"
        );
    }

    #[test]
    fn fast_scrolling_glides_with_momentum_then_ends() {
        let quick: Vec<(f64, f64)> = (0..12).map(|i| (i as f64 * 0.015, 1.0)).collect();
        let mut physics = ScrollPhysics::new(&config(true, true));
        let frames = run(&mut physics, &quick, false);
        assert_eq!(
            phases(&frames),
            [
                Phase::Began,
                Phase::Changed,
                Phase::Ended,
                Phase::MomentumBegan,
                Phase::MomentumChanged,
                Phase::MomentumEnded,
            ]
        );

        let mut physics = ScrollPhysics::new(&config(false, true));
        let without = total_dy(&run(&mut physics, &quick, false));
        assert!(total_dy(&frames) > without, "momentum adds glide");
    }

    #[test]
    fn single_slow_tick_never_glides() {
        let mut physics = ScrollPhysics::new(&config(true, true));
        let frames = run(&mut physics, &[(0.0, 1.0)], false);
        assert!(!frames.iter().any(|f| f.phase == Phase::MomentumBegan));
    }

    #[test]
    fn tick_during_momentum_ends_it_before_a_new_gesture() {
        let quick: Vec<(f64, f64)> = (0..12).map(|i| (i as f64 * 0.015, 1.0)).collect();
        let mut physics = ScrollPhysics::new(&config(true, true));
        let mut frames = Vec::new();
        let mut now = 0.0;
        while !matches!(physics.state, State::Momentum { began: true }) {
            for &(at, lines) in &quick {
                if at > now - FRAME && at <= now {
                    physics.feed(at, lines, 0.0, false);
                }
            }
            frames.extend(physics.tick(now));
            now += FRAME;
        }
        physics.feed(now, 1.0, 0.0, false);
        let next = physics.tick(now + FRAME);
        assert_eq!(next[0].phase, Phase::MomentumEnded);
        assert_eq!(next[1].phase, Phase::Began);
        assert!(next[1].dy > 0, "keeps moving without a stall");
    }

    #[test]
    fn precise_ticks_are_small_immediate_and_never_glide() {
        let mut physics = ScrollPhysics::new(&config(true, true));
        let ticks: Vec<(f64, f64)> = (0..10).map(|i| (i as f64 * 0.01, 1.0)).collect();
        let frames = run(&mut physics, &ticks, true);
        assert_eq!(total_dy(&frames), 10 * PRECISE_PX_PER_LINE as i64);
        assert_eq!(
            frames[0].dy, PRECISE_PX_PER_LINE as i32,
            "delivered on the first frame"
        );
        assert_eq!(
            phases(&frames),
            [Phase::Began, Phase::Changed, Phase::Ended]
        );
    }

    #[test]
    fn horizontal_ticks_move_x_only() {
        let mut physics = ScrollPhysics::new(&config(false, false));
        physics.feed(0.0, 0.0, -1.0, false);
        let mut frames = Vec::new();
        let mut now = 0.0;
        while !physics.is_idle() {
            frames.extend(physics.tick(now));
            now += FRAME;
        }
        assert_eq!(
            frames.iter().map(|f| f.dx as i64).sum::<i64>(),
            -(PX_PER_LINE as i64)
        );
        assert!(frames.iter().all(|f| f.dy == 0));
    }

    #[test]
    fn cancel_closes_the_open_phase() {
        let mut physics = ScrollPhysics::new(&config(true, true));
        assert!(physics.cancel().is_empty());

        physics.feed(0.0, 1.0, 0.0, false);
        assert!(
            physics.cancel().is_empty(),
            "nothing was posted yet, so nothing to close"
        );

        physics.feed(0.0, 1.0, 0.0, false);
        physics.tick(0.0);
        assert_eq!(physics.cancel(), [frame(0, 0, Phase::Ended)]);
        assert!(physics.is_idle());
    }

    #[test]
    fn frame_rate_barely_changes_the_curve() {
        let at = |rate: f64| {
            let mut physics = ScrollPhysics::new(&config(false, false));
            physics.feed(0.0, 4.0, 0.0, false);
            let mut sum = 0;
            let mut now = 0.0;
            while now <= 0.1 {
                sum += physics.tick(now).iter().map(|f| f.dy).sum::<i32>();
                now += 1.0 / rate;
            }
            sum
        };
        assert!(
            (at(60.0) - at(120.0)).abs() <= 6,
            "{} vs {}",
            at(60.0),
            at(120.0)
        );
    }
}
