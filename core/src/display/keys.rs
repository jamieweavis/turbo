//! STUB — to be implemented: event tap for the brightness keys.

#![allow(dead_code)]

use std::sync::Arc;

use super::DisplayManager;
use crate::error::CoreError;

pub(crate) struct BrightnessKeyTap {
    displays: Arc<DisplayManager>,
}

impl BrightnessKeyTap {
    pub fn new(displays: Arc<DisplayManager>) -> Self {
        Self { displays }
    }

    /// Fails with `PermissionDenied` without Accessibility. Idempotent.
    pub fn start(&self) -> Result<(), CoreError> {
        Ok(())
    }

    pub fn stop(&self) {}

    pub fn is_running(&self) -> bool {
        false
    }
}
