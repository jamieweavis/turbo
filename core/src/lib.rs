//! Jaycast core: everything that decides or does something lives here.
//! The Swift shell draws UI and forwards user intent through the FFI below.

uniffi::setup_scaffolding!();

mod config;
mod error;
mod input;

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

pub use config::*;
pub use error::CoreError;
use input::InputEngine;

/// Basic facts about the core, shown in the shell so the FFI is exercised end to end.
#[derive(Debug, Clone, uniffi::Record)]
pub struct CoreInfo {
    pub name: String,
    pub version: String,
}

#[uniffi::export]
pub fn core_info() -> CoreInfo {
    CoreInfo {
        name: env!("CARGO_PKG_NAME").to_owned(),
        version: env!("CARGO_PKG_VERSION").to_owned(),
    }
}

/// The app's single core instance. The shell creates one at launch and keeps it.
#[derive(uniffi::Object)]
pub struct Core {
    store: ConfigStore,
    config: Mutex<Config>,
    input: InputEngine,
}

#[uniffi::export]
impl Core {
    /// `data_dir` is where settings live (the shell passes Application Support/Jaycast).
    #[uniffi::constructor]
    pub fn new(data_dir: String) -> Result<Arc<Self>, CoreError> {
        let store = ConfigStore::new(&PathBuf::from(data_dir));
        let config = store.load()?;
        let input = InputEngine::new(config.mouse.clone());
        Ok(Arc::new(Self {
            store,
            config: Mutex::new(config),
            input,
        }))
    }

    pub fn mouse_config(&self) -> MouseConfig {
        self.config.lock().unwrap().mouse.clone()
    }

    /// Saves the new mouse settings and applies them immediately if input is running.
    pub fn set_mouse_config(&self, mouse: MouseConfig) -> Result<(), CoreError> {
        let mut config = self.config.lock().unwrap();
        config.mouse = mouse.clone();
        self.store.save(&config)?;
        self.input.set_config(mouse);
        Ok(())
    }

    /// Starts intercepting mouse input. Fails with `PermissionDenied` without Accessibility.
    /// Calling it while already running is a no-op.
    pub fn start_input(&self) -> Result<(), CoreError> {
        self.input.start()
    }

    pub fn stop_input(&self) {
        self.input.stop();
    }

    pub fn is_input_running(&self) -> bool {
        self.input.is_running()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn core_info_reports_crate_version() {
        let info = core_info();
        assert_eq!(info.name, "jaycast-core");
        assert_eq!(info.version, env!("CARGO_PKG_VERSION"));
    }
}
