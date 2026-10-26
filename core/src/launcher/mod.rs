//! The launcher: app index, fuzzy search ranked by usage, system actions and hotkeys.
//!
//! Module ownership (so parallel work doesn't collide):
//! - `mod.rs`, `apps`, `fuzzy`, `ranking`: the app index (with change watching), matching,
//!   usage-based ranking, and `search` / `open`.
//! - `system`: the system actions (lock, sleep, dark mode, …) listed by `system::actions`
//!   and run by `system::perform`.
//! - `hotkeys`: global hotkeys (palette + per-app) and the focus-or-hide app toggle.

mod hotkeys;
mod system;

use std::path::Path;
use std::sync::{Arc, Mutex};

use crate::config::LauncherConfig;
use crate::error::CoreError;

pub(crate) use hotkeys::HotkeyManager;

#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct SearchResult {
    /// Opaque; pass back to `Core::open_result`. Apps use `app:<bundle id>`, system actions
    /// `system:<action id>`.
    pub id: String,
    pub title: String,
    pub subtitle: String,
    pub kind: ResultKind,
    /// The app bundle's path, for the shell to draw its icon.
    pub path: Option<String>,
    /// An SF Symbol name, for results without a bundle (system actions).
    pub symbol: Option<String>,
    /// Destructive or disruptive (restart, empty trash, …): the shell confirms first.
    pub needs_confirmation: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum ResultKind {
    Application,
    SystemAction,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct AppInfo {
    pub bundle_id: String,
    pub name: String,
    pub path: String,
}

/// Lets the shell react to launcher events. Called on the main thread for hotkeys, and from
/// a background thread for `apps_changed`.
#[uniffi::export(with_foreign)]
pub trait LauncherObserver: Send + Sync {
    /// The palette hotkey was pressed.
    fn toggle_palette(&self);
    /// The app index changed (an app was installed, removed or renamed).
    fn apps_changed(&self);
}

/// STUB — to be implemented: app index, matching and ranking.
pub(crate) struct Launcher {
    observer: Mutex<Option<Arc<dyn LauncherObserver>>>,
}

impl Launcher {
    /// Starts indexing on a background thread; `data_dir` holds the usage history.
    pub fn new(data_dir: &Path) -> Arc<Self> {
        let _ = data_dir;
        Arc::new(Self {
            observer: Mutex::new(None),
        })
    }

    pub fn set_observer(&self, observer: Option<Arc<dyn LauncherObserver>>) {
        *self.observer.lock().unwrap() = observer;
    }

    /// Best matches for `query`, apps and system actions together. An empty query returns
    /// the most used. Fast enough to call on every keystroke (no I/O).
    pub fn search(&self, query: &str, limit: usize) -> Vec<SearchResult> {
        let _ = (query, limit);
        Vec::new()
    }

    /// Opens a result from `search` and records the choice for ranking.
    pub fn open(&self, id: &str, query: &str) -> Result<(), CoreError> {
        let _ = (id, query);
        Err(CoreError::Other {
            message: "the launcher is not implemented yet".into(),
        })
    }

    /// Every indexed app, sorted by name (for choosing app hotkeys in settings).
    pub fn apps(&self) -> Vec<AppInfo> {
        Vec::new()
    }
}

/// Applies a new launcher config to the hotkeys.
pub(crate) fn apply_config(hotkeys: &HotkeyManager, config: &LauncherConfig) {
    hotkeys.apply(config);
}
