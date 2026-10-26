//! Jaycast core: everything that decides or does something lives here.
//! The Swift shell draws UI and forwards user intent through the FFI below.

uniffi::setup_scaffolding!();

mod clipboard;
mod config;
mod display;
mod error;
mod files;
mod input;
mod launcher;
mod pasteboard;
mod snippets;
mod windows;

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use clipboard::ClipboardHistory;
pub use clipboard::{ClipboardItem, ClipboardKind};
pub use config::*;
use display::{BrightnessKeyTap, DisplayManager};
pub use display::{DisplayInfo, DisplayObserver};
pub use error::CoreError;
pub use files::FileResult;
use files::FileSearch;
use input::InputEngine;
pub use launcher::{AppInfo, LauncherObserver, OpenOutcome, PaletteMode, ResultKind, SearchResult};
use launcher::{HotkeyManager, Launcher};
use snippets::SnippetExpander;
use windows::WindowManager;

/// Basic facts about the core, shown in the shell so the FFI is exercised end to end.
#[derive(Debug, Clone, uniffi::Record)]
pub struct CoreInfo {
    pub name: String,
    pub version: String,
}

#[uniffi::export]
pub fn core_info() -> CoreInfo {
    CoreInfo {
        name: env!("CARGO_PKG_NAME").to_owned(),
        version: env!("CARGO_PKG_VERSION").to_owned(),
    }
}

/// The default window hotkeys and gap, for the shell's "Restore Defaults".
#[uniffi::export]
pub fn default_window_config() -> WindowConfig {
    WindowConfig::default()
}

/// The app's single core instance. The shell creates one at launch and keeps it.
#[derive(uniffi::Object)]
pub struct Core {
    store: ConfigStore,
    config: Mutex<Config>,
    input: InputEngine,
    displays: Arc<DisplayManager>,
    brightness_keys: BrightnessKeyTap,
    launcher: Arc<Launcher>,
    hotkeys: HotkeyManager,
    clipboard: Arc<ClipboardHistory>,
    files: FileSearch,
    windows: Arc<WindowManager>,
    snippets: SnippetExpander,
}

#[uniffi::export]
impl Core {
    /// `data_dir` is where settings live (the shell passes Application Support/Jaycast).
    #[uniffi::constructor]
    pub fn new(data_dir: String) -> Result<Arc<Self>, CoreError> {
        let data_dir = PathBuf::from(data_dir);
        let store = ConfigStore::new(&data_dir);
        let config = store.load()?;
        let input = InputEngine::new(config.mouse.clone());
        let displays = DisplayManager::new(config.monitor.clone());
        let brightness_keys = BrightnessKeyTap::new(Arc::clone(&displays));
        let windows = Arc::new(WindowManager::new(config.launcher.windows.clone()));
        let launcher = Launcher::new(&data_dir, config.launcher.clone(), Arc::clone(&windows));
        let hotkeys = HotkeyManager::new(Arc::clone(&windows));
        let clipboard = ClipboardHistory::new(&data_dir, config.launcher.clipboard.clone());
        let files = FileSearch::new(config.launcher.files.clone());
        let snippets = SnippetExpander::new(config.launcher.snippets.clone());
        Ok(Arc::new(Self {
            store,
            config: Mutex::new(config),
            input,
            displays,
            brightness_keys,
            launcher,
            hotkeys,
            clipboard,
            files,
            windows,
            snippets,
        }))
    }

    pub fn mouse_config(&self) -> MouseConfig {
        self.config.lock().unwrap().mouse.clone()
    }

    /// Saves the new mouse settings and applies them immediately if input is running.
    pub fn set_mouse_config(&self, mouse: MouseConfig) -> Result<(), CoreError> {
        let mut config = self.config.lock().unwrap();
        config.mouse = mouse.clone();
        self.store.save(&config)?;
        self.input.set_config(mouse);
        Ok(())
    }

    pub fn monitor_config(&self) -> MonitorConfig {
        self.config.lock().unwrap().monitor.clone()
    }

    /// Saves the new monitor settings and applies them immediately.
    pub fn set_monitor_config(&self, monitor: MonitorConfig) -> Result<(), CoreError> {
        let mut config = self.config.lock().unwrap();
        config.monitor = monitor.clone();
        self.store.save(&config)?;
        self.displays.set_config(monitor);
        Ok(())
    }

    /// External displays with their cached brightness. Never blocks on DDC.
    pub fn displays(&self) -> Vec<DisplayInfo> {
        self.displays.displays()
    }

    /// Sets a display's brightness (0.0–1.0). Returns at once; the DDC write is coalesced
    /// on a background thread.
    pub fn set_brightness(&self, display_id: u32, brightness: f64) -> Result<(), CoreError> {
        self.displays.set_brightness(display_id, brightness)
    }

    /// Registers the shell's observer for display and brightness changes (`None` clears it).
    pub fn set_display_observer(&self, observer: Option<Arc<dyn DisplayObserver>>) {
        self.displays.set_observer(observer);
    }

