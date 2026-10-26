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
    pub hyper_key: HyperKeyConfig,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, uniffi::Record)]
#[serde(default)]
pub struct LauncherConfig {
    /// Opens the palette. `None` means no global hotkey (the menu bar still opens it).
    pub hotkey: Option<Hotkey>,
    /// Hotkeys that focus an app (launching it if needed), or hide it if it's already in front.
    pub app_hotkeys: Vec<AppHotkey>,
    /// Hotkeys that run a system action (Lock Screen, Sleep, …) straight away.
    pub action_hotkeys: Vec<ActionHotkey>,
    /// Show a calculator / unit / currency result when the query is an expression.
    pub calculator: bool,
    pub quicklinks: Vec<Quicklink>,
    pub commands: Vec<ShellCommand>,
    pub clipboard: ClipboardConfig,
    pub files: FileSearchConfig,
    pub windows: WindowConfig,
    pub snippets: SnippetsConfig,
    pub caffeinate: CaffeinateConfig,
    pub color_picker: ColorPickerConfig,
}

impl Default for LauncherConfig {
    fn default() -> Self {
        Self {
            hotkey: Some(Hotkey {
                key_code: KEY_SPACE,
                modifiers: vec![Modifier::Option],
            }),
            app_hotkeys: Vec::new(),
            action_hotkeys: Vec::new(),
            calculator: true,
            quicklinks: default_quicklinks(),
            commands: Vec::new(),
            clipboard: ClipboardConfig::default(),
            files: FileSearchConfig::default(),
            windows: WindowConfig::default(),
            snippets: SnippetsConfig::default(),
            caffeinate: CaffeinateConfig::default(),
            color_picker: ColorPickerConfig::default(),
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
    /// Oldest unpinned items are dropped beyond this many; 0 keeps any number.
    pub max_items: u32,
    /// Unpinned items older than this many days are dropped; 0 keeps them forever.
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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, uniffi::Record)]
#[serde(default)]
pub struct WindowConfig {
    /// Register the window hotkeys. The actions stay in palette search either way.
    pub hotkeys_enabled: bool,
    /// Points left between snapped windows and between windows and the screen edges.
    pub gap: u32,
    /// Percent of the visible frame Almost Maximize fills, in each dimension.
    pub almost_maximize_percent: u32,
    pub hotkeys: Vec<WindowHotkey>,
}

impl Default for WindowConfig {
    fn default() -> Self {
        use WindowAction::*;
        let control_option = || vec![Modifier::Control, Modifier::Option];
        let all_three = || vec![Modifier::Control, Modifier::Option, Modifier::Command];
        let bind = |action, key_code, modifiers| WindowHotkey {
            action,
            hotkey: Hotkey {
                key_code,
                modifiers,
            },
        };
        Self {
            hotkeys_enabled: true,
            gap: 0,
            almost_maximize_percent: 90,
            hotkeys: vec![
                bind(LeftHalf, KEY_LEFT_ARROW, control_option()),
                bind(RightHalf, KEY_RIGHT_ARROW, control_option()),
                bind(TopHalf, KEY_UP_ARROW, control_option()),
                bind(BottomHalf, KEY_DOWN_ARROW, control_option()),
                bind(TopLeft, KEY_U, control_option()),
                bind(TopRight, KEY_I, control_option()),
                bind(BottomLeft, KEY_J, control_option()),
                bind(BottomRight, KEY_K, control_option()),
                bind(LeftThird, KEY_D, control_option()),
                bind(CenterThird, KEY_F, control_option()),
                bind(RightThird, KEY_G, control_option()),
                bind(LeftTwoThirds, KEY_E, control_option()),
                bind(RightTwoThirds, KEY_T, control_option()),
                bind(Maximize, KEY_RETURN, control_option()),
                bind(Center, KEY_C, control_option()),
                bind(Restore, KEY_DELETE, control_option()),
                bind(NextDisplay, KEY_RIGHT_ARROW, all_three()),
                bind(PreviousDisplay, KEY_LEFT_ARROW, all_three()),
            ],
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, uniffi::Record)]
pub struct WindowHotkey {
    pub action: WindowAction,
    pub hotkey: Hotkey,
}

/// Moves or resizes the focused window of the frontmost app. Positions are within the
/// display's visible frame (below the menu bar, beside the Dock).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, uniffi::Enum)]
pub enum WindowAction {
    /// Repeating a half cycles its width (or height) through 1/2, 2/3 and 1/3.
    LeftHalf,
    RightHalf,
    TopHalf,
    BottomHalf,
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
    LeftThird,
    CenterThird,
    RightThird,
    LeftTwoThirds,
    RightTwoThirds,
    /// Sixths: a grid of two rows of three columns.
    TopLeftSixth,
    TopCenterSixth,
    TopRightSixth,
    BottomLeftSixth,
    BottomCenterSixth,
    BottomRightSixth,
    Maximize,
    /// Maximised, inset to leave a margin of the desktop showing.
    AlmostMaximize,
    /// Same size, centred.
    Center,
    /// Back to the frame it had before Jaycast first moved it.
    Restore,
    /// Same relative position and size on the next display.
    NextDisplay,
    PreviousDisplay,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, uniffi::Record)]
#[serde(default)]
pub struct SnippetsConfig {
    /// Expand a snippet's keyword as soon as it's typed in any app (needs Accessibility).
    /// Snippets stay in palette search either way.
    pub expansion: bool,
    pub snippets: Vec<Snippet>,
}

impl Default for SnippetsConfig {
    fn default() -> Self {
        Self {
            expansion: true,
            snippets: Vec::new(),
        }
    }
}

/// Keeping the Mac awake. Whether it's on isn't saved: like `caffeinate`, it ends when
/// Jaycast quits.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, uniffi::Record)]
#[serde(default)]
pub struct CaffeinateConfig {
    /// Keep the display on too, not just the system awake.
    pub keep_display_on: bool,
    pub toggle_hotkey: Option<Hotkey>,
    pub on_hotkey: Option<Hotkey>,
    pub off_hotkey: Option<Hotkey>,
}

impl Default for CaffeinateConfig {
    fn default() -> Self {
        Self {
            keep_display_on: true,
            toggle_hotkey: None,
            on_hotkey: None,
            off_hotkey: None,
        }
    }
}

/// Picking a colour from anywhere on screen. The history lives beside the config, in
/// `colors.json`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, uniffi::Record)]
#[serde(default)]
pub struct ColorPickerConfig {
    /// Shows the magnifier to pick a colour.
    pub hotkey: Option<Hotkey>,
    /// How a picked colour is copied.
    pub format: ColorFormat,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, uniffi::Enum)]
