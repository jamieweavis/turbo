//! Keep Awake: keeps the Mac (and optionally the display) awake with an IOKit power
//! assertion, the same mechanism `caffeinate` uses. The assertion belongs to the process,
//! so macOS drops it if Jaycast quits or crashes; whether it's on is saved in
//! `<data_dir>/keep-awake.json` and taken again at launch, so an update or relaunch doesn't
//! turn it off.

use parking_lot::Mutex;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use objc2_core_foundation::CFString;

use crate::config::KeepAwakeConfig;
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
const ASSERTION_NAME: &str = "Jaycast Keep Awake";

/// What a Keep Awake hotkey or palette command does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum KeepAwakeCommand {
    Toggle,
    On,
    Off,
}

impl KeepAwakeCommand {
    /// Whether Keep Awake should be on after this command, given whether it is now.
    pub fn resolve(self, active: bool) -> bool {
        match self {
            Self::Toggle => !active,
            Self::On => true,
            Self::Off => false,
        }
    }
}

pub(crate) struct KeepAwake {
    path: PathBuf,
    state: Mutex<State>,
}

/// What `keep-awake.json` holds.
#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(default)]
struct Saved {
    active: bool,
}

struct State {
    keep_display_on: bool,
    /// The held assertion while Keep Awake is on.
    assertion: Option<IOPMAssertionID>,
}

impl KeepAwake {
    /// Turns back on if it was on when Jaycast last quit; a missing or unreadable file
    /// leaves it off.
    pub fn new(data_dir: &Path, config: KeepAwakeConfig) -> Self {
        let path = data_dir.join("keep-awake.json");
        let saved: Saved = fs::read(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();
        let assertion = if saved.active && config.enabled {
            create(config.keep_display_on)
                .inspect_err(|error| log::error!("couldn't restore Keep Awake: {error}"))
                .ok()
        } else {
            None
        };
        Self {
            path,
            state: Mutex::new(State {
                keep_display_on: config.keep_display_on,
                assertion,
            }),
        }
    }

    pub fn is_active(&self) -> bool {
        self.state.lock().assertion.is_some()
    }

    pub fn set_active(&self, active: bool) -> Result<(), CoreError> {
        let mut state = self.state.lock();
        match (active, state.assertion) {
            (true, None) => state.assertion = Some(create(state.keep_display_on)?),
            (false, Some(id)) => {
                release(id);
                state.assertion = None;
            }
            _ => return Ok(()),
        }
        log::info!("Keep Awake {}", if active { "on" } else { "off" });
        self.save(active)
    }

    /// Writes atomically, like the config.
    fn save(&self, active: bool) -> Result<(), CoreError> {
        if let Some(dir) = self.path.parent() {
            fs::create_dir_all(dir)?;
        }
        let tmp = self.path.with_extension("json.tmp");
        fs::write(&tmp, serde_json::to_vec_pretty(&Saved { active })?)?;
        fs::rename(&tmp, &self.path)?;
        Ok(())
    }

    /// Applied live: switching "keep display on" while active swaps the assertion.
    /// Turning Keep Awake off as a feature also stops keeping the Mac awake.
    pub fn set_config(&self, config: KeepAwakeConfig) -> Result<(), CoreError> {
        if !config.enabled {
            self.set_active(false)?;
        }
        let mut state = self.state.lock();
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

impl Drop for KeepAwake {
    fn drop(&mut self) {
        if let Some(id) = self.state.get_mut().assertion.take() {
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
        log::error!("couldn't release the Keep Awake assertion ({status:#x})");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commands_resolve() {
        assert!(KeepAwakeCommand::Toggle.resolve(false));
        assert!(!KeepAwakeCommand::Toggle.resolve(true));
        assert!(KeepAwakeCommand::On.resolve(true));
        assert!(KeepAwakeCommand::On.resolve(false));
        assert!(!KeepAwakeCommand::Off.resolve(true));
        assert!(!KeepAwakeCommand::Off.resolve(false));
    }

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("jaycast-test-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    /// Holds a real assertion for a moment; harmless, and released on every path.
    #[test]
    fn turns_on_and_off_and_swaps_assertion_kind() {
        let keep_awake = KeepAwake::new(&temp_dir("keep-awake"), KeepAwakeConfig::default());
        assert!(!keep_awake.is_active());
        keep_awake.set_active(true).unwrap();
        assert!(keep_awake.is_active());
        keep_awake.set_active(true).unwrap();
        keep_awake
            .set_config(KeepAwakeConfig {
                keep_display_on: false,
                ..KeepAwakeConfig::default()
            })
            .unwrap();
        assert!(keep_awake.is_active());
        keep_awake.set_active(false).unwrap();
        assert!(!keep_awake.is_active());
    }

    #[test]
    fn comes_back_on_after_a_relaunch() {
        let dir = temp_dir("keep-awake-relaunch");
        let keep_awake = KeepAwake::new(&dir, KeepAwakeConfig::default());
        keep_awake.set_active(true).unwrap();
        drop(keep_awake);
        let relaunched = KeepAwake::new(&dir, KeepAwakeConfig::default());
        assert!(relaunched.is_active());
        relaunched.set_active(false).unwrap();
        drop(relaunched);
        assert!(!KeepAwake::new(&dir, KeepAwakeConfig::default()).is_active());
    }
}
