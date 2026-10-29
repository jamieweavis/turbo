//! Turns discrete wheel ticks into a trackpad-like stream of pixel deltas.
//!
//! Pure: no CoreGraphics and no clock. Callers pass `now` in seconds from any fixed epoch,
//! which keeps the curves deterministic and testable.
//!
//! Model: every tick starts a fresh animation covering the tick's distance plus whatever the
//! previous animation hadn't delivered yet. An animation cruises at constant speed for most of
//! its distance, then settles: Low eases out over a fixed duration, Regular and High coast
//! under drag. Faster spinning means more distance per tick and shorter animations, and a run
//! of quick swipes keeps multiplying the distance so long pages fly by.
//!
//! Output, chosen separately for vertical and horizontal scrolling: with trackpad
//! simulation, the cruise is a gesture (fingers down) and the coast is
//! momentum (fingers lifted), so scroll views rubber-band at their edges and drop the rest of
//! the coast. Low has no coast, so it is all gesture. Without it, events are plain continuous
//! scrolls with no phase, which scroll views treat like an old-style wheel and stop dead at
//! their edges.

use crate::config::{SmoothScrollConfig, Smoothness};

/// Pixels per line in precise mode: fine positioning, independent of `speed`.
const PRECISE_PX_PER_LINE: f64 = 4.0;

/// Ticks closer together than this, in the same direction, count as one continuous spin.
const CONSECUTIVE_MAX: f64 = 0.16;
/// Floor on measured tick intervals, so two ticks in one event batch don't divide by zero.
const INTERVAL_MIN: f64 = 0.001;
/// Tick interval at which per-tick distance stops growing.
const ACCEL_END_INTERVAL: f64 = 0.015;
/// How many recent intervals are averaged, so one uneven notch doesn't jolt the speed.
const INTERVAL_SMOOTHING: usize = 3;

/// A spin needs at least this many consecutive ticks to count as a swipe.
const SWIPE_MIN_TICKS: u32 = 2;
/// Longest pause between swipes that still continues a swipe run.
const SWIPE_MAX_GAP: f64 = 0.375;
/// A swipe run must average at least this many ticks per second.
const SWIPE_MIN_RATE: f64 = 16.0;
/// Swipes in a run before it starts multiplying distance.
const FAST_SCROLL_THRESHOLD: u32 = 3;
/// Multiplier on the first swipe past the threshold.
const FAST_SCROLL_INITIAL: f64 = 1.33;
/// How much each further swipe adds, as growth of the multiplier's excess.
const FAST_SCROLL_GROWTH: f64 = 2.04;
const FAST_SCROLL_MAX: f64 = 50.0;

/// Screen the per-tick distances are tuned for; bigger screens scroll slightly further.
const REFERENCE_SCREEN: (f64, f64) = (1920.0, 1080.0);
/// How much of the fastest per-tick distance follows the screen size.
const SCREEN_WEIGHT: f64 = 0.1;

/// Speed (px/s) at which a coast counts as stopped.
const STOP_SPEED: f64 = 30.0;
/// Initial slope of Low's ease-out (1 would be linear); it always ends at zero speed.
const EASE_START_SLOPE: f64 = 1.2;
/// In trackpad output, every animation is a gesture for at least this long before it may
/// turn into momentum: apps like Xcode ignore momentum deltas, so a tick must start as a
/// gesture, and fewer switches mean fewer of the stutters some apps show on each switch.
const GESTURE_MIN_TIME: f64 = CONSECUTIVE_MAX;

/// Phases of the scroll events to post.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Phase {
    /// A plain continuous scroll, outside any gesture.
    None,
    Began,
    Changed,
    Ended,
    MomentumBegan,
    MomentumChanged,
    MomentumEnded,
    /// Fingers touching the trackpad. Followed by `Cancelled` it is a tap without scrolling,
    /// which makes apps that run their own momentum stop it.
    MayBegin,
    Cancelled,
}

/// One scroll event to post: whole pixels, macOS sign convention (positive = up / left).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Frame {
    pub dx: i32,
    pub dy: i32,
    pub phase: Phase,
}

/// How an animation is reported to apps.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Output {
    Unphased,
    Trackpad,
}

/// How an animation settles once its cruise is over.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Tail {
    /// The whole animation follows an ease-out over its duration, ending at zero speed.
    Ease,
    /// Coasts under drag `dv/dt = -coefficient * v^exponent` down to [`STOP_SPEED`].
    /// Exponents below 1 glide at speed and then stop more decisively than plain friction.
    Drag { coefficient: f64, exponent: f64 },
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct Params {
    /// Pixels per tick for an isolated tick.
    min_px: f64,
    /// Pixels per tick once ticks come every [`ACCEL_END_INTERVAL`], on the reference screen.
    max_px: f64,
    /// Cruise duration (s) for slow ticks, shrinking towards `fast_duration` as they speed up.
    slow_duration: f64,
    fast_duration: f64,
    /// How quickly the duration falls off as ticks speed up; 0 is linear.
    duration_curvature: f64,
    tail: Tail,
    vertical_output: Output,
    horizontal_output: Output,
    acceleration: bool,
}

