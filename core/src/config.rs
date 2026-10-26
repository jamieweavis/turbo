//! User settings, persisted as JSON in the app's data directory.
//!
//! Every struct uses `#[serde(default)]` so configs written by older builds keep loading
//! as fields are added.

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::CoreError;

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub mouse: MouseConfig,
    pub monitor: MonitorConfig,
    pub launcher: LauncherConfig,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, uniffi::Record)]
#[serde(default)]
pub struct LauncherConfig {
    /// Opens the palette. `None` means no global hotkey (the menu bar still opens it).
    pub hotkey: Option<Hotkey>,
    /// Hotkeys that focus an app (launching it if needed), or hide it if it's already in front.
    pub app_hotkeys: Vec<AppHotkey>,
    /// Show a calculator / unit / currency result when the query is an expression.
    pub calculator: bool,
    pub quicklinks: Vec<Quicklink>,
    pub commands: Vec<ShellCommand>,
    pub clipboard: ClipboardConfig,
    pub files: FileSearchConfig,
}

impl Default for LauncherConfig {
    fn default() -> Self {
        Self {
            hotkey: Some(Hotkey {
                key_code: KEY_SPACE,
                modifiers: vec![Modifier::Option],
            }),
            app_hotkeys: Vec::new(),
            calculator: true,
            quicklinks: default_quicklinks(),
            commands: Vec::new(),
            clipboard: ClipboardConfig::default(),
            files: FileSearchConfig::default(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, uniffi::Record)]
#[serde(default)]
pub struct ClipboardConfig {
    /// Record clipboard history. Off also stops monitoring entirely.
    pub enabled: bool,
    /// Opens the palette in clipboard history.
    pub hotkey: Option<Hotkey>,
    /// Oldest unpinned items are dropped beyond this many.
    pub max_items: u32,
    /// Unpinned items older than this are dropped.
    pub retention_days: u32,
    /// Bundle ids whose copies are never recorded (password managers by default). Copies
    /// marked concealed or transient by the source app are never recorded either.
    pub ignored_apps: Vec<String>,
    /// Choosing an item pastes it into the frontmost app (⌘V); otherwise it's only copied.
    pub paste_on_open: bool,
}

impl Default for ClipboardConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            hotkey: Some(Hotkey {
                key_code: KEY_V,
                modifiers: vec![Modifier::Option, Modifier::Command],
            }),
            max_items: 500,
            retention_days: 30,
            ignored_apps: [
                "com.1password.1password",
                "com.agilebits.onepassword7",
                "com.bitwarden.desktop",
                "com.apple.keychainaccess",
                "com.apple.Passwords",
            ]
            .map(String::from)
            .to_vec(),
            paste_on_open: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, uniffi::Record)]
#[serde(default)]
pub struct FileSearchConfig {
    /// Opens the palette in file search.
    pub hotkey: Option<Hotkey>,
    /// Folders searched (`~` allowed). Spotlight indexes more, but results stay in these.
    pub scopes: Vec<String>,
}

impl Default for FileSearchConfig {
    fn default() -> Self {
        Self {
            hotkey: None,
            scopes: ["~/Desktop", "~/Documents", "~/Downloads"]
                .map(String::from)
                .to_vec(),
        }
    }
}

fn default_quicklinks() -> Vec<Quicklink> {
    let link = |id: &str, name: &str, keyword: &str, url: &str| Quicklink {
        id: id.into(),
        name: name.into(),
        keyword: keyword.into(),
        url: url.into(),
    };
    vec![
        link(
            "google",
            "Search Google",
            "g",
            "https://www.google.com/search?q={query}",
        ),
        link(
            "youtube",
            "Search YouTube",
            "yt",
            "https://www.youtube.com/results?search_query={query}",
        ),
        link(
            "github",
            "Search GitHub",
            "gh",
            "https://github.com/search?q={query}",
        ),
    ]
}

