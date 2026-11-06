//! Jaycast core: everything that decides or does something lives here.
//! The Swift shell draws UI and forwards user intent through the FFI below.

uniffi::setup_scaffolding!();

mod aux_keys;
mod camera;
mod clipboard;
mod color_picker;
mod config;
mod display;
mod emoji;
mod error;
mod files;
mod hyper;
mod input;
mod keep_awake;
mod launcher;
mod logging;
mod media_keys;
mod pasteboard;
mod snippets;
mod windows;

use parking_lot::Mutex;
use std::path::PathBuf;
use std::sync::Arc;

use camera::Cameras;
pub use camera::{CameraAccess, CameraDevice, CameraKind};
use clipboard::ClipboardHistory;
pub use clipboard::{ClipboardItem, ClipboardKind};
use color_picker::ColorHistory;
pub use color_picker::{PickedColor, format_color};
pub use config::*;
use display::{BrightnessKeyTap, DisplayManager};
pub use display::{DisplayInfo, DisplayObserver};
use emoji::EmojiPicker;
pub use emoji::{EmojiInfo, EmojiSection};
pub use error::CoreError;
pub use files::FileResult;
use files::FileSearch;
use hyper::HyperKey;
use input::InputEngine;
use keep_awake::KeepAwake;
pub use launcher::{
    AppInfo, LauncherItem, LauncherObserver, OpenOutcome, PaletteMode, ResultKind, SearchResult,
    SystemActionInfo, system_actions,
};
use launcher::{HotkeyManager, Launcher};
use media_keys::MediaKeys;
pub use media_keys::{AutomationAccess, MediaCommand, MediaPlayer};
use snippets::SnippetExpander;
use windows::{WindowDrag, WindowManager};

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
    saver: ConfigSaver,
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
    hyper_key: HyperKey,
    window_drag: WindowDrag,
    keep_awake: Arc<KeepAwake>,
    colors: ColorHistory,
    emoji: EmojiPicker,
    cameras: Cameras,
    media_keys: Arc<MediaKeys>,
}

#[uniffi::export]
impl Core {
    /// `data_dir` is where history and state live (the shell passes Application
    /// Support/Jaycast); the config itself is `~/.config/jaycast.toml`.
    #[uniffi::constructor]
    pub fn new(data_dir: String) -> Result<Arc<Self>, CoreError> {
        logging::init();
        log::info!(
            "starting {} {}",
            env!("CARGO_PKG_NAME"),
            env!("CARGO_PKG_VERSION")
        );
        let data_dir = PathBuf::from(data_dir);
        let saver = ConfigSaver::new(
            ConfigStore::new(ConfigStore::default_path()?, &data_dir),
            SAVE_DEBOUNCE,
        );
        let config = saver.load()?;
        config::log_non_defaults(&config);
        let input = InputEngine::new(config.mouse.clone());
        let displays = DisplayManager::new(config.monitor.clone());
        let brightness_keys = BrightnessKeyTap::new(Arc::clone(&displays));
        let windows = Arc::new(WindowManager::new(config.launcher.windows.clone()));
        let keep_awake = Arc::new(KeepAwake::new(
            &data_dir,
            config.launcher.keep_awake.clone(),
        ));
        let media_keys = Arc::new(MediaKeys::new(config.media_keys.clone()));
        let launcher = Launcher::new(
            &data_dir,
            config.launcher.clone(),
            Arc::clone(&windows),
            Arc::clone(&keep_awake),
            Arc::clone(&media_keys),
        );
        let hotkeys = HotkeyManager::new(
            Arc::clone(&windows),
            Arc::clone(&keep_awake),
            Arc::clone(&media_keys),
        );
        let clipboard = ClipboardHistory::new(&data_dir, config.launcher.clipboard.clone());
        let files = FileSearch::new(config.launcher.files.clone());
        let snippets = SnippetExpander::new(config.launcher.snippets.clone());
        let hyper_key = HyperKey::new(config.hyper_key.clone());
        let window_drag = WindowDrag::new(config.window_drag.clone(), Arc::clone(&windows));
        let colors = ColorHistory::new(&data_dir);
        let emoji = EmojiPicker::new(&data_dir);
        let cameras = Cameras::new(&data_dir);
        Ok(Arc::new(Self {
            saver,
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
            hyper_key,
            window_drag,
            keep_awake,
            colors,
            emoji,
            cameras,
            media_keys,
        }))
    }