impl Params {
    fn new(config: &SmoothScrollConfig) -> Self {
        let (min_px, max_px, slow_duration, fast_duration, duration_curvature, tail) =
            match config.smoothness {
                Smoothness::Low => (60.0, 120.0, 0.24, 0.10, 0.0, Tail::Ease),
                Smoothness::Regular => (
                    60.0,
                    120.0,
                    0.18,
                    0.11,
                    4.0,
                    Tail::Drag {
                        coefficient: 23.0,
                        exponent: 1.0,
                    },
                ),
                Smoothness::High => (
                    90.0,
                    180.0,
                    0.22,
                    0.22,
                    0.0,
                    Tail::Drag {
                        coefficient: 35.0,
                        exponent: 0.75,
                    },
                ),
            };
        let output = |trackpad| {
            if trackpad {
                Output::Trackpad
            } else {
                Output::Unphased
            }
        };
        let speed = config.speed.clamp(0.1, 10.0);
        Self {
            min_px: min_px * speed,
            max_px: max_px * speed,
            slow_duration,
            fast_duration,
            duration_curvature,
            tail,
            vertical_output: output(config.trackpad_simulation_vertical),
            horizontal_output: output(config.trackpad_simulation_horizontal),
            acceleration: config.acceleration,
        }
    }

    /// Pixels for one tick, given the smoothed interval since the previous one and how big
    /// the screen is along the scroll axis relative to the reference.
    fn tick_distance(&self, interval: Option<f64>, screen_factor: f64) -> f64 {
        if !self.acceleration {
            return self.min_px;
        }
        let max_px = self.max_px * (1.0 - SCREEN_WEIGHT + SCREEN_WEIGHT * screen_factor);
        let interval = interval.unwrap_or(CONSECUTIVE_MAX).max(ACCEL_END_INTERVAL);
        let (slow_rate, fast_rate) = (1.0 / CONSECUTIVE_MAX, 1.0 / ACCEL_END_INTERVAL);
        let t = ((1.0 / interval - slow_rate) / (fast_rate - slow_rate)).clamp(0.0, 1.0);
        self.min_px + (max_px - self.min_px) * t
    }

    /// Cruise duration for the animation a tick starts.
    fn duration(&self, interval: Option<f64>) -> f64 {
        let interval = interval.unwrap_or(CONSECUTIVE_MAX);
        let t = ((CONSECUTIVE_MAX - interval) / (CONSECUTIVE_MAX - INTERVAL_MIN)).clamp(0.0, 1.0);
        let curvature = self.duration_curvature;
        let eased = if curvature == 0.0 {
            t
        } else {
            (t * curvature).exp_m1() / curvature.exp_m1()
        };
        self.slow_duration + (self.fast_duration - self.slow_duration) * eased
    }
}

/// Multiplier for a tick in the given swipe of a run (1-based).
fn fast_scroll_factor(swipe: u32) -> f64 {
    if swipe < FAST_SCROLL_THRESHOLD {
        return 1.0;
    }
    let steps = (swipe - FAST_SCROLL_THRESHOLD) as f64;
    let growth = (FAST_SCROLL_GROWTH.powf(steps) - 1.0) / (FAST_SCROLL_GROWTH - 1.0);
    (1.0 + (FAST_SCROLL_INITIAL - 1.0) * growth).min(FAST_SCROLL_MAX)
}

/// Axis (`true` = horizontal) and sign of a tick.
type Direction = (bool, f64);

/// What the tick history says about one tick.
#[derive(Debug, Clone, Copy, PartialEq)]
struct TickInfo {
    /// Smoothed time since the previous tick, or `None` for the first tick of a spin.
    interval: Option<f64>,
    /// Ticks in the current spin before this one.
    consecutive: u32,
    /// Completed swipes in the current run.
    swipes: u32,
}

/// Tracks the rhythm of the wheel: spins of consecutive ticks, and runs of quick swipes.
#[derive(Debug, Clone, Default)]
struct Cadence {
    direction: Option<Direction>,
    last_tick_at: Option<f64>,
    consecutive: u32,
    intervals: Vec<f64>,
    swipes: u32,
    run_started_at: f64,
    run_ticks: u32,
}

