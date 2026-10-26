//! STUB — to be implemented: clipboard history (monitor, SQLite store, paste).

#![allow(dead_code)]

use std::path::Path;
use std::sync::{Arc, Mutex};

use crate::config::ClipboardConfig;
use crate::error::CoreError;
use crate::launcher::LauncherObserver;

#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct ClipboardItem {
    pub id: i64,
    pub kind: ClipboardKind,
    /// The text (for text and links), or file names (for files), or a description of the
    /// image ("PNG image, 1200 × 800").
    pub text: String,
    /// A stored copy of the image, for previews (images only).
    pub image_path: Option<String>,
    /// Copied file paths (files only).
    pub file_paths: Vec<String>,
    /// The app that was frontmost when it was copied.
    pub source_app: Option<String>,
    pub source_bundle_id: Option<String>,
    /// Seconds since 1970; the most recent copy for re-copied items.
    pub copied_at: f64,
    /// Pinned items never expire and sort first.
    pub pinned: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum ClipboardKind {
    Text,
    Link,
    Image,
    Files,
}

pub(crate) struct ClipboardHistory {
    config: Mutex<ClipboardConfig>,
    observer: Mutex<Option<Arc<dyn LauncherObserver>>>,
}

impl ClipboardHistory {
    /// Opens (or creates) the store in `data_dir` and starts monitoring on a background
    /// thread if enabled. Returns immediately.
    pub fn new(data_dir: &Path, config: ClipboardConfig) -> Arc<Self> {
        let _ = data_dir;
        Arc::new(Self {
            config: Mutex::new(config),
            observer: Mutex::new(None),
        })
    }

    /// Applies limits and the enabled switch live (pruning straight away if limits shrink).
    pub fn set_config(&self, config: ClipboardConfig) {
        *self.config.lock().unwrap() = config;
    }

    /// Receives `clipboard_changed`.
    pub fn set_observer(&self, observer: Option<Arc<dyn LauncherObserver>>) {
        *self.observer.lock().unwrap() = observer;
    }

    /// Pinned first, then newest first; `query` filters by text (case-insensitive, fuzzy
    /// enough to be useful). Fast: no pasteboard access.
    pub fn items(&self, query: &str, limit: usize) -> Vec<ClipboardItem> {
        let _ = (query, limit);
        Vec::new()
    }

    /// Puts the item back on the clipboard and, if `paste`, sends ⌘V to the frontmost app
    /// (needs Accessibility). The shell closes the palette first.
    pub fn paste(&self, id: i64, paste: bool) -> Result<(), CoreError> {
        let _ = (id, paste);
        Err(CoreError::Other {
            message: "clipboard history is not implemented yet".into(),
        })
    }

    pub fn delete(&self, id: i64) -> Result<(), CoreError> {
        let _ = id;
        Ok(())
    }

    pub fn set_pinned(&self, id: i64, pinned: bool) -> Result<(), CoreError> {
        let _ = (id, pinned);
        Ok(())
    }

    /// Deletes every unpinned item.
    pub fn clear(&self) -> Result<(), CoreError> {
        Ok(())
    }
}