    pub fn mouse_config(&self) -> MouseConfig {
        self.config.lock().mouse.clone()
    }

    /// Applies the new mouse settings immediately if input is running, and saves them.
    pub fn set_mouse_config(&self, mouse: MouseConfig) -> Result<(), CoreError> {
        let mut config = self.config.lock();
        let before = config.clone();
        config.mouse = mouse.clone();
        config::log_changes(&before, &config);
        self.input.set_config(mouse);
        self.saver.save(&config)
    }

    pub fn monitor_config(&self) -> MonitorConfig {
        self.config.lock().monitor.clone()
    }

    /// Applies the new monitor settings immediately, and saves them.
    pub fn set_monitor_config(&self, monitor: MonitorConfig) -> Result<(), CoreError> {
        let mut config = self.config.lock();
        let before = config.clone();
        config.monitor = monitor.clone();
        config::log_changes(&before, &config);
        let saved = self.saver.save(&config);
        self.displays.set_config(monitor);
        self.brightness_keys.refresh()?;
        saved
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

    /// The built-in display's brightness (0.0–1.0), or `None` when it isn't active (a
    /// closed MacBook, a desktop Mac) or can't be read.
    pub fn builtin_brightness(&self) -> Option<f64> {
        display::builtin::brightness()
    }

    /// Sets the built-in display's brightness (0.0–1.0).
    pub fn set_builtin_brightness(&self, brightness: f64) -> Result<(), CoreError> {
        if display::builtin::set_brightness(brightness) {
            Ok(())
        } else {
            Err(CoreError::Other {
                message: "couldn't set the built-in display's brightness".into(),
            })
        }
    }

    /// Registers the shell's observer for display and brightness changes (`None` clears it).
    pub fn set_display_observer(&self, observer: Option<Arc<dyn DisplayObserver>>) {
        self.displays.set_observer(observer);
    }

    pub fn hyper_key_config(&self) -> HyperKeyConfig {
        self.config.lock().hyper_key.clone()
    }

    /// Applies the new Hyper key settings immediately if input is running, and saves them.
    /// The settings are saved even if remapping the key fails.
    pub fn set_hyper_key_config(&self, hyper_key: HyperKeyConfig) -> Result<(), CoreError> {
        let mut config = self.config.lock();
        let before = config.clone();
        config.hyper_key = hyper_key.clone();
        config::log_changes(&before, &config);
        let saved = self.saver.save(&config);
        self.hyper_key.set_config(hyper_key)?;
        saved
    }

    pub fn window_drag_config(&self) -> WindowDragConfig {
        self.config.lock().window_drag.clone()
    }

    /// Applies the new window drag settings immediately if input is running, and saves them.
    pub fn set_window_drag_config(&self, window_drag: WindowDragConfig) -> Result<(), CoreError> {
        let mut config = self.config.lock();
        let before = config.clone();
        config.window_drag = window_drag.clone();
        config::log_changes(&before, &config);
        let saved = self.saver.save(&config);
        self.window_drag.set_config(window_drag)?;
        saved
    }

    pub fn media_keys_config(&self) -> MediaKeysConfig {
        self.config.lock().media_keys.clone()
    }

    /// Applies the new media key settings immediately (starting or stopping the key tap if
    /// input is running), and saves them.
    pub fn set_media_keys_config(&self, media_keys: MediaKeysConfig) -> Result<(), CoreError> {
        let mut config = self.config.lock();
        let before = config.clone();
        config.media_keys = media_keys.clone();
        config::log_changes(&before, &config);
        let saved = self.saver.save(&config);
        self.media_keys.set_config(media_keys)?;
        saved
    }

    /// The supported music apps that are installed, for choosing where media keys go.
    pub fn media_players(&self) -> Vec<MediaPlayer> {
        self.media_keys.players()
    }

    /// Sends play/pause, next or previous wherever the media keys would.
    pub fn perform_media_command(&self, command: MediaCommand) -> Result<(), CoreError> {
        self.media_keys.perform(command)
    }

    /// Whether Jaycast may control a player (macOS's Automation permission), without asking.
    /// Can block indefinitely: call it off the main thread.
    pub fn media_player_access(&self, bundle_id: String) -> AutomationAccess {
        self.media_keys.access(&bundle_id)
    }

    /// Shows macOS's Automation prompt for a running player if it hasn't been answered.
    /// Blocks until it is: call it off the main thread.
    pub fn request_media_player_access(&self, bundle_id: String) -> AutomationAccess {
        self.media_keys.request_access(&bundle_id)
    }

    pub fn launcher_config(&self) -> LauncherConfig {
        self.config.lock().launcher.clone()
    }

    /// Applies the new launcher settings and re-registers hotkeys, and saves them. Main
    /// thread only.
    pub fn set_launcher_config(&self, launcher: LauncherConfig) -> Result<(), CoreError> {
        let mut config = self.config.lock();
        let before = config.clone();
        config.launcher = launcher.clone();
        config::log_changes(&before, &config);
        let saved = self.saver.save(&config);
        launcher::apply_config(&self.hotkeys, &launcher);
        self.clipboard.set_config(launcher.clipboard.clone());
        self.files.set_config(launcher.files.clone());
        self.windows.set_config(launcher.windows.clone());
        self.snippets.set_config(launcher.snippets.clone());
        let keep_awake = self.keep_awake.set_config(launcher.keep_awake.clone());
        self.launcher.set_config(launcher);
        keep_awake?;
        saved
    }

    /// Writes settings changed in the last moment, which are otherwise saved once changes
    /// settle. The shell calls this before quitting.
    pub fn flush_config(&self) -> Result<(), CoreError> {
        self.saver.flush()
    }

    /// Whether Keep Awake is keeping the Mac awake.
    pub fn keep_awake_active(&self) -> bool {
        self.keep_awake.is_active()
    }

    /// Turns keeping the Mac awake on or off. It's remembered, so it comes back on when
    /// Jaycast relaunches (after an update, say).
    pub fn set_keep_awake_active(&self, active: bool) -> Result<(), CoreError> {
        self.keep_awake.set_active(active)
    }

    /// Picked colours, newest first.
    pub fn color_history(&self) -> Vec<PickedColor> {
        self.colors.colors()
    }

    /// Picked colours whose Hex, RGB or HSL text matches `query` (all of them when it's
    /// empty), newest first.
    pub fn search_color_history(&self, query: String) -> Vec<PickedColor> {
        self.colors.search(&query)
    }

    /// Records a colour the shell picked (sRGB) and copies it in the configured format.
    /// Returns the copied text.
    pub fn record_picked_color(&self, red: u8, green: u8, blue: u8) -> Result<String, CoreError> {
        let format = self.config.lock().launcher.color_picker.format;
        self.colors.record(red, green, blue, format)
    }

    /// Copies a colour from the history as `format`.
    pub fn copy_color(&self, color: PickedColor, format: ColorFormat) -> Result<(), CoreError> {
        pasteboard::copy(&format_color(color, format))
    }

    pub fn delete_picked_color(&self, color: PickedColor) -> Result<(), CoreError> {
        self.colors.delete(color)
    }

    pub fn clear_color_history(&self) -> Result<(), CoreError> {
        self.colors.clear()
    }

    /// Emoji in grid sections: recently used then every group for an empty query,
    /// otherwise one section of matches by name or shortcode.
    pub fn search_emoji(&self, query: String) -> Vec<EmojiSection> {
        self.emoji.search(&query)
    }

    /// Copies an emoji, or pastes it into the frontmost app (needs Accessibility; close the
    /// palette first), and remembers it as recently used.
    pub fn insert_emoji(&self, emoji: String, paste: bool) -> Result<(), CoreError> {
        self.emoji.insert(&emoji, paste)
    }

    /// The recently used emoji, newest first.
    pub fn recent_emoji(&self) -> Vec<EmojiInfo> {
        self.emoji.recent()
    }

    /// Connected cameras, built-in first, with the one Camera Preview shows marked
    /// `selected`. Quick enough to call whenever the preview opens or the menu is shown.
    pub fn cameras(&self) -> Vec<CameraDevice> {
        self.cameras.list()
    }

    /// The camera Camera Preview shows, or `None` when there's no camera.
    pub fn current_camera(&self) -> Option<CameraDevice> {
        self.cameras.current()
    }

    /// Makes `id` the camera Camera Preview shows, and remembers it (also while it's
    /// unplugged, so it's chosen again when it's back).
    pub fn select_camera(&self, id: String) -> Result<(), CoreError> {
        self.cameras.select(&id)
    }

    /// Whether Jaycast may use the camera.
    pub fn camera_access(&self) -> CameraAccess {
        camera::access()
    }

    /// Shows the system's camera prompt if it hasn't been answered, and returns whether
    /// access is granted.
    pub async fn request_camera_access(&self) -> bool {
        camera::request_access().await
    }

    /// Forgets one recently used emoji.
    pub fn remove_recent_emoji(&self, emoji: String) -> Result<(), CoreError> {
        self.emoji.remove_recent(&emoji)
    }

    /// Forgets every recently used emoji.
    pub fn clear_recent_emoji(&self) -> Result<(), CoreError> {
        self.emoji.clear_recent()
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

    /// Opens a snippet result, pasting it or (`paste` false) only copying it, whatever the
    /// snippets' paste setting says (the palette's ⌘↩).
    pub fn open_snippet(
        &self,
        id: String,
        query: String,
        paste: bool,
    ) -> Result<OpenOutcome, CoreError> {
        self.launcher.open_snippet(&id, &query, paste)
    }

    /// Forgets a result's usage history (the actions menu's Reset Ranking).
    pub fn reset_result_ranking(&self, id: String) -> Result<(), CoreError> {
        self.launcher.reset_ranking(&id)
    }

    /// Whether an app with this bundle id is running (to offer Hide and Quit).
    pub fn is_app_running(&self, bundle_id: String) -> bool {
        launcher::running::is_running(&bundle_id)
    }

    pub fn hide_app(&self, bundle_id: String) -> Result<(), CoreError> {
        launcher::running::hide(&bundle_id)
    }

    /// Asks the app to quit, or with `force` kills it without letting it save.
    pub fn quit_app(&self, bundle_id: String, force: bool) -> Result<(), CoreError> {
        launcher::running::quit(&bundle_id, force)
    }

    /// Tells the core whether the palette is showing, so snippet keywords typed into it
    /// don't expand. Call on every show and hide.
    pub fn set_palette_visible(&self, visible: bool) {
        self.snippets.set_palette_visible(visible);
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

    /// Every app and built-in command, for setting hotkeys, aliases and whether each shows in
    /// search.
    pub fn launcher_items(&self) -> Vec<LauncherItem> {
        self.launcher.items()
    }

    /// Starts the event taps (mouse, brightness keys, snippet expansion, the Hyper key,
    /// window dragging and media keys). Each tap only runs while its feature is on.
    /// Fails with `PermissionDenied` without Accessibility. Calling it while already running
    /// is a no-op.
    pub fn start_input(&self) -> Result<(), CoreError> {
        started("mouse", self.input.start())?;
        started("brightness key", self.brightness_keys.start())?;
        started("snippet", self.snippets.start())?;
        started("Hyper key", self.hyper_key.start())?;
        started("window drag", self.window_drag.start())?;
        started("media key", self.media_keys.start())?;
        log::info!("input taps started");
        Ok(())
    }

    /// Also gives the Hyper key back its normal behaviour; call it before quitting.
    pub fn stop_input(&self) {
        log::info!("stopping input taps");
        self.input.stop();
        self.brightness_keys.stop();
        self.snippets.stop();
        self.hyper_key.stop();
        self.window_drag.stop();
        self.media_keys.stop();
    }

    /// True once input has started (with Accessibility), whether or not any feature currently
    /// needs a tap running.
    pub fn is_input_running(&self) -> bool {
        self.input.is_running() && self.brightness_keys.is_running()
    }
}

fn started(tap: &str, result: Result<(), CoreError>) -> Result<(), CoreError> {
    result.inspect_err(|error| log::error!("couldn't start the {tap} tap: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn core_info_reports_crate_version() {
        let info = core_info();
        assert_eq!(info.name, "jaycast");
        assert_eq!(info.version, env!("CARGO_PKG_VERSION"));
    }
}
