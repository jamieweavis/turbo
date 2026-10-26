//! STUB — to be implemented: quicklinks and shell commands.

use std::sync::Arc;

use super::{LauncherObserver, OpenOutcome, SearchResult};
use crate::config::LauncherConfig;
use crate::error::CoreError;

/// Scored results for `query` (raw, as typed). Ids are `quicklink:<id>` / `command:<id>`.
/// `<keyword> <argument>` scores `KEYWORD_SCORE`; name matches use fuzzy scores like apps.
pub(crate) fn results(query: &str, config: &LauncherConfig) -> Vec<(i32, SearchResult)> {
    let _ = (query, config);
    Vec::new()
}

/// Opens a quicklink or runs a command. `query` is what was typed, from which the argument
/// is taken. Must return quickly: commands run on a background thread and report through
/// `observer.command_finished`.
pub(crate) fn open(
    id: &str,
    query: &str,
    config: &LauncherConfig,
    observer: Option<Arc<dyn LauncherObserver>>,
) -> Result<OpenOutcome, CoreError> {
    let _ = (query, config, observer);
    Err(CoreError::Other {
        message: format!("unknown result {id}"),
    })
}
