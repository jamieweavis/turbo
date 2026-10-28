//! Moving and resizing a window by dragging anywhere in it with modifiers held: by default
//! ⌃⌘ with the left button moves it and with the right button resizes it.
//!
//! - `geometry`: where the window goes for a drag (pure, unit-tested).
//! - `tap`: the session-level tap that decides which clicks start a drag and swallows them.
//! - `worker`: the Accessibility calls, off the tap thread.

mod geometry;
mod tap;
mod worker;

use parking_lot::Mutex;
use std::sync::Arc;

use super::WindowManager;
use super::ax;
use crate::config::WindowDragConfig;
use crate::error::CoreError;
use tap::DragTap;

pub(crate) struct WindowDrag {
    state: Mutex<State>,
    tap: DragTap,
}

struct State {
    /// `start` was called (and `stop` hasn't been since).
    started: bool,
    config: WindowDragConfig,
}

impl WindowDrag {
    /// Shares `windows` so a dragged window can be put back with `Restore`.
    pub fn new(config: WindowDragConfig, windows: Arc<WindowManager>) -> Self {
        Self {
            tap: DragTap::new(config.clone(), windows),
            state: Mutex::new(State {
                started: false,
                config,
            }),
        }
    }

    /// Applied live: turning it off stops the tap, so clicks are untouched.
    pub fn set_config(&self, config: WindowDragConfig) -> Result<(), CoreError> {
        let mut state = self.state.lock();
        self.tap.set_config(config.clone());
        state.config = config;
        self.sync(&state)
    }

    /// Fails with `PermissionDenied` without Accessibility. A no-op while running.
    pub fn start(&self) -> Result<(), CoreError> {
        let mut state = self.state.lock();
        // SAFETY: no arguments; reads the process's TCC state.
        if unsafe { ax::AXIsProcessTrusted() } == 0 {
            return Err(CoreError::PermissionDenied);
        }
        state.started = true;
        let result = self.sync(&state);
        if result.is_err() {
            state.started = false;
            self.tap.stop();
        }
        result
    }

    pub fn stop(&self) {
        self.state.lock().started = false;
        self.tap.stop();
    }

    /// The tap only runs while started and on, so it costs nothing otherwise.
    fn sync(&self, state: &State) -> Result<(), CoreError> {
        if state.started && state.config.is_active() {
            self.tap.start()
        } else {
            self.tap.stop();
            Ok(())
        }
    }
}