impl Cadence {
    fn record(&mut self, now: f64, direction: Direction) -> TickInfo {
        if self.direction != Some(direction) {
            *self = Self {
                direction: Some(direction),
                ..Self::default()
            };
        }
        self.run_ticks += 1;
        let gap = self.last_tick_at.map(|at| (now - at).max(INTERVAL_MIN));
        self.last_tick_at = Some(now);

        match gap {
            Some(gap) if gap <= CONSECUTIVE_MAX => {
                self.consecutive += 1;
                if self.intervals.len() == INTERVAL_SMOOTHING {
                    self.intervals.remove(0);
                }
                self.intervals.push(gap);
            }
            _ => {
                let continues_run = self.consecutive >= SWIPE_MIN_TICKS
                    && gap.is_some_and(|gap| gap <= SWIPE_MAX_GAP)
                    && self.run_ticks as f64 / (now - self.run_started_at) >= SWIPE_MIN_RATE;
                if continues_run {
                    self.swipes += 1;
                } else {
                    self.swipes = 0;
                    self.run_started_at = now;
                    self.run_ticks = 0;
                }
                self.consecutive = 0;
                self.intervals.clear();
            }
        }

        TickInfo {
            interval: (!self.intervals.is_empty())
                .then(|| self.intervals.iter().sum::<f64>() / self.intervals.len() as f64),
            consecutive: self.consecutive,
            swipes: self.swipes,
        }
    }
}

/// A coast under drag from some starting speed down to [`STOP_SPEED`].
#[derive(Debug, Clone, Copy, PartialEq)]
struct Drag {
    coefficient: f64,
    exponent: f64,
}

impl Drag {
    fn is_linear(&self) -> bool {
        (self.exponent - 1.0).abs() < 1e-9
    }

    /// Distance covered from `speed` until stopped.
    fn distance(&self, speed: f64) -> f64 {
        let (a, b) = (self.coefficient, self.exponent);
        if self.is_linear() {
            (speed - STOP_SPEED) / a
        } else {
            (speed.powf(2.0 - b) - STOP_SPEED.powf(2.0 - b)) / ((2.0 - b) * a)
        }
    }

    /// The starting speed that covers exactly `distance` before stopping.
    fn speed_for(&self, distance: f64) -> f64 {
        let (a, b) = (self.coefficient, self.exponent);
        if self.is_linear() {
            a * distance + STOP_SPEED
        } else {
            (distance * (2.0 - b) * a + STOP_SPEED.powf(2.0 - b)).powf(1.0 / (2.0 - b))
        }
    }

    /// Time from `speed` until stopped.
    fn duration(&self, speed: f64) -> f64 {
        let (a, b) = (self.coefficient, self.exponent);
        if self.is_linear() {
            (speed / STOP_SPEED).ln() / a
        } else {
            (speed.powf(1.0 - b) - STOP_SPEED.powf(1.0 - b)) / ((1.0 - b) * a)
        }
    }

    /// Distance covered `t` seconds into a coast that started at `speed`.
    fn position(&self, speed: f64, t: f64) -> f64 {
        let (a, b) = (self.coefficient, self.exponent);
        if self.is_linear() {
            speed / a * -(-a * t).exp_m1()
        } else {
            let k = 1.0 - b;
            let now_speed = (speed.powf(k) - k * a * t).max(0.0).powf(1.0 / k);
            (speed.powf(2.0 - b) - now_speed.powf(2.0 - b)) / ((2.0 - b) * a)
        }
    }
}

/// Low's ease-out: starts at [`EASE_START_SLOPE`] times the average speed and ends at rest.
fn ease_out(u: f64) -> f64 {
    let s = EASE_START_SLOPE;
    u * (s + u * ((3.0 - 2.0 * s) + u * (s - 2.0)))
}

/// Distance over time for one animation, landing exactly on `distance` at `end_time`.
#[derive(Debug, Clone, Copy)]
struct Curve {
    distance: f64,
    tail: Tail,
    /// Cruise speed, or the coast's starting speed when there is no cruise.
    speed: f64,
    /// When the cruise ends. For an ease-out the whole curve counts as cruise.
    cruise_time: f64,
    cruise_distance: f64,
    end_time: f64,
}

