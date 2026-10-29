//! Software dimming below the monitor's own minimum, so the bottom of the range reaches black.
//!
//! The brightness range is split: the top part drives DDC (monitor backlight), and the bottom
//! [`DIM_RANGE`] holds DDC at 0 while scaling the display's gamma down to black. macOS
//! restores every gamma table when the process exits, so a crash can't leave a screen dark.

use parking_lot::Mutex;
use std::collections::HashMap;

use objc2_core_graphics::{
    CGDirectDisplayID, CGDisplayRestoreColorSyncSettings, CGSetDisplayTransferByFormula,
};

/// The share of the range below DDC 0; 0.25 is the bottom four brightness-key steps.
pub(super) const DIM_RANGE: f64 = 0.25;

/// How a combined brightness maps onto the two mechanisms.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct Split {
    /// 0.0–1.0 of the monitor's DDC range.
    pub ddc: f64,
    /// Gamma scale: 1.0 is untouched, 0.0 is black.
    pub gamma: f64,
}

pub(super) fn split(brightness: f64) -> Split {
    let brightness = brightness.clamp(0.0, 1.0);
    if brightness >= DIM_RANGE {
        Split {
            ddc: (brightness - DIM_RANGE) / (1.0 - DIM_RANGE),
            gamma: 1.0,
        }
    } else {
        Split {
            ddc: 0.0,
            gamma: brightness / DIM_RANGE,
        }
    }
}

/// The combined brightness for a DDC reading, assuming the display isn't software-dimmed
/// (true at startup: gamma never outlives the process).
pub(super) fn from_ddc(ddc: f64) -> f64 {
    DIM_RANGE + ddc.clamp(0.0, 1.0) * (1.0 - DIM_RANGE)
}

/// Tracks which displays are dimmed, so restoring one can re-apply the others.
#[derive(Default)]
pub(super) struct Dimmer {
    dimmed: Mutex<HashMap<CGDirectDisplayID, f64>>,
}

impl Dimmer {
    /// Applies `gamma` to a display. Cheap (no DDC), so safe on the key tap's hot path.
    pub fn set(&self, display: CGDirectDisplayID, gamma: f64) {
        let mut dimmed = self.dimmed.lock();
        if gamma < 1.0 {
            dimmed.insert(display, gamma);
            apply(display, gamma);
        } else if dimmed.remove(&display).is_some() {
            // There's no per-display restore: restore all, then re-dim the rest.
            restore_all(&dimmed);
        }
    }

    /// Forgets a disconnected display.
    pub fn forget(&self, display: CGDirectDisplayID) {
        self.dimmed.lock().remove(&display);
    }

    pub fn reset_all(&self) {
        let mut dimmed = self.dimmed.lock();
        if !dimmed.is_empty() {
            dimmed.clear();
            restore_all(&dimmed);
        }
    }
}

impl Drop for Dimmer {
    fn drop(&mut self) {
        self.reset_all();
    }
}

fn restore_all(still_dimmed: &HashMap<CGDirectDisplayID, f64>) {
    CGDisplayRestoreColorSyncSettings();
    for (&display, &gamma) in still_dimmed {
        apply(display, gamma);
    }
}

fn apply(display: CGDirectDisplayID, gamma: f64) {
    let max = gamma.clamp(0.0, 1.0) as f32;
    // Linear ramp from 0 to `max` on every channel.
    let _ = CGSetDisplayTransferByFormula(display, 0.0, max, 1.0, 0.0, max, 1.0, 0.0, max, 1.0);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn top_of_range_is_all_ddc() {
        assert_eq!(
            split(1.0),
            Split {
                ddc: 1.0,
                gamma: 1.0
            }
        );
        assert_eq!(
            split(DIM_RANGE),
            Split {
                ddc: 0.0,
                gamma: 1.0
            }
        );
    }

    #[test]
    fn bottom_of_range_dims_to_black() {
        assert_eq!(
            split(0.0),
            Split {
                ddc: 0.0,
                gamma: 0.0
            }
        );
        let half = split(DIM_RANGE / 2.0);
        assert_eq!(half.ddc, 0.0);
        assert!((half.gamma - 0.5).abs() < 1e-9);
    }

    #[test]
    fn ddc_readings_round_trip() {
        for ddc in [0.0, 0.3, 1.0] {
            assert!((split(from_ddc(ddc)).ddc - ddc).abs() < 1e-9);
        }
    }
}