/// A URL template opened from the palette. Placeholders: `{query}` (the text typed after the
/// keyword, URL-encoded) and `{clipboard}` (URL-encoded).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, uniffi::Record)]
pub struct Quicklink {
    /// Stable and unique (the shell generates a UUID for new ones).
    pub id: String,
    pub name: String,
    /// Typed before the argument: `g rust traits`. May be empty (then found by name only).
    pub keyword: String,
    pub url: String,
}

/// A shell command run from the palette with `/bin/zsh -lc`. Placeholders: `{query}` (the text
/// typed after the keyword, shell-quoted) and `{clipboard}` (shell-quoted).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, uniffi::Record)]
pub struct ShellCommand {
    pub id: String,
    pub name: String,
    pub keyword: String,
    pub command: String,
    pub output: CommandOutput,
    pub needs_confirmation: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, uniffi::Enum)]
pub enum CommandOutput {
    /// Run in the background; only failures are reported.
    Discard,
    /// Show stdout/stderr in a window when it finishes.
    Show,
    /// Copy stdout to the clipboard when it finishes.
    Copy,
}

/// `kVK_Space`.
pub const KEY_SPACE: u16 = 0x31;
/// `kVK_ANSI_V`.
pub const KEY_V: u16 = 0x09;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, uniffi::Record)]
pub struct Hotkey {
    /// macOS virtual key code (`kVK_*`).
    pub key_code: u16,
    pub modifiers: Vec<Modifier>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, uniffi::Record)]
pub struct AppHotkey {
    pub bundle_id: String,
    /// Shown in settings; refreshed from the app index when it's installed.
    pub name: String,
    pub hotkey: Hotkey,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, uniffi::Record)]
#[serde(default)]
pub struct MonitorConfig {
    /// Master switch: brightness control for external displays over DDC/CI.
    pub enabled: bool,
    /// Brightness keys adjust the external display under the cursor. On the built-in display
    /// (or one without DDC) they keep their normal behaviour.
    pub brightness_keys: bool,
}

impl Default for MonitorConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            brightness_keys: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, uniffi::Record)]
#[serde(default)]
pub struct MouseConfig {
    /// Master switch for everything in this struct.
    pub enabled: bool,
    pub smooth_scroll: SmoothScrollConfig,
    pub scroll_modifiers: ScrollModifierConfig,
    pub buttons: Vec<ButtonBinding>,
}

impl Default for MouseConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            smooth_scroll: SmoothScrollConfig::default(),
            scroll_modifiers: ScrollModifierConfig::default(),
            buttons: vec![
                ButtonBinding {
                    button: BUTTON_BACK,
                    trigger: Trigger::Click,
                    action: Action::Back,
                },
                ButtonBinding {
                    button: BUTTON_FORWARD,
                    trigger: Trigger::Click,
                    action: Action::Forward,
                },
                ButtonBinding {
                    button: BUTTON_MIDDLE,
                    trigger: Trigger::Hold,
                    action: Action::MissionControl,
                },
            ],
        }
    }
}

/// CoreGraphics button numbers (`kCGMouseEventButtonNumber`): 0 left, 1 right, 2 middle, 3+ extra.
pub const BUTTON_MIDDLE: u32 = 2;
pub const BUTTON_BACK: u32 = 3;
pub const BUTTON_FORWARD: u32 = 4;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, uniffi::Record)]
#[serde(default)]
pub struct SmoothScrollConfig {
    pub enabled: bool,
    pub smoothness: Smoothness,
    /// Multiplier on the distance scrolled per wheel tick; 1.0 is the default feel.
    pub speed: f64,
    /// Fast consecutive ticks scroll progressively further.
    pub acceleration: bool,
}

impl Default for SmoothScrollConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            smoothness: Smoothness::Regular,
            speed: 1.0,
            acceleration: true,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, uniffi::Enum)]
