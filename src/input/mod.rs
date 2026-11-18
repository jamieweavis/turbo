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

use parking_lot::Mutex;

use crate::config::MouseConfig;
use crate::error::CoreError;

pub(crate) use actions::modifier_flag;

#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    fn AXIsProcessTrusted() -> u8;
}

/// Written to `kCGEventSourceUserData` on every event Backpack posts, so the tap can let
/// its own events through untouched instead of processing them again.
pub(crate) const SYNTHETIC_EVENT_MARKER: i64 = 0x4A41_5943; // "JAYC"

pub(crate) struct InputEngine {
    state: Mutex<State>,
    tap: tap::EventTap,
}

struct State {
    /// `start` was called (and `stop` hasn't been since).
    started: bool,
    /// With the mouse off the tap doesn't run, so mouse events never pass through Backpack.
    enabled: bool,
}

impl InputEngine {
    pub fn new(config: MouseConfig) -> Self {
        Self {
            state: Mutex::new(State {
                started: false,
                enabled: config.enabled,
            }),
            tap: tap::EventTap::new(config.effective()),
        }
    }

    /// Applied live: turning the mouse off stops the tap.
    pub fn set_config(&self, config: MouseConfig) {
        let mut state = self.state.lock();
        state.enabled = config.enabled;
        self.tap.set_config(config.effective());
        if let Err(error) = self.sync(&state) {
            log::error!("couldn't start the mouse event tap: {error}");
        }
    }

    /// Fails with `PermissionDenied` without Accessibility. A no-op while running.
    pub fn start(&self) -> Result<(), CoreError> {
        let mut state = self.state.lock();
        // SAFETY: no arguments; reads the process's TCC state.
        if unsafe { AXIsProcessTrusted() } == 0 {
            return Err(CoreError::PermissionDenied);
        }
        state.started = true;
        let result = self.sync(&state);
        if result.is_err() {
            state.started = false;
        }
        result
    }

    pub fn stop(&self) {
        self.state.lock().started = false;
        self.tap.stop();
    }

    /// Started, whether or not the tap itself is running (it doesn't while the mouse is off).
    pub fn is_running(&self) -> bool {
        self.state.lock().started
    }

    /// Runs the tap exactly when it's wanted.
    fn sync(&self, state: &State) -> Result<(), CoreError> {
        if state.started && state.enabled {
            self.tap.start()
        } else {
            self.tap.stop();
            Ok(())
        }
    }
}
