//! STUB — to be implemented: global hotkeys and the app focus-or-hide toggle.

use std::sync::{Arc, Mutex};

use super::LauncherObserver;
use crate::config::LauncherConfig;

/// Registers the palette and per-app hotkeys. Hotkeys don't need Accessibility.
/// `new` and `apply` must be called on the main thread.
pub(crate) struct HotkeyManager {
    observer: Mutex<Option<Arc<dyn LauncherObserver>>>,
}

impl HotkeyManager {
    pub fn new() -> Self {
        Self {
            observer: Mutex::new(None),
        }
    }

    /// Receives `toggle_palette` when the palette hotkey is pressed.
    pub fn set_observer(&self, observer: Option<Arc<dyn LauncherObserver>>) {
        *self.observer.lock().unwrap() = observer;
    }

    /// Replaces every registered hotkey with the ones in `config`.
    pub fn apply(&self, config: &LauncherConfig) {
        let _ = config;
    }
}
