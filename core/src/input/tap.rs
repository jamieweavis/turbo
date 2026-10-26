//! STUB — to be implemented: the CGEventTap on its own thread + CFRunLoop.

use std::sync::Mutex;

use crate::config::MouseConfig;
use crate::error::CoreError;

pub(crate) struct EventTap {
    config: Mutex<MouseConfig>,
}

impl EventTap {
    pub fn new(config: MouseConfig) -> Self {
        Self {
            config: Mutex::new(config),
        }
    }

    pub fn set_config(&self, config: MouseConfig) {
        *self.config.lock().unwrap() = config;
    }

    pub fn start(&self) -> Result<(), CoreError> {
        Ok(())
    }

    pub fn stop(&self) {}

    pub fn is_running(&self) -> bool {
        false
    }
}