pub enum ColorFormat {
    #[default]
    Hex,
    Rgb,
    Hsl,
}

/// Text inserted by keyword or from the palette. Placeholders: `{clipboard}`, `{date}`,
/// `{time}`, `{datetime}` (the user's locale formats) and `{cursor}` (where the caret ends up).
/// `{{` and `}}` are literal braces.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, uniffi::Record)]
pub struct Snippet {
    pub id: String,
    pub name: String,
    /// Typed anywhere to expand, e.g. `;addr`. May be empty (then palette only).
    pub keyword: String,
    pub text: String,
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
pub const KEY_C: u16 = 0x08;
pub const KEY_D: u16 = 0x02;
pub const KEY_E: u16 = 0x0E;
pub const KEY_F: u16 = 0x03;
pub const KEY_G: u16 = 0x05;
pub const KEY_I: u16 = 0x22;
pub const KEY_J: u16 = 0x26;
pub const KEY_K: u16 = 0x28;
pub const KEY_T: u16 = 0x11;
pub const KEY_U: u16 = 0x20;
pub const KEY_RETURN: u16 = 0x24;
/// `kVK_Delete` (backspace).
pub const KEY_DELETE: u16 = 0x33;
pub const KEY_LEFT_ARROW: u16 = 0x7B;
pub const KEY_RIGHT_ARROW: u16 = 0x7C;
pub const KEY_DOWN_ARROW: u16 = 0x7D;
pub const KEY_UP_ARROW: u16 = 0x7E;

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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, uniffi::Record)]
pub struct ActionHotkey {
    /// A system action id (`lock-screen`, `sleep`, …), as listed by `system_actions`.
    pub action: String,
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
#[serde(from = "StoredHyperKeyConfig")]
pub struct HyperKeyConfig {
    /// Off leaves the keyboard alone, keeping the chosen key for when it's back on.
    pub enabled: bool,
    /// The key that acts as ⌃⌥⇧⌘ while held.
    pub key: HyperKeySource,
    /// Hyper includes Shift (⌃⌥⇧⌘). Without it Hyper is ⌃⌥⌘, and Shift can be added on top.
    pub include_shift: bool,
    /// What pressing and releasing the key on its own does.
    pub quick_press: QuickPress,
}

impl HyperKeyConfig {
    /// The key to turn into Hyper, if any.
    pub fn active_key(&self) -> Option<HyperKeySource> {
        self.enabled.then_some(self.key)
    }
}

impl Default for HyperKeyConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            key: HyperKeySource::CapsLock,
            include_shift: true,
            quick_press: QuickPress::Nothing,
        }
    }
}