    pub fn launcher_config(&self) -> LauncherConfig {
        self.config.lock().unwrap().launcher.clone()
    }

    /// Saves the new launcher settings and re-registers hotkeys. Main thread only.
    pub fn set_launcher_config(&self, launcher: LauncherConfig) -> Result<(), CoreError> {
        let mut config = self.config.lock().unwrap();
        config.launcher = launcher.clone();
        self.store.save(&config)?;
        launcher::apply_config(&self.hotkeys, &launcher);
        self.clipboard.set_config(launcher.clipboard.clone());
        self.files.set_config(launcher.files.clone());
        self.windows.set_config(launcher.windows.clone());
        self.snippets.set_config(launcher.snippets.clone());
        self.launcher.set_config(launcher);
        Ok(())
    }

    /// Registers the saved hotkeys. Call once at launch, on the main thread, after
    /// `set_launcher_observer`.
    pub fn start_hotkeys(&self) {
        launcher::apply_config(&self.hotkeys, &self.launcher_config());
    }

    /// Hotkeys that couldn't be registered: duplicates, or combinations another app owns.
    pub fn failed_hotkeys(&self) -> Vec<Hotkey> {
        self.hotkeys.failed()
    }

    /// Suspends all global hotkeys while a shortcut recorder is listening, so pressing an
    /// existing hotkey records it instead of firing it. Main thread only.
    pub fn set_hotkeys_paused(&self, paused: bool) {
        self.hotkeys.set_paused(paused);
    }

    /// Registers the shell's observer for the palette hotkey and app index changes.
    pub fn set_launcher_observer(&self, observer: Option<Arc<dyn LauncherObserver>>) {
        self.launcher.set_observer(observer.clone());
        self.clipboard.set_observer(observer.clone());
        self.hotkeys.set_observer(observer);
    }

    /// Best matches for `query` (apps, system and window actions, snippets, …). Cheap enough
    /// for every keystroke.
    pub fn search(&self, query: String, limit: u32) -> Vec<SearchResult> {
        self.launcher.search(&query, limit as usize)
    }

    /// Opens a search result. `query` is what was typed, so ranking learns which result
    /// each query leads to.
    pub fn open_result(&self, id: String, query: String) -> Result<OpenOutcome, CoreError> {
        self.launcher.open(&id, &query)
    }

    /// Clipboard history: pinned first, then newest; `query` filters by text.
    pub fn clipboard_items(&self, query: String, limit: u32) -> Vec<ClipboardItem> {
        self.clipboard.items(&query, limit as usize)
    }

    /// Puts an item back on the clipboard and, if `paste`, pastes it into the frontmost app.
    /// Close the palette first so the paste lands in the right place.
    pub fn paste_clipboard_item(&self, id: i64, paste: bool) -> Result<(), CoreError> {
        self.clipboard.paste(id, paste)
    }

    pub fn delete_clipboard_item(&self, id: i64) -> Result<(), CoreError> {
        self.clipboard.delete(id)
    }

    pub fn set_clipboard_item_pinned(&self, id: i64, pinned: bool) -> Result<(), CoreError> {
        self.clipboard.set_pinned(id, pinned)
    }

    /// Deletes every unpinned item.
    pub fn clear_clipboard_history(&self) -> Result<(), CoreError> {
        self.clipboard.clear()
    }

    /// Spotlight file search in the configured folders. Blocks briefly: call it off the
    /// main thread.
    pub fn search_files(&self, query: String, limit: u32) -> Result<Vec<FileResult>, CoreError> {
        self.files.search(&query, limit as usize)
    }

    pub fn open_file(&self, path: String) -> Result<(), CoreError> {
        self.files.open(&path)
    }

    pub fn reveal_file(&self, path: String) -> Result<(), CoreError> {
        self.files.reveal(&path)
    }

    /// Every indexed app, sorted by name.
    pub fn installed_apps(&self) -> Vec<AppInfo> {
        self.launcher.apps()
    }

    /// Starts the event taps (mouse, brightness keys and snippet expansion). Fails with
    /// `PermissionDenied` without Accessibility. Calling it while already running is a no-op.
    pub fn start_input(&self) -> Result<(), CoreError> {
        self.input.start()?;
        self.brightness_keys.start()?;
        self.snippets.start()
    }

    pub fn stop_input(&self) {
        self.input.stop();
        self.brightness_keys.stop();
        self.snippets.stop();
    }

    /// True only when the mouse and brightness key taps are running (the snippet tap only
    /// runs while expansion is on).
    pub fn is_input_running(&self) -> bool {
        self.input.is_running() && self.brightness_keys.is_running()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn core_info_reports_crate_version() {
        let info = core_info();
        assert_eq!(info.name, "jaycast-core");
        assert_eq!(info.version, env!("CARGO_PKG_VERSION"));
    }
}
