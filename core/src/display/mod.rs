//! External display brightness over DDC/CI, and the brightness keys that drive it.
//!
//! Module ownership (so parallel work doesn't collide):
//! - `mod.rs`, `ddc`: display enumeration, hotplug, cached brightness and the DDC writers.
//! - `keys`: the brightness-key event tap, which calls [`DisplayManager::adjust`].

mod keys;

use std::sync::{Arc, Mutex};

use crate::config::MonitorConfig;
use crate::error::CoreError;

pub(crate) use keys::BrightnessKeyTap;

/// An external display Jaycast knows about.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct DisplayInfo {
    /// The `CGDirectDisplayID`; stable while the display stays connected.
    pub id: u32,
    pub name: String,
    /// Whether brightness can be read and written over DDC/CI.
    pub supports_ddc: bool,
    /// 0.0–1.0, normalised from the monitor's own range. Meaningless without DDC.
    pub brightness: f64,
}

/// Lets the shell react to display changes. Called from background threads.
#[uniffi::export(with_foreign)]
pub trait DisplayObserver: Send + Sync {
    /// A display was connected or disconnected, or finished its initial DDC read.
    fn displays_changed(&self, displays: Vec<DisplayInfo>);
    /// Brightness changed. `from_keys` is true when the brightness keys did it, so the shell
    /// shows the on-screen indicator.
    fn brightness_changed(&self, display_id: u32, brightness: f64, from_keys: bool);
}

/// STUB — to be implemented: enumeration, hotplug and DDC.
pub(crate) struct DisplayManager {
    config: Mutex<MonitorConfig>,
    observer: Mutex<Option<Arc<dyn DisplayObserver>>>,
}

impl DisplayManager {
    pub fn new(config: MonitorConfig) -> Arc<Self> {
        Arc::new(Self {
            config: Mutex::new(config),
            observer: Mutex::new(None),
        })
    }

    pub fn set_config(&self, config: MonitorConfig) {
        *self.config.lock().unwrap() = config;
    }

    pub fn config(&self) -> MonitorConfig {
        self.config.lock().unwrap().clone()
    }

    pub fn set_observer(&self, observer: Option<Arc<dyn DisplayObserver>>) {
        *self.observer.lock().unwrap() = observer;
    }

    /// The external displays, with cached brightness (never blocks on DDC).
    pub fn displays(&self) -> Vec<DisplayInfo> {
        Vec::new()
    }

    /// Sets brightness (clamped to 0.0–1.0). Returns immediately; the DDC write happens on a
    /// background thread, coalescing rapid changes (e.g. slider drags) to the latest value.
    pub fn set_brightness(&self, display_id: u32, brightness: f64) -> Result<(), CoreError> {
        let _ = (display_id, brightness);
        Err(CoreError::Other {
            message: "display brightness is not implemented yet".into(),
        })
    }

    /// Nudges brightness by `delta` (e.g. ±1/16) for the brightness keys. Returns the new
    /// value, or `None` if this display can't be controlled, so the key should pass through.
    /// Must not block: the key tap calls it on its hot path. Notifies the observer with
    /// `from_keys: true`.
    pub fn adjust(&self, display_id: u32, delta: f64) -> Option<f64> {
        let _ = (display_id, delta);
        None
    }
}
