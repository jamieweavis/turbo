//! Mouse input: one event tap feeds button remapping, scroll modifiers and smooth scrolling.
//!
//! Module ownership (so parallel work doesn't collide):
//! - `tap`, `buttons`, `actions`, `scroll`: the event tap and everything it routes, except
//!   the smoothing itself.
//! - `scroll_physics` (pure, unit-tested) and `smooth_scroll` (posts the animated events):
//!   smooth scrolling. `scroll` hands wheel ticks to [`smooth_scroll::SmoothScroller::feed`].

mod actions;
mod buttons;
mod scroll;
mod scroll_physics;
mod smooth_scroll;
mod tap;

use crate::config::MouseConfig;
use crate::error::CoreError;

pub(crate) use actions::modifier_flag;

/// Written to `kCGEventSourceUserData` on every event Jaycast posts, so the tap can let
/// its own events through untouched instead of processing them again.
pub(crate) const SYNTHETIC_EVENT_MARKER: i64 = 0x4A41_5943; // "JAYC"

pub(crate) struct InputEngine {
    tap: tap::EventTap,
}

impl InputEngine {
    pub fn new(config: MouseConfig) -> Self {
        Self {
            tap: tap::EventTap::new(config.effective()),
        }
    }

    pub fn set_config(&self, config: MouseConfig) {
        self.tap.set_config(config.effective());
    }

    pub fn start(&self) -> Result<(), CoreError> {
        self.tap.start()
    }

    pub fn stop(&self) {
        self.tap.stop();
    }

    pub fn is_running(&self) -> bool {
        self.tap.is_running()
    }
}
