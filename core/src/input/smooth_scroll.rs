//! STUB — to be implemented: animates wheel ticks into continuous, phased scroll events.

#![allow(dead_code)]

use crate::config::SmoothScrollConfig;

/// Owns its own emitter thread. All methods are cheap and safe to call from the tap thread.
pub(crate) struct SmoothScroller {}

impl SmoothScroller {
    pub fn new(config: SmoothScrollConfig) -> Self {
        let _ = config;
        Self {}
    }

    pub fn set_config(&self, config: SmoothScrollConfig) {
        let _ = config;
    }

    /// One wheel tick from a mouse (never a trackpad or our own events). Deltas are in lines,
    /// with macOS's sign convention, after the tap has applied direction and axis modifiers.
    /// `precise` asks for small, unsmoothed, momentum-free steps.
    pub fn feed(&self, vertical: f64, horizontal: f64, precise: bool) {
        let _ = (vertical, horizontal, precise);
    }

    /// Cancels any animation in flight, e.g. when the user clicks or input stops.
    pub fn stop(&self) {}
}
