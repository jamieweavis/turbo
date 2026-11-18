//! The Hyper key: one key (Caps Lock, or a left or right modifier) that acts as ⌃⌥⇧⌘ while
//! held, so it can start shortcuts no app uses. Pressed and released on its own, it can
//! send Escape or toggle Caps Lock instead.
//!
//! - `remap`: remapping the chosen key in the HID event system, and toggling Caps Lock.
//! - `machine`: what happens to each event while Hyper is or isn't held (pure).
//! - `tap`: the HID-level tap that applies it.
//! - `power`: sleep/wake notifications, so a press cut short by sleep doesn't stick.
//!
//! The key is only remapped while the tap runs, so it never goes dead: with Accessibility
//! revoked, input stopped or the setting off, it's back to being itself.

mod machine;
mod power;
mod remap;
mod tap;

use parking_lot::Mutex;

use crate::config::HyperKeyConfig;
use crate::error::CoreError;
use tap::HyperTap;

#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    fn AXIsProcessTrusted() -> u8;
}

pub(crate) struct HyperKey {
    state: Mutex<State>,
    tap: HyperTap,
}

struct State {
    /// `start` was called (and `stop` hasn't been since).
    started: bool,
    config: HyperKeyConfig,
}

impl HyperKey {
    /// Also removes a remapping left behind if Backpack didn't get to quit cleanly.
    pub fn new(config: HyperKeyConfig) -> Self {
        if let Err(error) = remap::apply(None) {
            log::error!("couldn't restore the Hyper key: {error}");
        }
        Self {
            tap: HyperTap::new(config.clone()),
            state: Mutex::new(State {
                started: false,
                config,
            }),
        }
    }

    /// Applied live: choosing another key remaps it at once, and `None` gives it back.
    pub fn set_config(&self, config: HyperKeyConfig) -> Result<(), CoreError> {
        let mut state = self.state.lock();
        self.tap.set_config(config.clone());
        state.config = config;
        self.sync(&state)
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
            self.release();
        }
        result
    }

    pub fn stop(&self) {
        let mut state = self.state.lock();
        state.started = false;
        self.release();
    }

    /// Runs the tap and remaps the key exactly when wanted. The tap starts first and stops
    /// last, so the remapped key is never left without anything turning it into Hyper.
    fn sync(&self, state: &State) -> Result<(), CoreError> {
        match state.config.active_key() {
            Some(key) if state.started => {
                self.tap.start()?;
                remap::apply(Some(key))
            }
            _ => {
                self.release();
                Ok(())
            }
        }
    }

    fn release(&self) {
        if let Err(error) = remap::apply(None) {
            log::error!("couldn't restore the Hyper key: {error}");
        }
        self.tap.stop();
    }
}

impl Drop for HyperKey {
    fn drop(&mut self) {
        self.release();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Never starts the tap, so it leaves the real keyboard alone.
    #[test]
    fn nothing_runs_until_started() {
        let config = HyperKeyConfig {
            enabled: true,
            ..HyperKeyConfig::default()
        };
        let hyper = HyperKey::new(config.clone());
        assert!(!hyper.tap.is_running());
        hyper.set_config(config).unwrap();
        assert!(!hyper.tap.is_running());
        hyper.stop();
        assert!(!hyper.tap.is_running());
    }
}
