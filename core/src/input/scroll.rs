//! Scroll wheel routing as pure logic: given one scroll event's deltas and held modifiers,
//! decide whether to pass it, rewrite it, smooth it or turn it into zoom.

use crate::config::{Modifier, MouseConfig};

/// Without smoothing, precise mode scales each tick down by this much.
pub(crate) const PRECISE_FACTOR: f64 = 0.25;

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub(crate) struct Axes<T> {
    pub vertical: T,
    pub horizontal: T,
}

impl<T> Axes<T> {
    fn swapped(self) -> Self {
        Self {
            vertical: self.horizontal,
            horizontal: self.vertical,
        }
    }

    fn map<U>(self, f: impl Fn(T) -> U) -> Axes<U> {
        Axes {
            vertical: f(self.vertical),
            horizontal: f(self.horizontal),
        }
    }
}

/// The parts of a scroll wheel event routing cares about. Deltas use macOS's signs
/// (positive vertical is "up"), already reflecting the system natural-scrolling setting.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct WheelEvent {
    /// Trackpads and Magic Mouse send continuous (pixel) scrolling; wheels don't.
    pub continuous: bool,
    pub lines: Axes<i64>,
    /// Fractional lines (`kCGScrollWheelEventFixedPtDeltaAxis*`).
    pub fixed: Axes<f64>,
    pub points: Axes<i64>,
    pub modifiers: Vec<Modifier>,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum ScrollDecision {
    PassThrough,
    Swallow,
    /// Pass the event with these deltas, and with these modifier flags cleared so apps
    /// don't apply their own meaning on top of ours (e.g. AppKit's shift-to-horizontal).
    Rewrite {
        lines: Axes<i64>,
        fixed: Axes<f64>,
        points: Axes<i64>,
        clear_modifiers: Vec<Modifier>,
    },
    /// Swallow and hand to the smooth scroller, in lines.
    Smooth {
        vertical: f64,
        horizontal: f64,
        precise: bool,
    },
    /// Swallow and send one zoom step.
    Zoom {
        zoom_in: bool,
    },
}

pub(crate) fn route(event: &WheelEvent, config: &MouseConfig) -> ScrollDecision {
    if event.continuous {
        return ScrollDecision::PassThrough;
    }
    let modifiers = &config.scroll_modifiers;
    let held = |m: Option<Modifier>| m.is_some_and(|m| event.modifiers.contains(&m));

    if held(modifiers.zoom) {
        let amount = if event.fixed.vertical != 0.0 {
            event.fixed.vertical
        } else {
            event.fixed.horizontal
        };
        return if amount == 0.0 {
            ScrollDecision::Swallow
        } else {
            ScrollDecision::Zoom {
                zoom_in: amount > 0.0,
            }
        };
    }

    let mut lines = event.lines;
    let mut fixed = event.fixed;
    let mut points = event.points;
    let mut clear_modifiers = Vec::new();

    if modifiers.reverse_direction {
        lines = lines.map(|v| -v);
        fixed = fixed.map(|v| -v);
        points = points.map(|v| -v);
    }
    if held(modifiers.horizontal) {
        lines = lines.swapped();
        fixed = fixed.swapped();
        points = points.swapped();
        clear_modifiers.extend(modifiers.horizontal);
    }
    let precise = held(modifiers.precise);
    if precise {
        clear_modifiers.extend(modifiers.precise);
    }

    if config.smooth_scroll.enabled {
        // The scroller applies its own acceleration, so it gets the tick counts
        // (`kCGScrollWheelEventDeltaAxis*`, normally ±1), not the accelerated deltas.
        return ScrollDecision::Smooth {
            vertical: lines.vertical as f64,
            horizontal: lines.horizontal as f64,
            precise,
        };
    }

    if precise {
        fixed = fixed.map(|v| v * PRECISE_FACTOR);
        points = points.map(|v| scale_keeping_sign(v, PRECISE_FACTOR));
        // Line deltas are integers; keep at least one so line-based apps still move.
        lines = lines.map(|v| scale_keeping_sign(v, PRECISE_FACTOR));
    }

    if lines == event.lines
        && fixed == event.fixed
        && points == event.points
        && clear_modifiers.is_empty()
    {
        return ScrollDecision::PassThrough;
    }
    ScrollDecision::Rewrite {
        lines,
        fixed,
        points,
        clear_modifiers,
    }
}