pub enum Smoothness {
    Low,
    Regular,
    High,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, uniffi::Record)]
#[serde(default)]
pub struct ScrollModifierConfig {
    /// Flip the wheel's direction without touching the trackpad's natural scrolling.
    pub reverse_direction: bool,
    /// Held while scrolling: scroll horizontally.
    pub horizontal: Option<Modifier>,
    /// Held while scrolling: zoom in/out.
    pub zoom: Option<Modifier>,
    /// Held while scrolling: small, unsmoothed steps for fine positioning.
    pub precise: Option<Modifier>,
}

impl Default for ScrollModifierConfig {
    fn default() -> Self {
        Self {
            reverse_direction: false,
            horizontal: Some(Modifier::Shift),
            zoom: Some(Modifier::Command),
            precise: Some(Modifier::Option),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, uniffi::Enum)]
pub enum Modifier {
    Shift,
    Control,
    Option,
    Command,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, uniffi::Record)]
pub struct ButtonBinding {
    pub button: u32,
    pub trigger: Trigger,
    pub action: Action,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, uniffi::Enum)]
pub enum Trigger {
    Click,
    DoubleClick,
    Hold,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, uniffi::Enum)]
pub enum Action {
    Back,
    Forward,
    MissionControl,
    AppExpose,
    ShowDesktop,
    Launchpad,
    PreviousSpace,
    NextSpace,
    /// Post the original button's click, e.g. to keep middle click while middle hold is bound.
    MiddleClick,
    KeyboardShortcut {
        /// macOS virtual key code (`kVK_*`).
        key_code: u16,
        modifiers: Vec<Modifier>,
    },
}

/// Loads and saves [`Config`] at `<data_dir>/config.json`.
pub struct ConfigStore {
    path: PathBuf,
}

impl ConfigStore {
    pub fn new(data_dir: &Path) -> Self {
        Self {
            path: data_dir.join("config.json"),
        }
    }

    /// A missing file yields the defaults; an unreadable one is an error rather than
    /// silently discarding the user's settings.
    pub fn load(&self) -> Result<Config, CoreError> {
        match fs::read(&self.path) {
            Ok(bytes) => Ok(serde_json::from_slice(&bytes)?),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Config::default()),
            Err(error) => Err(error.into()),
        }
    }

    /// Writes atomically (temp file + rename) so a crash never leaves a half-written config.
    pub fn save(&self, config: &Config) -> Result<(), CoreError> {
        if let Some(dir) = self.path.parent() {
            fs::create_dir_all(dir)?;
        }
        let tmp = self.path.with_extension("json.tmp");
        fs::write(&tmp, serde_json::to_vec_pretty(config)?)?;
        fs::rename(&tmp, &self.path)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("jaycast-test-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn missing_file_loads_defaults() {
        let store = ConfigStore::new(&temp_dir("missing"));
        assert_eq!(store.load().unwrap(), Config::default());
    }

    #[test]
    fn save_then_load_round_trips() {
        let dir = temp_dir("roundtrip");
        let store = ConfigStore::new(&dir);
        let mut config = Config::default();
        config.mouse.smooth_scroll.speed = 1.5;
        config.mouse.buttons.push(ButtonBinding {
            button: 5,
            trigger: Trigger::DoubleClick,
            action: Action::KeyboardShortcut {
                key_code: 0x31,
                modifiers: vec![Modifier::Command, Modifier::Shift],
            },
        });
        store.save(&config).unwrap();
        assert_eq!(store.load().unwrap(), config);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn partial_file_fills_in_defaults() {
        let dir = temp_dir("partial");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("config.json"), r#"{"mouse":{"enabled":false}}"#).unwrap();
        let config = ConfigStore::new(&dir).load().unwrap();
        assert!(!config.mouse.enabled);
        assert_eq!(config.mouse.smooth_scroll, SmoothScrollConfig::default());
        fs::remove_dir_all(dir).unwrap();
    }
}