impl Curve {
    fn new(distance: f64, duration: f64, tail: Tail) -> Self {
        let mut curve = Self {
            distance,
            tail,
            speed: 0.0,
            cruise_time: 0.0,
            cruise_distance: distance,
            end_time: 0.0,
        };
        if duration <= 0.0 || distance <= 0.0 {
            return curve;
        }
        let speed = distance / duration;
        let drag = match tail {
            Tail::Ease => {
                curve.cruise_time = duration;
                curve.end_time = duration;
                return curve;
            }
            Tail::Drag {
                coefficient,
                exponent,
            } => Drag {
                coefficient,
                exponent,
            },
        };
        if speed <= STOP_SPEED {
            // Too slow to need a coast.
            curve.speed = speed;
            curve.cruise_time = duration;
            curve.end_time = duration;
            return curve;
        }
        let coast = drag.distance(speed);
        if coast <= distance {
            curve.speed = speed;
            curve.cruise_distance = distance - coast;
            curve.cruise_time = curve.cruise_distance / speed;
        } else {
            // Short hop: all coast, starting just fast enough to cover the distance.
            curve.speed = drag.speed_for(distance);
            curve.cruise_distance = 0.0;
        }
        curve.end_time = curve.cruise_time + drag.duration(curve.speed);
        curve
    }

