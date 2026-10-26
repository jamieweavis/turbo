//! Jaycast core: everything that decides or does something lives here.
//! The Swift shell draws UI and forwards user intent through the FFI below.

uniffi::setup_scaffolding!();

mod config;
mod display;
mod error;
mod input;

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

pub use config::*;
use display::{BrightnessKeyTap, DisplayManager};
pub use display::{DisplayInfo, DisplayObserver};
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
    displays: Arc<DisplayManager>,
    brightness_keys: BrightnessKeyTap,
}

#[uniffi::export]
impl Core {
    /// `data_dir` is where settings live (the shell passes Application Support/Jaycast).
    #[uniffi::constructor]
    pub fn new(data_dir: String) -> Result<Arc<Self>, CoreError> {
        let store = ConfigStore::new(&PathBuf::from(data_dir));
        let config = store.load()?;
        let input = InputEngine::new(config.mouse.clone());
        let displays = DisplayManager::new(config.monitor.clone());
        let brightness_keys = BrightnessKeyTap::new(Arc::clone(&displays));
        Ok(Arc::new(Self {
            store,
            config: Mutex::new(config),
            input,
            displays,
            brightness_keys,
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

    pub fn monitor_config(&self) -> MonitorConfig {
        self.config.lock().unwrap().monitor.clone()
    }

    /// Saves the new monitor settings and applies them immediately.
    pub fn set_monitor_config(&self, monitor: MonitorConfig) -> Result<(), CoreError> {
        let mut config = self.config.lock().unwrap();
        config.monitor = monitor.clone();
        self.store.save(&config)?;
        self.displays.set_config(monitor);
        Ok(())
    }

    /// External displays with their cached brightness. Never blocks on DDC.
    pub fn displays(&self) -> Vec<DisplayInfo> {
        self.displays.displays()
    }

    /// Sets a display's brightness (0.0–1.0). Returns at once; the DDC write is coalesced
    /// on a background thread.
    pub fn set_brightness(&self, display_id: u32, brightness: f64) -> Result<(), CoreError> {
        self.displays.set_brightness(display_id, brightness)
    }

    /// Registers the shell's observer for display and brightness changes (`None` clears it).
    pub fn set_display_observer(&self, observer: Option<Arc<dyn DisplayObserver>>) {
        self.displays.set_observer(observer);
    }

    /// Starts the event taps (mouse and brightness keys). Fails with `PermissionDenied`
    /// without Accessibility. Calling it while already running is a no-op.
    pub fn start_input(&self) -> Result<(), CoreError> {
        self.input.start()?;
        self.brightness_keys.start()
    }

    pub fn stop_input(&self) {
        self.input.stop();
        self.brightness_keys.stop();
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
