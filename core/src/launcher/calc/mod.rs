//! STUB — to be implemented: calculator, unit and currency conversion.

use std::path::Path;

use super::{ResultKind, SearchResult};

pub(crate) struct Calculator {}

/// A successful evaluation, shown as the palette's top result.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Calculation {
    /// Formatted for reading: `1,234.5`, `3.11 mi`.
    pub display: String,
    /// What ⏎ copies: `1234.5`, `3.11`.
    pub copy_text: String,
    /// The normalised input, shown as the subtitle: `5 km in miles`.
    pub description: String,
}

impl Calculation {
    pub fn result(&self) -> SearchResult {
        SearchResult {
            id: format!("calc:{}", self.copy_text),
            title: self.display.clone(),
            subtitle: self.description.clone(),
            kind: ResultKind::Calculator,
            path: None,
            symbol: Some("equal.circle.fill".into()),
            needs_confirmation: false,
            completion: None,
        }
    }
}

impl Calculator {
    /// Starts loading cached currency rates (and refreshing stale ones) in the background;
    /// `data_dir` holds the cache. Returns immediately.
    pub fn new(data_dir: &Path) -> Self {
        let _ = data_dir;
        Self {}
    }

    /// `None` unless the whole query is an expression worth answering (a bare number or a
    /// word isn't). Pure and fast: called on every keystroke, never touches the network.
    pub fn evaluate(&self, query: &str) -> Option<Calculation> {
        let _ = query;
        None
    }
}