    fn position(&self, t: f64) -> f64 {
        if t >= self.end_time {
            return self.distance;
        }
        match self.tail {
            Tail::Ease => self.distance * ease_out(t / self.end_time),
            Tail::Drag { .. } if t <= self.cruise_time => self.speed * t,
            Tail::Drag {
                coefficient,
                exponent,
            } => {
                let drag = Drag {
                    coefficient,
                    exponent,
                };
                let coasted = drag.position(self.speed, t - self.cruise_time);
                (self.cruise_distance + coasted).min(self.distance)
            }
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct Animation {
    direction: Direction,
    output: Output,
    started_at: f64,
    curve: Curve,
    /// Distance along the curve already handed to `carry`.
    delivered: f64,
    /// Animation time of the previous frame, if there was one.
    last_t: Option<f64>,
}

impl Animation {
    /// Distance not yet handed out by a frame.
    fn remaining(&self) -> f64 {
        self.curve.distance - self.delivered
    }
}

/// Which trackpad phase has been opened by a posted event and must be closed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Open {
    None,
    Gesture,
    Momentum,
}

pub(crate) struct ScrollPhysics {
    params: Params,
    cadence: Cadence,
    animation: Option<Animation>,
    open: Open,
    /// Events closing a phase that a tick interrupted, posted before the next frame.
    pending: Vec<Frame>,
    /// Signed sub-pixel remainder not yet emitted.
    carry: f64,
    /// Size of the screen being scrolled, in points.
    screen: (f64, f64),
}

impl ScrollPhysics {
    pub fn new(config: &SmoothScrollConfig) -> Self {
        Self {
            params: Params::new(config),
            cadence: Cadence::default(),
            animation: None,
            open: Open::None,
            pending: Vec::new(),
            carry: 0.0,
            screen: REFERENCE_SCREEN,
        }
    }

    /// Takes effect from the next tick; an animation in flight keeps going.
    pub fn set_config(&mut self, config: &SmoothScrollConfig) {
        self.params = Params::new(config);
    }

    /// The size of the screen under the pointer, applied from the next tick.
    pub fn set_screen(&mut self, width: f64, height: f64) {
        if width > 0.0 && height > 0.0 {
            self.screen = (width, height);
        }
    }

    pub fn is_idle(&self) -> bool {
        self.animation.is_none() && self.pending.is_empty()
    }

    /// Adds one wheel tick, in lines with macOS's sign convention.
    pub fn feed(&mut self, now: f64, vertical: f64, horizontal: f64, precise: bool) {
        if vertical == 0.0 && horizontal == 0.0 {
            return;
        }
        let (direction, lines) = if vertical.abs() >= horizontal.abs() {
            ((false, vertical.signum()), vertical.abs())
        } else {
            ((true, horizontal.signum()), horizontal.abs())
        };
        let output = match (precise, direction) {
            (true, _) => Output::Unphased,
            (false, (true, _)) => self.params.horizontal_output,
            (false, (false, _)) => self.params.vertical_output,
        };
        let tick = self.cadence.record(now, direction);

        let previous = self
            .animation
            .filter(|a| a.direction == direction && a.output == output);
        if previous.is_none() {
            // Reversing or switching axis stops the current motion dead rather than fighting
            // it, and ends its gesture so the new direction starts a fresh one.
            self.carry = 0.0;
            let closing = self.close();
            self.pending.extend(closing);
        }
        // A tick after a pause starts afresh; one that continues a spin or swipe run picks
        // up what's left so quick ticks add up.
        let continues = tick.consecutive > 0 || tick.swipes > 0;
        let leftover = previous
            .filter(|_| continues && !precise)
            .map_or(0.0, |a| a.remaining());

        let (distance, duration) = if precise {
            (PRECISE_PX_PER_LINE * lines, 0.0)
        } else {
            let (horizontal, _) = direction;
            let screen_factor = if horizontal {
                self.screen.0 / REFERENCE_SCREEN.0
            } else {
                self.screen.1 / REFERENCE_SCREEN.1
            };
            let mut per_tick = self.params.tick_distance(tick.interval, screen_factor);
            if self.params.acceleration {
                per_tick *= fast_scroll_factor(tick.swipes + 1);
            }
            (
                leftover + per_tick * lines,
                self.params.duration(tick.interval),
            )
        };
        self.animation = Some(Animation {
            direction,
            output,
            started_at: now,
            curve: Curve::new(distance, duration, self.params.tail),
            delivered: 0.0,
            last_t: None,
        });
    }

    /// Advances to `now` and returns the events to post: usually one, a few around phase
    /// changes, none when idle or when nothing reached a whole pixel.
    pub fn tick(&mut self, now: f64) -> Vec<Frame> {
        let mut frames = std::mem::take(&mut self.pending);
        let Some(animation) = &mut self.animation else {
            return frames;
        };
        let t = (now - animation.started_at).max(0.0);
        let position = animation.curve.position(t);
        let ((horizontal, sign), finished) = (animation.direction, t >= animation.curve.end_time);
        self.carry += (position - animation.delivered) * sign;
        animation.delivered = position;

        // A frame is momentum only once all of it lies past the cruise.
        let coasting = animation.last_t.is_some_and(|last| {
            last >= animation.curve.cruise_time.max(GESTURE_MIN_TIME)
                && matches!(animation.curve.tail, Tail::Drag { .. })
        });
        animation.last_t = Some(t);
        let output = animation.output;

        let whole = if finished {
            self.carry.round()
        } else {
            self.carry.trunc()
        };
        self.carry -= whole;
        if whole != 0.0 {
            let (dx, dy) = if horizontal {
                (whole as i32, 0)
            } else {
                (0, whole as i32)
            };
            let phase = match (output, self.open, coasting) {
                (Output::Unphased, ..) => Phase::None,
                (Output::Trackpad, Open::None, _) => {
                    self.open = Open::Gesture;
                    Phase::Began
                }
                (Output::Trackpad, Open::Gesture, false) => Phase::Changed,
                (Output::Trackpad, Open::Gesture, true) => {
                    frames.push(zero_frame(Phase::Ended));
                    self.open = Open::Momentum;
                    Phase::MomentumBegan
                }
                (Output::Trackpad, Open::Momentum, true) => Phase::MomentumChanged,
                (Output::Trackpad, Open::Momentum, false) => {
                    // A tick during the coast puts the fingers back down.
                    frames.push(zero_frame(Phase::MomentumEnded));
                    self.open = Open::Gesture;
                    Phase::Began
                }
            };
            frames.push(Frame { dx, dy, phase });
        }
        if finished {
            frames.extend(self.close());
            self.animation = None;
            self.carry = 0.0;
        }
        frames
    }

    /// Stops immediately, returning the events that close the gesture if one was open.
    pub fn cancel(&mut self) -> Vec<Frame> {
        let mut frames = std::mem::take(&mut self.pending);
        frames.extend(self.close());
        self.animation = None;
        self.carry = 0.0;
        self.cadence = Cadence::default();
        frames
    }

    /// Ends whichever phase is open, then taps so apps with their own momentum don't
    /// carry on coasting.
    fn close(&mut self) -> Vec<Frame> {
        let end = match self.open {
            Open::None => return Vec::new(),
            Open::Gesture => Phase::Ended,
            Open::Momentum => Phase::MomentumEnded,
        };
        self.open = Open::None;
        vec![
            zero_frame(end),
            zero_frame(Phase::MayBegin),
            zero_frame(Phase::Cancelled),
        ]
    }
}

fn zero_frame(phase: Phase) -> Frame {
    Frame {
        dx: 0,
        dy: 0,
        phase,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FRAME: f64 = 1.0 / 120.0;
    /// Distance of one isolated tick at the default speed.
    const TICK_PX: i64 = 60;
    const HIGH_TICK_PX: i64 = 90;

    fn config(acceleration: bool) -> SmoothScrollConfig {
        SmoothScrollConfig {
            acceleration,
            ..SmoothScrollConfig::default()
        }
    }

    fn new_physics(acceleration: bool) -> ScrollPhysics {
        ScrollPhysics::new(&config(acceleration))
    }

    fn physics_with(smoothness: Smoothness) -> ScrollPhysics {
        ScrollPhysics::new(&SmoothScrollConfig {
            smoothness,
            ..config(false)
        })
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

    /// `count` ticks `spacing` apart, starting at `from`.
    fn spin(from: f64, count: usize, spacing: f64) -> Vec<(f64, f64)> {
        (0..count)
            .map(|i| (from + i as f64 * spacing, 1.0))
            .collect()
    }

    #[test]
    fn one_tick_delivers_exactly_its_distance() {
        for (smoothness, expected) in [
            (Smoothness::Low, TICK_PX),
            (Smoothness::Regular, TICK_PX),
            (Smoothness::High, HIGH_TICK_PX),
        ] {
            let mut physics = physics_with(smoothness);
            let frames = run(&mut physics, &[(0.0, 1.0)], false);
            assert_eq!(total_dy(&frames), expected, "{smoothness:?}");
            assert!(frames.len() > 5, "should be spread over several frames");
        }
    }

    #[test]
    fn distance_is_conserved_at_fractional_speeds_and_downward() {
        let mut cfg = config(false);
        cfg.speed = 1.37;
        let mut physics = ScrollPhysics::new(&cfg);
        let frames = run(
            &mut physics,
            &[(0.0, -1.0), (0.03, -1.0), (0.6, -1.0)],
            false,
        );
        let expected = (-3.0 * TICK_PX as f64 * 1.37).round() as i64;
        assert!(
            (total_dy(&frames) - expected).abs() <= 1,
            "{} vs {expected}",
            total_dy(&frames)
        );
    }

    #[test]
    fn cruises_then_eases_out() {
        let mut physics = new_physics(false);
        let frames = run(&mut physics, &[(0.0, 4.0)], false);
        let moving: Vec<i32> = frames.iter().map(|f| f.dy).filter(|&d| d != 0).collect();
        assert!(
            (moving[1] - moving[3]).abs() <= 1,
            "steady while cruising: {moving:?}"
        );
        assert!(
            moving[1] > moving[moving.len() - 2] * 3,
            "eases out: {moving:?}"
        );
    }

    #[test]
    fn low_eases_out_to_rest() {
        let mut physics = physics_with(Smoothness::Low);
        let frames = run(&mut physics, &[(0.0, 4.0)], false);
        let moving: Vec<i32> = frames.iter().map(|f| f.dy).filter(|&d| d != 0).collect();
        assert!(moving[0] >= moving[moving.len() / 2], "{moving:?}");
        assert!(moving[0] > moving[moving.len() - 1] * 3, "{moving:?}");
        assert!((ease_out(1.0) - 1.0).abs() < 1e-12);
    }

    #[test]
    fn power_drag_lands_on_its_distance() {
        let drag = Drag {
            coefficient: 35.0,
            exponent: 0.75,
        };
        for speed in [100.0, 1_000.0, 10_000.0] {
            let end = drag.duration(speed);
            assert!((drag.position(speed, end) - drag.distance(speed)).abs() < 1e-6);
            assert!((drag.speed_for(drag.distance(speed)) - speed).abs() < 1e-6);
        }
        let curve = Curve::new(
            2_000.0,
            0.22,
            Tail::Drag {
                coefficient: 35.0,
                exponent: 0.75,
            },
        );
        let mut last = 0.0;
        let mut t = 0.0;
        while t < curve.end_time {
            let p = curve.position(t);
            assert!(p >= last, "monotonic");
            last = p;
            t += 0.001;
        }
        assert!((curve.position(curve.end_time - 1e-9) - 2_000.0).abs() < 0.5);
    }

    #[test]
    fn single_tick_settles_quickly() {
        let mut physics = new_physics(true);
        physics.feed(0.0, 1.0, 0.0, false);
        let end = physics.animation.unwrap().curve.end_time;
        assert!((0.2..0.3).contains(&end), "{end}");
    }

    #[test]
    fn without_trackpad_simulation_every_preset_is_unphased() {
        for smoothness in [Smoothness::Low, Smoothness::Regular, Smoothness::High] {
            let mut physics = ScrollPhysics::new(&SmoothScrollConfig {
                smoothness,
                trackpad_simulation_vertical: false,
                trackpad_simulation_horizontal: false,
                ..config(false)
            });
            let frames = run(&mut physics, &[(0.0, 1.0)], false);
            assert_eq!(phases(&frames), [Phase::None], "{smoothness:?}");
            assert!(frames.iter().all(|f| f.dy != 0), "no empty events");
        }
    }

    #[test]
    fn trackpad_simulation_is_per_axis() {
        let mut physics = ScrollPhysics::new(&SmoothScrollConfig {
            trackpad_simulation_vertical: true,
            trackpad_simulation_horizontal: false,
            ..config(false)
        });
        physics.feed(0.0, 0.0, 1.0, false);
        let mut horizontal = Vec::new();
        let mut now = 0.0;
        while !physics.is_idle() {
            horizontal.extend(physics.tick(now));
            now += FRAME;
        }
        assert_eq!(phases(&horizontal), [Phase::None]);

        let vertical = run(&mut physics, &[(10.0, 1.0)], false);
        assert_eq!(phases(&vertical)[0], Phase::Began);
    }

    #[test]
    fn regular_simulates_a_trackpad_too() {
        let mut physics = physics_with(Smoothness::Regular);
        let frames = run(&mut physics, &[(0.0, 4.0)], false);
        let phases = phases(&frames);
        assert_eq!(phases[0], Phase::Began);
        assert!(phases.contains(&Phase::MomentumBegan), "{phases:?}");
    }

    #[test]
    fn low_is_all_gesture_with_no_momentum() {
        let mut physics = physics_with(Smoothness::Low);
        let frames = run(&mut physics, &[(0.0, 4.0)], false);
        assert_eq!(
            phases(&frames),
            [
                Phase::Began,
                Phase::Changed,
                Phase::Ended,
                Phase::MayBegin,
                Phase::Cancelled
            ]
        );
    }

    #[test]
    fn high_runs_gesture_then_momentum_then_taps() {
        let mut physics = physics_with(Smoothness::High);
        let frames = run(&mut physics, &[(0.0, 1.0)], false);
        assert_eq!(
            phases(&frames),
            [
                Phase::Began,
                Phase::Changed,
                Phase::Ended,
                Phase::MomentumBegan,
                Phase::MomentumChanged,
                Phase::MomentumEnded,
                Phase::MayBegin,
                Phase::Cancelled,
            ]
        );
        for frame in &frames {
            let moves = frame.dy != 0;
            let should_move = matches!(
                frame.phase,
                Phase::Began | Phase::Changed | Phase::MomentumBegan | Phase::MomentumChanged
            );
            assert_eq!(moves, should_move, "{frame:?}");
        }
    }

    #[test]
    fn high_stays_a_gesture_at_first() {
        let mut physics = physics_with(Smoothness::High);
        let mut now = 0.0;
        physics.feed(0.0, 1.0, 0.0, false);
        let mut early = Vec::new();
        while now < GESTURE_MIN_TIME {
            early.extend(physics.tick(now));
            now += FRAME;
        }
        assert!(
            early
                .iter()
                .all(|f| matches!(f.phase, Phase::Began | Phase::Changed)),
            "{early:?}"
        );
    }

    #[test]
    fn a_tick_during_momentum_starts_a_new_gesture() {
        let mut physics = physics_with(Smoothness::High);
        let frames = run(&mut physics, &[(0.0, 1.0), (0.25, 1.0)], false);
        let phases = phases(&frames);
        let resumed = phases
            .windows(2)
            .any(|w| w == [Phase::MomentumEnded, Phase::Began]);
        assert!(resumed, "{phases:?}");
        assert!(total_dy(&frames) > HIGH_TICK_PX);
    }

    #[test]
    fn high_reversal_ends_the_gesture_and_starts_another() {
        let mut physics = physics_with(Smoothness::High);
        physics.feed(0.0, 1.0, 0.0, false);
        physics.tick(0.0);
        physics.tick(FRAME);
        physics.feed(2.0 * FRAME, -1.0, 0.0, false);
        let frames = physics.tick(2.0 * FRAME);
        assert_eq!(
            frames.iter().map(|f| f.phase).collect::<Vec<_>>()[..3],
            [Phase::Ended, Phase::MayBegin, Phase::Cancelled]
        );
        let rest = run(&mut physics, &[], false);
        assert_eq!(rest.iter().find(|f| f.dy != 0).unwrap().phase, Phase::Began);
    }

    #[test]
    fn quick_ticks_accelerate() {
        let mut physics = new_physics(true);
        let fast_total = total_dy(&run(&mut physics, &spin(0.0, 10, 0.02), false));
        let mut physics = new_physics(true);
        let slow_total = total_dy(&run(&mut physics, &spin(0.0, 10, 0.5), false));

        assert_eq!(slow_total, 10 * TICK_PX);
        assert!(
            fast_total > slow_total * 3 / 2,
            "{fast_total} vs {slow_total}"
        );
    }

    #[test]
    fn acceleration_off_is_linear() {
        let mut physics = new_physics(false);
        assert_eq!(
            total_dy(&run(&mut physics, &spin(0.0, 10, 0.02), false)),
            10 * TICK_PX
        );
    }

    #[test]
    fn per_tick_distance_is_capped() {
        let params = Params::new(&config(true));
        assert_eq!(params.tick_distance(None, 1.0), 60.0);
        assert_eq!(params.tick_distance(Some(0.001), 1.0), 120.0);
    }

    #[test]
    fn bigger_screens_scroll_slightly_further_when_fast() {
        let params = Params::new(&config(true));
        assert_eq!(
            params.tick_distance(None, 2.0),
            60.0,
            "slow ticks unchanged"
        );
        let fast = params.tick_distance(Some(0.001), 2.0);
        assert!((fast - 132.0).abs() < 1e-9, "{fast}");
    }

    #[test]
    fn faster_ticks_shorten_the_animation() {
        let params = Params::new(&config(true));
        assert!((params.duration(None) - 0.18).abs() < 1e-9);
        assert!((params.duration(Some(INTERVAL_MIN)) - 0.11).abs() < 1e-9);
        assert!(
            params.duration(Some(0.08)) > 0.17,
            "only quick spins speed it up"
        );
    }

    #[test]
    fn swipe_runs_speed_up() {
        assert_eq!(fast_scroll_factor(1), 1.0);
        assert_eq!(fast_scroll_factor(3), 1.0);
        assert!((fast_scroll_factor(4) - FAST_SCROLL_INITIAL).abs() < 1e-9);
        assert!(fast_scroll_factor(6) > 3.0);
        assert_eq!(fast_scroll_factor(100), FAST_SCROLL_MAX);

        let mut cadence = Cadence::default();
        let mut info = None;
        for swipe in 0..5 {
            for (at, _) in spin(swipe as f64 * 0.25, 6, 0.015) {
                info = Some(cadence.record(at, (false, 1.0)));
            }
        }
        assert_eq!(info.unwrap().swipes, 4);

        // A long pause ends the run.
        assert_eq!(cadence.record(10.0, (false, 1.0)).swipes, 0);
    }

    #[test]
    fn reversing_cancels_the_current_motion() {
        let mut physics = new_physics(false);
        physics.feed(0.0, 1.0, 0.0, false);
        let mut up = 0;
        for i in 0..3 {
            up += physics
                .tick(i as f64 * FRAME)
                .iter()
                .map(|f| f.dy)
                .sum::<i32>();
        }
        assert!(up > 0 && up < TICK_PX as i32);

        physics.feed(3.0 * FRAME, -1.0, 0.0, false);
        let down = run(&mut physics, &[], false);
        assert!(down.iter().all(|f| f.dy <= 0), "no more upward motion");
        assert_eq!(total_dy(&down), -TICK_PX, "old upward distance discarded");
    }

    #[test]
    fn ticks_after_a_pause_drop_the_leftover() {
        let mut physics = new_physics(false);
        let frames = run(&mut physics, &[(0.0, 1.0), (0.17, 1.0)], false);
        assert!(total_dy(&frames) < 2 * TICK_PX);
    }

    #[test]
    fn precise_ticks_are_small_immediate_and_unphased() {
        let mut physics = physics_with(Smoothness::High);
        let frames = run(&mut physics, &spin(0.0, 10, 0.01), true);
        assert_eq!(total_dy(&frames), 10 * PRECISE_PX_PER_LINE as i64);
        assert_eq!(
            frames[0].dy, PRECISE_PX_PER_LINE as i32,
            "delivered on the first frame"
        );
        assert_eq!(phases(&frames), [Phase::None]);
    }

    #[test]
    fn horizontal_ticks_move_x_only() {
        let mut physics = new_physics(false);
        physics.feed(0.0, 0.0, -1.0, false);
        let mut frames = Vec::new();
        let mut now = 0.0;
        while !physics.is_idle() {
            frames.extend(physics.tick(now));
            now += FRAME;
        }
        assert_eq!(frames.iter().map(|f| f.dx as i64).sum::<i64>(), -TICK_PX);
        assert!(frames.iter().all(|f| f.dy == 0));
    }

    #[test]
    fn cancel_closes_the_open_phase() {
        let mut physics = physics_with(Smoothness::High);
        assert!(physics.cancel().is_empty());

        physics.feed(0.0, 1.0, 0.0, false);
        assert!(
            physics.cancel().is_empty(),
            "nothing was posted yet, so nothing to close"
        );

        physics.feed(0.0, 1.0, 0.0, false);
        physics.tick(0.0);
        physics.tick(FRAME);
        assert_eq!(
            physics.cancel(),
            [
                zero_frame(Phase::Ended),
                zero_frame(Phase::MayBegin),
                zero_frame(Phase::Cancelled)
            ]
        );
        assert!(physics.is_idle());

        let mut unphased = ScrollPhysics::new(&SmoothScrollConfig {
            trackpad_simulation_vertical: false,
            ..config(false)
        });
        unphased.feed(0.0, 1.0, 0.0, false);
        unphased.tick(0.0);
        unphased.tick(FRAME);
        assert!(unphased.cancel().is_empty(), "nothing to close");
    }

    #[test]
    fn frame_rate_barely_changes_the_curve() {
        let at = |rate: f64| {
            let mut physics = new_physics(false);
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
