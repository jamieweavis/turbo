//! Caffeinate: keeps the Mac (and optionally the display) awake with an IOKit power
//! assertion, the same mechanism `caffeinate` uses. The assertion belongs to the process,
//! so macOS drops it if Jaycast quits or crashes.

use std::sync::Mutex;

use objc2_core_foundation::CFString;

use crate::config::CaffeinateConfig;
use crate::error::CoreError;

type IOPMAssertionID = u32;

#[link(name = "IOKit", kind = "framework")]
unsafe extern "C" {
    fn IOPMAssertionCreateWithName(
        assertion_type: &CFString,
        level: u32,
        name: &CFString,
        id: *mut IOPMAssertionID,
    ) -> i32;
    fn IOPMAssertionRelease(id: IOPMAssertionID) -> i32;
}

/// `kIOPMAssertionLevelOn`.
const LEVEL_ON: u32 = 255;
/// `kIOPMAssertPreventUserIdleDisplaySleep`, which also keeps the system awake.
const PREVENT_DISPLAY_SLEEP: &str = "PreventUserIdleDisplaySleep";
/// `kIOPMAssertPreventUserIdleSystemSleep`.
const PREVENT_SYSTEM_SLEEP: &str = "PreventUserIdleSystemSleep";
const ASSERTION_NAME: &str = "Jaycast Caffeinate";

/// What a caffeinate hotkey does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CaffeinateCommand {
    Toggle,
    On,
    Off,
}

impl CaffeinateCommand {
    /// Whether caffeinate should be on after this command, given whether it is now.
    pub fn resolve(self, active: bool) -> bool {
        match self {
            Self::Toggle => !active,
            Self::On => true,
            Self::Off => false,
        }
    }
}

pub(crate) struct Caffeinate {
    state: Mutex<State>,
}

struct State {
    keep_display_on: bool,
    /// The held assertion while caffeinate is on.
    assertion: Option<IOPMAssertionID>,
}

impl Caffeinate {
    pub fn new(config: CaffeinateConfig) -> Self {
        Self {
            state: Mutex::new(State {
                keep_display_on: config.keep_display_on,
                assertion: None,
            }),
        }
    }

    pub fn is_active(&self) -> bool {
        self.state.lock().unwrap().assertion.is_some()
    }

    pub fn set_active(&self, active: bool) -> Result<(), CoreError> {
        let mut state = self.state.lock().unwrap();
        match (active, state.assertion) {
            (true, None) => state.assertion = Some(create(state.keep_display_on)?),
            (false, Some(id)) => {
                release(id);
                state.assertion = None;
            }
            _ => {}
        }
        Ok(())
    }

    /// Applied live: switching "keep display on" while active swaps the assertion.
    pub fn set_config(&self, config: CaffeinateConfig) -> Result<(), CoreError> {
        let mut state = self.state.lock().unwrap();
        if state.keep_display_on == config.keep_display_on {
            return Ok(());
        }
        state.keep_display_on = config.keep_display_on;
        if let Some(old) = state.assertion {
            // Take the new one before letting go of the old, so there's no gap.
            state.assertion = Some(create(config.keep_display_on)?);
            release(old);
        }
        Ok(())
    }
}

impl Drop for Caffeinate {
    fn drop(&mut self) {
        if let Some(id) = self.state.get_mut().unwrap().assertion.take() {
            release(id);
        }
    }
}

fn create(keep_display_on: bool) -> Result<IOPMAssertionID, CoreError> {
    let kind = if keep_display_on {
        PREVENT_DISPLAY_SLEEP
    } else {
        PREVENT_SYSTEM_SLEEP
    };
    let mut id = 0;
    // SAFETY: two valid CFStrings in, one assertion id out.
    let status = unsafe {
        IOPMAssertionCreateWithName(
            &CFString::from_str(kind),
            LEVEL_ON,
            &CFString::from_str(ASSERTION_NAME),
            &mut id,
        )
    };
    if status != 0 {
        return Err(CoreError::Other {
            message: format!("Couldn't keep the Mac awake (IOKit error {status:#x})"),
        });
    }
    Ok(id)
}

fn release(id: IOPMAssertionID) {
    // SAFETY: `id` came from a successful `IOPMAssertionCreateWithName` and is released once.
    let status = unsafe { IOPMAssertionRelease(id) };
    if status != 0 {
        eprintln!("jaycast: couldn't release the caffeinate assertion ({status:#x})");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commands_resolve() {
        assert!(CaffeinateCommand::Toggle.resolve(false));
        assert!(!CaffeinateCommand::Toggle.resolve(true));
        assert!(CaffeinateCommand::On.resolve(true));
        assert!(CaffeinateCommand::On.resolve(false));
        assert!(!CaffeinateCommand::Off.resolve(true));
        assert!(!CaffeinateCommand::Off.resolve(false));
    }

    /// Holds a real assertion for a moment; harmless, and released on every path.
    #[test]
    fn turns_on_and_off_and_swaps_assertion_kind() {
        let caffeinate = Caffeinate::new(CaffeinateConfig::default());
        assert!(!caffeinate.is_active());
        caffeinate.set_active(true).unwrap();
        assert!(caffeinate.is_active());
        caffeinate.set_active(true).unwrap();
        caffeinate
            .set_config(CaffeinateConfig {
                keep_display_on: false,
                ..CaffeinateConfig::default()
            })
            .unwrap();
        assert!(caffeinate.is_active());
        caffeinate.set_active(false).unwrap();
        assert!(!caffeinate.is_active());
    }
}
