//! STUB — to be implemented: snippets found in palette search, and keyword expansion
//! while typing in any app.

#![allow(dead_code)]

use std::sync::Mutex;

use crate::config::{Snippet, SnippetsConfig};
use crate::error::CoreError;
use crate::launcher::SearchResult;

/// Snippets matching `query` by name or keyword, scored like other launcher results
/// (`snippet:<id>`, `ResultKind::Snippet`). Pure and cheap: called on every keystroke.
pub(crate) fn results(query: &str, config: &SnippetsConfig) -> Vec<(i32, SearchResult)> {
    let _ = (query, config);
    Vec::new()
}

/// Expands the snippet's placeholders and types the text into the frontmost app, then
/// leaves the caret at `{cursor}`. Needs Accessibility. The shell closes the palette first.
pub(crate) fn insert(snippet: &Snippet) -> Result<(), CoreError> {
    let _ = snippet;
    Err(CoreError::Other {
        message: "snippets are not implemented yet".into(),
    })
}

/// A keyboard event tap that watches what's typed and replaces a snippet keyword with the
/// snippet as soon as the keyword is complete. Runs only while started and expansion is on.
pub(crate) struct SnippetExpander {
    config: Mutex<SnippetsConfig>,
}

impl SnippetExpander {
    pub fn new(config: SnippetsConfig) -> Self {
        Self {
            config: Mutex::new(config),
        }
    }

    /// Applied live: turning expansion off (or removing every keyword) stops the tap.
    pub fn set_config(&self, config: SnippetsConfig) {
        *self.config.lock().unwrap() = config;
    }

    /// Fails with `PermissionDenied` without Accessibility. A no-op while running.
    pub fn start(&self) -> Result<(), CoreError> {
        Ok(())
    }

    pub fn stop(&self) {}
}
