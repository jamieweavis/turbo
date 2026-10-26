//! Turns discrete wheel ticks into a trackpad-like stream of pixel deltas.
//!
//! Pure: no CoreGraphics and no clock. Callers pass `now` in seconds from any fixed epoch,
//! which keeps the curves deterministic and testable.
//!
//! Model: every tick starts a fresh animation covering the tick's distance plus whatever the
//! previous animation hadn't delivered yet. An animation cruises at constant speed for most of
//! its distance, then eases out under exponential drag, so content moves crisply and settles
//! without a long glide. Faster spinning means more distance per tick and shorter animations,
//! and a run of quick swipes keeps multiplying the distance so long pages fly by.

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
/// How quickly the base duration falls off as ticks speed up; 0 would be linear.
const DURATION_CURVATURE: f64 = 4.0;

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

/// Speed (px/s) at which the ease-out counts as stopped.
const STOP_SPEED: f64 = 30.0;

/// macOS gesture scroll phases.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Phase {
    Began,
    Changed,
    Ended,
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
    /// Pixels per tick for an isolated tick.
    min_px: f64,
    /// Pixels per tick once ticks come every [`ACCEL_END_INTERVAL`].
    max_px: f64,
    /// Cruise duration (s) for slow ticks, shrinking towards `fast_duration` as they speed up.
    slow_duration: f64,
    fast_duration: f64,
    /// Drag coefficient (1/s) of the ease-out; higher stops sooner.
    drag: f64,
    acceleration: bool,
}

impl Params {
    fn new(config: &SmoothScrollConfig) -> Self {
        let (min_px, max_px, slow_duration, fast_duration, drag) = match config.smoothness {
            Smoothness::Low => (60.0, 120.0, 0.12, 0.09, 30.0),
            Smoothness::Regular => (60.0, 120.0, 0.18, 0.11, 23.0),
            Smoothness::High => (90.0, 180.0, 0.22, 0.22, 12.0),
        };
        let speed = config.speed.clamp(0.1, 10.0);
        Self {
            min_px: min_px * speed,
            max_px: max_px * speed,
            slow_duration,
            fast_duration,
            drag,
            acceleration: config.acceleration,
        }
    }

    /// Pixels for one tick, given the smoothed interval since the previous one.
    fn tick_distance(&self, interval: Option<f64>) -> f64 {
        if !self.acceleration {
            return self.min_px;
        }
        let interval = interval.unwrap_or(CONSECUTIVE_MAX).max(ACCEL_END_INTERVAL);
        let (slow_rate, fast_rate) = (1.0 / CONSECUTIVE_MAX, 1.0 / ACCEL_END_INTERVAL);
        let t = ((1.0 / interval - slow_rate) / (fast_rate - slow_rate)).clamp(0.0, 1.0);
        self.min_px + (self.max_px - self.min_px) * t
    }