/// The on-disk shape, which also reads files from before `enabled` existed, when a `null`
/// key meant off.
#[derive(Deserialize)]
#[serde(default)]
struct StoredHyperKeyConfig {
    enabled: Option<bool>,
    key: Option<HyperKeySource>,
    include_shift: bool,
    quick_press: QuickPress,
}

impl Default for StoredHyperKeyConfig {
    fn default() -> Self {
        let defaults = HyperKeyConfig::default();
        Self {
            enabled: None,
            key: None,
            include_shift: defaults.include_shift,
            quick_press: defaults.quick_press,
        }
    }
}

impl From<StoredHyperKeyConfig> for HyperKeyConfig {
    fn from(stored: StoredHyperKeyConfig) -> Self {
        let defaults = HyperKeyConfig::default();
        Self {
            enabled: stored.enabled.unwrap_or(stored.key.is_some()),
            key: stored.key.unwrap_or(defaults.key),
            include_shift: stored.include_shift,
            quick_press: stored.quick_press,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, uniffi::Enum)]
pub enum HyperKeySource {
    CapsLock,
    LeftControl,
    LeftShift,
    LeftOption,
    LeftCommand,
    RightControl,
    RightShift,
    RightOption,
    RightCommand,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, uniffi::Enum)]
pub enum QuickPress {
    Nothing,
    Escape,
    ToggleCapsLock,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, uniffi::Record)]
#[serde(default)]
pub struct MouseConfig {
    pub smooth_scroll: SmoothScrollConfig,
    pub scroll_modifiers: ScrollModifierConfig,
    /// Off leaves every mouse button alone, keeping the bindings for when it's back on.
    pub buttons_enabled: bool,
    pub buttons: Vec<ButtonBinding>,
}

impl Default for MouseConfig {
    fn default() -> Self {
        Self {
            smooth_scroll: SmoothScrollConfig::default(),
            scroll_modifiers: ScrollModifierConfig::default(),
            buttons_enabled: true,
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
        fs::write(
            dir.join("config.json"),
            r#"{"mouse":{"buttons_enabled":false}}"#,
        )
        .unwrap();
        let config = ConfigStore::new(&dir).load().unwrap();
        assert!(!config.mouse.buttons_enabled);
        assert_eq!(config.mouse.smooth_scroll, SmoothScrollConfig::default());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn hyper_key_from_before_the_switch_keeps_its_state() {
        let on: HyperKeyConfig = serde_json::from_str(r#"{"key":"RightOption"}"#).unwrap();
        assert!(on.enabled);
        assert_eq!(on.key, HyperKeySource::RightOption);
        let off: HyperKeyConfig = serde_json::from_str(r#"{"key":null}"#).unwrap();
        assert!(!off.enabled);
        assert_eq!(off.key, HyperKeySource::CapsLock);
    }

    #[test]
    fn hyper_key_round_trips() {
        let config = HyperKeyConfig {
            enabled: false,
            key: HyperKeySource::LeftCommand,
            ..HyperKeyConfig::default()
        };
        let json = serde_json::to_string(&config).unwrap();
        assert_eq!(
            serde_json::from_str::<HyperKeyConfig>(&json).unwrap(),
            config
        );
    }
}