/// Scales an integer delta, never rounding a non-zero value down to zero.
fn scale_keeping_sign(value: i64, factor: f64) -> i64 {
    if value == 0 {
        return 0;
    }
    let scaled = (value as f64 * factor).round() as i64;
    if scaled == 0 { value.signum() } else { scaled }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{ScrollModifierConfig, SmoothScrollConfig};

    fn wheel(lines: i64, modifiers: Vec<Modifier>) -> WheelEvent {
        WheelEvent {
            continuous: false,
            lines: Axes {
                vertical: lines,
                horizontal: 0,
            },
            fixed: Axes {
                vertical: lines as f64,
                horizontal: 0.0,
            },
            points: Axes {
                vertical: lines * 10,
                horizontal: 0,
            },
            modifiers,
        }
    }

    fn config(smooth: bool, modifiers: ScrollModifierConfig) -> MouseConfig {
        MouseConfig {
            smooth_scroll: SmoothScrollConfig {
                enabled: smooth,
                ..SmoothScrollConfig::default()
            },
            scroll_modifiers: modifiers,
            ..MouseConfig::default()
        }
    }

    fn no_modifiers() -> ScrollModifierConfig {
        ScrollModifierConfig {
            reverse_direction: false,
            horizontal: None,
            zoom: None,
            precise: None,
        }
    }

    #[test]
    fn continuous_scrolling_is_never_touched() {
        let mut event = wheel(3, vec![Modifier::Command]);
        event.continuous = true;
        let config = config(true, ScrollModifierConfig::default());
        assert_eq!(route(&event, &config), ScrollDecision::PassThrough);
    }

    #[test]
    fn plain_wheel_without_smoothing_passes_through() {
        let config = config(false, no_modifiers());
        assert_eq!(
            route(&wheel(1, vec![]), &config),
            ScrollDecision::PassThrough
        );
    }

    #[test]
    fn plain_wheel_with_smoothing_feeds_the_scroller() {
        let config = config(true, no_modifiers());
        assert_eq!(
            route(&wheel(-2, vec![]), &config),
            ScrollDecision::Smooth {
                vertical: -2.0,
                horizontal: 0.0,
                precise: false
            }
        );
    }

    #[test]
    fn smoothing_feeds_tick_counts_not_accelerated_deltas() {
        let config = config(true, no_modifiers());
        let mut event = wheel(1, vec![]);
        event.fixed.vertical = 4.7;
        event.points.vertical = 47;
        assert_eq!(
            route(&event, &config),
            ScrollDecision::Smooth {
                vertical: 1.0,
                horizontal: 0.0,
                precise: false
            }
        );
    }

    #[test]
    fn reverse_flips_every_delta() {
        let config = config(
            false,
            ScrollModifierConfig {
                reverse_direction: true,
                ..no_modifiers()
            },
        );
        assert_eq!(
            route(&wheel(1, vec![]), &config),
            ScrollDecision::Rewrite {
                lines: Axes {
                    vertical: -1,
                    horizontal: 0
                },
                fixed: Axes {
                    vertical: -1.0,
                    horizontal: 0.0
                },
                points: Axes {
                    vertical: -10,
                    horizontal: 0
                },
                clear_modifiers: vec![],
            }
        );
    }

    #[test]
    fn horizontal_modifier_swaps_axes_and_clears_its_flag() {
        let config = config(false, ScrollModifierConfig::default());
        let ScrollDecision::Rewrite {
            lines,
            clear_modifiers,
            ..
        } = route(&wheel(2, vec![Modifier::Shift]), &config)
        else {
            panic!("expected a rewrite");
        };
        assert_eq!(
            lines,
            Axes {
                vertical: 0,
                horizontal: 2
            }
        );
        assert_eq!(clear_modifiers, vec![Modifier::Shift]);
    }

    #[test]
    fn horizontal_and_reverse_combine_when_smoothing() {
        let config = config(
            true,
            ScrollModifierConfig {
                reverse_direction: true,
                ..ScrollModifierConfig::default()
            },
        );
        assert_eq!(
            route(&wheel(1, vec![Modifier::Shift]), &config),
            ScrollDecision::Smooth {
                vertical: 0.0,
                horizontal: -1.0,
                precise: false
            }
        );
    }

    #[test]
    fn zoom_modifier_turns_ticks_into_zoom_steps() {
        let config = config(true, ScrollModifierConfig::default());
        assert_eq!(
            route(&wheel(1, vec![Modifier::Command]), &config),
            ScrollDecision::Zoom { zoom_in: true }
        );
        assert_eq!(
            route(&wheel(-1, vec![Modifier::Command]), &config),
            ScrollDecision::Zoom { zoom_in: false }
        );
        assert_eq!(
            route(&wheel(0, vec![Modifier::Command]), &config),
            ScrollDecision::Swallow
        );
    }

    #[test]
    fn zoom_ignores_reverse_direction() {
        // Wheel away from you always zooms in, whatever the scroll direction setting.
        let config = config(
            false,
            ScrollModifierConfig {
                reverse_direction: true,
                ..ScrollModifierConfig::default()
            },
        );
        assert_eq!(
            route(&wheel(1, vec![Modifier::Command]), &config),
            ScrollDecision::Zoom { zoom_in: true }
        );
    }

    #[test]
    fn precise_modifier_with_smoothing_sets_the_flag() {
        let config = config(true, ScrollModifierConfig::default());
        assert_eq!(
            route(&wheel(1, vec![Modifier::Option]), &config),
            ScrollDecision::Smooth {
                vertical: 1.0,
                horizontal: 0.0,
                precise: true
            }
        );
    }

    #[test]
    fn precise_modifier_without_smoothing_scales_down() {
        let config = config(false, ScrollModifierConfig::default());
        assert_eq!(
            route(&wheel(1, vec![Modifier::Option]), &config),
            ScrollDecision::Rewrite {
                lines: Axes {
                    vertical: 1,
                    horizontal: 0
                },
                fixed: Axes {
                    vertical: 0.25,
                    horizontal: 0.0
                },
                points: Axes {
                    vertical: 3,
                    horizontal: 0
                },
                clear_modifiers: vec![Modifier::Option],
            }
        );
    }

    #[test]
    fn unconfigured_modifiers_are_ignored() {
        let config = config(false, no_modifiers());
        assert_eq!(
            route(&wheel(1, vec![Modifier::Shift, Modifier::Command]), &config),
            ScrollDecision::PassThrough
        );
    }

    #[test]
    fn scale_keeping_sign_never_rounds_to_zero() {
        assert_eq!(scale_keeping_sign(1, 0.25), 1);
        assert_eq!(scale_keeping_sign(-1, 0.25), -1);
        assert_eq!(scale_keeping_sign(8, 0.25), 2);
        assert_eq!(scale_keeping_sign(0, 0.25), 0);
    }
}
