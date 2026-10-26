//! STUB — to be implemented: file search through Spotlight, scoped to chosen folders.

#![allow(dead_code)]

use std::sync::Mutex;

use crate::config::FileSearchConfig;
use crate::error::CoreError;

#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct FileResult {
    pub path: String,
    /// Finder's display name.
    pub name: String,
    /// The containing folder, abbreviated with `~`.
    pub parent: String,
    /// Uniform type identifier, e.g. `com.adobe.pdf`, `public.folder`.
    pub content_type: String,
    /// Seconds since 1970.
    pub modified: f64,
}

pub(crate) struct FileSearch {
    config: Mutex<FileSearchConfig>,
}

impl FileSearch {
    pub fn new(config: FileSearchConfig) -> Self {
        Self {
            config: Mutex::new(config),
        }
    }

    pub fn set_config(&self, config: FileSearchConfig) {
        *self.config.lock().unwrap() = config;
    }

    /// Spotlight matches for `query` inside the configured folders, best first. Blocks for
    /// the query (bounded to a fraction of a second): call it off the main thread.
    pub fn search(&self, query: &str, limit: usize) -> Result<Vec<FileResult>, CoreError> {
        let _ = (query, limit);
        Ok(Vec::new())
    }

    /// Opens the file with its default app.
    pub fn open(&self, path: &str) -> Result<(), CoreError> {
        let _ = path;
        Ok(())
    }

    /// Selects the file in a Finder window.
    pub fn reveal(&self, path: &str) -> Result<(), CoreError> {
        let _ = path;
        Ok(())
    }
}
