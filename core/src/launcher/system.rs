//! STUB — to be implemented: system actions.

#![allow(dead_code)]

use crate::error::CoreError;

/// A built-in system action, listed alongside apps in search.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct SystemAction {
    /// Stable id (`lock-screen`, `sleep`, …); search results use `system:<id>`.
    pub id: &'static str,
    pub title: &'static str,
    /// Extra words it should match ("lock" also finds "Lock Screen" via "secure", …).
    pub keywords: &'static [&'static str],
    /// SF Symbol shown in the palette.
    pub symbol: &'static str,
    pub needs_confirmation: bool,
}

pub(crate) fn actions() -> &'static [SystemAction] {
    &[]
}

/// Runs an action by id. Must not block for long: the shell calls it from the main thread.
pub(crate) fn perform(id: &str) -> Result<(), CoreError> {
    Err(CoreError::Other {
        message: format!("unknown system action {id}"),
    })
}