    /// Cruise duration for the animation a tick starts.
    fn duration(&self, interval: Option<f64>) -> f64 {
        let interval = interval.unwrap_or(CONSECUTIVE_MAX);
        let t = ((CONSECUTIVE_MAX - interval) / (CONSECUTIVE_MAX - INTERVAL_MIN)).clamp(0.0, 1.0);
        let eased = (t * DURATION_CURVATURE).exp_m1() / DURATION_CURVATURE.exp_m1();
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

/// Distance over time for one animation: constant speed, then exponential drag down to
/// [`STOP_SPEED`], landing exactly on `distance`.
#[derive(Debug, Clone, Copy)]
struct Curve {
    distance: f64,
    speed: f64,
    cruise_time: f64,
    cruise_distance: f64,
    drag: f64,
    end_time: f64,
}

impl Curve {
    fn new(distance: f64, duration: f64, drag: f64) -> Self {
        let mut curve = Self {
            distance,
            speed: 0.0,
            cruise_time: 0.0,
            cruise_distance: distance,
            drag,
            end_time: 0.0,
        };
        if duration <= 0.0 || distance <= 0.0 {
            return curve;
        }
        let speed = distance / duration;
        if speed <= STOP_SPEED {
            // Too slow to need easing out.
            curve.speed = speed;
            curve.cruise_time = duration;
            curve.end_time = duration;
            return curve;
        }
        // Drag from v to the stop speed covers (v - stop) / drag.
        let tail = (speed - STOP_SPEED) / drag;
        if tail <= distance {
            curve.speed = speed;
            curve.cruise_distance = distance - tail;
            curve.cruise_time = curve.cruise_distance / speed;
        } else {
            // Short hop: all ease-out, starting just fast enough to cover the distance.
            curve.speed = drag * distance + STOP_SPEED;
            curve.cruise_distance = 0.0;
        }
        curve.end_time = curve.cruise_time + (curve.speed / STOP_SPEED).ln() / drag;
        curve
    }

    fn position(&self, t: f64) -> f64 {
        if t >= self.end_time {
            self.distance
        } else if t <= self.cruise_time {
            self.speed * t
        } else {
            let coasting = t - self.cruise_time;
            self.cruise_distance + self.speed / self.drag * -(-self.drag * coasting).exp_m1()
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct Animation {
    direction: Direction,
    started_at: f64,
    curve: Curve,
    /// Distance along the curve already handed to `carry`.
    delivered: f64,
}

impl Animation {
    /// Distance not yet handed out by a frame.
    fn remaining(&self) -> f64 {
        self.curve.distance - self.delivered
    }
}

pub(crate) struct ScrollPhysics {
    params: Params,
    cadence: Cadence,
    animation: Option<Animation>,
    /// The gesture's Began event has been posted.
    began: bool,
    /// Signed sub-pixel remainder not yet emitted.
    carry: f64,
}

impl ScrollPhysics {
    pub fn new(config: &SmoothScrollConfig) -> Self {
        Self {
            params: Params::new(config),
            cadence: Cadence::default(),
            animation: None,
            began: false,
            carry: 0.0,
        }
    }

    /// Takes effect from the next tick; an animation in flight keeps going.
    pub fn set_config(&mut self, config: &SmoothScrollConfig) {
        self.params = Params::new(config);
    }

    pub fn is_idle(&self) -> bool {
        self.animation.is_none()
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
        let tick = self.cadence.record(now, direction);

        let previous = self.animation.filter(|a| a.direction == direction);
        if previous.is_none() {
            // Reversing or switching axis stops the current motion dead rather than fighting it.
            self.carry = 0.0;
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
            let mut per_tick = self.params.tick_distance(tick.interval);
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
            started_at: now,
            curve: Curve::new(distance, duration, self.params.drag),
            delivered: 0.0,
        });
    }

    /// Advances to `now` and returns the events to post: usually one, two when the gesture
    /// ends, none when idle or when nothing reached a whole pixel.
    pub fn tick(&mut self, now: f64) -> Vec<Frame> {
        let Some(animation) = &mut self.animation else {
            return Vec::new();
        };
        let t = (now - animation.started_at).max(0.0);
        let position = animation.curve.position(t);
        let ((horizontal, sign), finished) = (animation.direction, t >= animation.curve.end_time);
        self.carry += (position - animation.delivered) * sign;
        animation.delivered = position;

        let whole = if finished {
            self.carry.round()
        } else {
            self.carry.trunc()
        };
        self.carry -= whole;
        let (dx, dy) = if horizontal {
            (whole as i32, 0)
        } else {
            (0, whole as i32)
        };
        let phase = if self.began {
            Phase::Changed
        } else {
            Phase::Began
        };

        let mut frames = Vec::new();
        if whole != 0.0 || !self.began {
            frames.push(Frame { dx, dy, phase });
            self.began = true;
        }
        if finished {
            frames.push(frame_ended());
            self.reset();
        }
        frames
    }

    /// Stops immediately, returning the event that closes the gesture if one was open.
    pub fn cancel(&mut self) -> Vec<Frame> {
        let frames = if self.began {
            vec![frame_ended()]
        } else {
            Vec::new()
        };
        self.reset();
        self.cadence = Cadence::default();
        frames
    }

    fn reset(&mut self) {
        self.animation = None;
        self.began = false;
        self.carry = 0.0;
    }
}

fn frame_ended() -> Frame {
    Frame {
        dx: 0,
        dy: 0,
        phase: Phase::Ended,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FRAME: f64 = 1.0 / 120.0;
    /// Distance of one isolated tick at the default speed.
    const TICK_PX: i64 = 60;

    fn config(acceleration: bool) -> SmoothScrollConfig {
        SmoothScrollConfig {
            acceleration,
            ..SmoothScrollConfig::default()
        }
    }

    fn new_physics(acceleration: bool) -> ScrollPhysics {
        ScrollPhysics::new(&config(acceleration))
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
        let mut physics = new_physics(true);
        let frames = run(&mut physics, &[(0.0, 1.0)], false);
        assert_eq!(total_dy(&frames), TICK_PX);
        assert!(frames.len() > 5, "should be spread over several frames");
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
    fn single_tick_settles_quickly() {
        let mut physics = new_physics(true);
        physics.feed(0.0, 1.0, 0.0, false);
        let end = physics.animation.unwrap().curve.end_time;
        assert!((0.2..0.3).contains(&end), "{end}");
    }

    #[test]
    fn phases_run_began_changed_ended() {
        let mut physics = new_physics(false);
        let frames = run(&mut physics, &[(0.0, 1.0)], false);
        assert_eq!(
            phases(&frames),
            [Phase::Began, Phase::Changed, Phase::Ended]
        );
        assert_eq!(frames.last().unwrap().dy, 0, "Ended carries no delta");
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
        assert_eq!(params.tick_distance(None), 60.0);
        assert_eq!(params.tick_distance(Some(0.001)), 120.0);
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
    fn precise_ticks_are_small_and_immediate() {
        let mut physics = new_physics(true);
        let frames = run(&mut physics, &spin(0.0, 10, 0.01), true);
        assert_eq!(total_dy(&frames), 10 * PRECISE_PX_PER_LINE as i64);
        assert_eq!(
            frames[0].dy, PRECISE_PX_PER_LINE as i32,
            "delivered on the first frame"
        );
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
        let mut physics = new_physics(true);
        assert!(physics.cancel().is_empty());

        physics.feed(0.0, 1.0, 0.0, false);
        assert!(
            physics.cancel().is_empty(),
            "nothing was posted yet, so nothing to close"
        );

        physics.feed(0.0, 1.0, 0.0, false);
        physics.tick(0.0);
        assert_eq!(physics.cancel(), [frame_ended()]);
        assert!(physics.is_idle());
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
