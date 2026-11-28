//! User settings, persisted as TOML at `~/.config/turbo/config.toml`.
//!
//! Every struct uses `#[serde(default)]` so configs written by older builds keep loading
//! as fields are added.

use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use parking_lot::{Condvar, Mutex};
use serde::{Deserialize, Serialize};

use crate::error::CoreError;

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub mouse: MouseConfig,
    pub monitor: MonitorConfig,
    pub launcher: LauncherConfig,
    pub hyper_key: HyperKeyConfig,
    pub window_drag: WindowDragConfig,
    pub media_keys: MediaKeysConfig,
    pub app_icons: AppIconsConfig,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, uniffi::Record)]
#[serde(default)]
pub struct LauncherConfig {
    /// Opens the palette. `None` means no global hotkey (the menu bar still opens it).
    #[serde(with = "cleared")]
    pub hotkey: Option<Hotkey>,
    /// Hotkeys that focus an app (launching it if needed), or hide it if it's already in front.
    pub app_hotkeys: Vec<AppHotkey>,
    /// Hotkeys that run a system action (Lock Screen, Sleep, …) straight away.
    pub action_hotkeys: Vec<ActionHotkey>,
    /// Show a calculator / unit / currency result when the query is an expression.
    pub calculator: bool,
    /// Off leaves quicklinks out of search, keeping them for when it's back on.
    pub quicklinks_enabled: bool,
    pub quicklinks: Vec<Quicklink>,
    /// Off leaves commands out of search, keeping them for when it's back on.
    pub commands_enabled: bool,
    pub commands: Vec<ShellCommand>,
    pub clipboard: ClipboardConfig,
    pub files: FileSearchConfig,
    pub windows: WindowConfig,
    pub snippets: SnippetsConfig,
    /// Was `caffeinate` before the rename; the alias keeps older configs.
    #[serde(alias = "caffeinate")]
    pub keep_awake: KeepAwakeConfig,
    pub color_picker: ColorPickerConfig,
    pub emoji: EmojiConfig,
    pub camera: CameraConfig,
    /// Per-item settings for apps and built-in commands; items without one are enabled with
    /// no aliases.
    pub items: Vec<ItemSettings>,
    /// Result ids listed first, in this order, while the palette's search field is empty.
    pub favourites: Vec<String>,
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
            quicklinks_enabled: true,
            quicklinks: default_quicklinks(),
            commands_enabled: true,
            commands: Vec::new(),
            clipboard: ClipboardConfig::default(),
            files: FileSearchConfig::default(),
            windows: WindowConfig::default(),
            snippets: SnippetsConfig::default(),
            keep_awake: KeepAwakeConfig::default(),
            color_picker: ColorPickerConfig::default(),
            emoji: EmojiConfig::default(),
            camera: CameraConfig::default(),
            items: Vec::new(),
            favourites: Vec::new(),
        }
    }
}

/// Settings for one launcher item (an app or built-in command), keyed by its search result id
/// (`app:<bundle id>`, `system:<action id>`, `settings:<pane id>`, …).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, uniffi::Record)]
#[serde(default)]
pub struct ItemSettings {
    pub id: String,
    /// Off hides the item from palette search; its hotkey still works.
    pub enabled: bool,
    /// Extra names the item is found by in search. Typing one exactly puts it first.
    pub aliases: Vec<String>,
    /// For items without a hotkey setting of their own (palette modes without one, settings
    /// panes). Apps, system actions, window actions, Keep Awake and the colour picker keep
    /// theirs in `app_hotkeys`, `action_hotkeys` and their own sections.
    pub hotkey: Option<Hotkey>,
}

impl Default for ItemSettings {
    fn default() -> Self {
        Self {
            id: String::new(),
            enabled: true,
            aliases: Vec::new(),
            hotkey: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, uniffi::Record)]
#[serde(default)]
pub struct ClipboardConfig {
    /// Off stops watching the clipboard, and leaves out the history's hotkey and search item.
    pub enabled: bool,
    /// Opens the palette in clipboard history.
    #[serde(with = "cleared")]
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
    /// Off leaves out the file search hotkey and search item.
    pub enabled: bool,
    /// Opens the palette in file search.
    pub hotkey: Option<Hotkey>,
    /// Folders searched (`~` allowed). Spotlight indexes more, but results stay in these.
    pub scopes: Vec<String>,
}

impl Default for FileSearchConfig {
    fn default() -> Self {
        Self {
            enabled: true,
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
    /// Off leaves out the window actions' hotkeys and search items.
    pub enabled: bool,
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
            enabled: true,
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
    /// Back to the frame it had before Turbo first moved it.
    Restore,
    /// Same relative position and size on the next display.
    NextDisplay,
    PreviousDisplay,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, uniffi::Record)]
#[serde(default)]
pub struct SnippetsConfig {
    /// Off stops expansion and leaves snippets out of search, keeping them for when it's
    /// back on.
    pub enabled: bool,
    /// Expand a snippet's keyword as soon as it's typed in any app (needs Accessibility).
    /// Snippets stay in palette search either way.
    pub expansion: bool,
    /// Choosing a snippet in the palette pastes it into the frontmost app; otherwise it's
    /// only copied.
    pub paste_on_open: bool,
    pub snippets: Vec<Snippet>,
}

impl Default for SnippetsConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            expansion: true,
            paste_on_open: true,
            snippets: Vec::new(),
        }
    }
}

/// Keeping the Mac awake. Whether it's on lives in the data directory, in `keep-awake.json`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, uniffi::Record)]
#[serde(default)]
pub struct KeepAwakeConfig {
    /// Off lets the Mac sleep again and leaves out Keep Awake's hotkeys and search items.
    pub enabled: bool,
    /// Keep the display on too, not just the system awake.
    pub keep_display_on: bool,
    pub toggle_hotkey: Option<Hotkey>,
    pub on_hotkey: Option<Hotkey>,
    pub off_hotkey: Option<Hotkey>,
}

impl Default for KeepAwakeConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            keep_display_on: true,
            toggle_hotkey: None,
            on_hotkey: None,
            off_hotkey: None,
        }
    }
}

/// Picking a colour from anywhere on screen. The history lives in the data directory, in
/// `colors.json`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, uniffi::Record)]
#[serde(default)]
pub struct ColorPickerConfig {
    /// Off leaves out the colour picker's hotkey and search items.
    pub enabled: bool,
    /// Shows the magnifier to pick a colour.
    pub hotkey: Option<Hotkey>,
    /// How a picked colour is copied.
    pub format: ColorFormat,
}

impl Default for ColorPickerConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            hotkey: None,
            format: ColorFormat::default(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, uniffi::Record)]
#[serde(default)]
pub struct EmojiConfig {
    /// Off leaves out the emoji picker's hotkey and search item. Its hotkey is the
    /// `mode:emoji` item's.
    pub enabled: bool,
    /// Choosing an emoji pastes it into the frontmost app; otherwise it's only copied.
    pub paste_on_open: bool,
}

impl Default for EmojiConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            paste_on_open: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, uniffi::Record)]
#[serde(default)]
pub struct CameraConfig {
    /// Off leaves out Camera Preview's hotkey and search item. Its hotkey is the
    /// `mode:camera` item's. The camera itself only runs while the preview is showing.
    pub enabled: bool,
    /// Flips the preview horizontally, like a mirror (and like FaceTime's own preview).
    pub mirror: bool,
}

impl Default for CameraConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            mirror: true,
        }
    }
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
    /// Typed before the argument: `g yorkshire pudding recipe`. May be empty (then found by name only).
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

/// The keyboard's play/pause, next and previous keys, sent to a music app of your choice
/// instead of whatever macOS picks (often launching Music).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, uniffi::Record)]
#[serde(default)]
pub struct MediaKeysConfig {
    /// Off leaves the keys to macOS.
    pub enabled: bool,
    pub target: MediaTarget,
    /// Bundle id of the chosen player: the target with `MediaTarget::Fixed`, and the
    /// fallback with `MediaTarget::LastActive`.
    pub player: String,
    /// What a key does when the player it would go to isn't running.
    pub when_not_running: WhenNotRunning,
}

impl Default for MediaKeysConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            target: MediaTarget::LastActive,
            player: "com.spotify.client".into(),
            when_not_running: WhenNotRunning::Launch,
        }
    }
}

/// Custom app icons from the `icons` folder beside the config file: `Spotify.icns` there
/// goes on `Spotify.app`, and back on after an update clears it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, uniffi::Record)]
#[serde(default)]
pub struct AppIconsConfig {
    /// Off gives every app its own icon back.
    pub enabled: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, uniffi::Enum)]
pub enum MediaTarget {
    /// The supported player that played most recently, falling back to the chosen one.
    LastActive,
    /// Always the chosen player.
    Fixed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, uniffi::Enum)]
pub enum WhenNotRunning {
    /// Open the chosen player, then send it the key.
    Launch,
    /// Swallow the key.
    Ignore,
    /// Let macOS handle the key as if Turbo weren't running.
    PassThrough,
}

/// Moving and resizing a window by dragging anywhere in it with modifiers held.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, uniffi::Record)]
#[serde(default)]
pub struct WindowDragConfig {
    /// Off leaves every click alone, keeping the settings for when it's back on.
    pub enabled: bool,
    /// Held, exactly, while pressing a mouse button to grab the window under the cursor.
    pub modifiers: Vec<Modifier>,
    /// Off: the left button moves and the right button resizes. On: the other way round.
    pub swap_buttons: bool,
}

impl WindowDragConfig {
    /// Without any modifier every click would grab a window, so that counts as off.
    pub fn is_active(&self) -> bool {
        self.enabled && !self.modifiers.is_empty()
    }
}

impl Default for WindowDragConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            modifiers: vec![Modifier::Control, Modifier::Command],
            swap_buttons: false,
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
    /// Off leaves the mouse alone entirely, keeping the settings for when it's back on.
    pub enabled: bool,
    pub smooth_scroll: SmoothScrollConfig,
    pub scroll_modifiers: ScrollModifierConfig,
    /// Off leaves every mouse button alone, keeping the bindings for when it's back on.
    pub buttons_enabled: bool,
    pub buttons: Vec<ButtonBinding>,
}

impl Default for MouseConfig {
    fn default() -> Self {
        Self {
            enabled: true,
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

impl MouseConfig {
    /// What the event tap acts on: with the mouse off, nothing.
    pub fn effective(&self) -> Self {
        if self.enabled {
            return self.clone();
        }
        Self {
            enabled: false,
            smooth_scroll: SmoothScrollConfig {
                enabled: false,
                ..self.smooth_scroll.clone()
            },
            scroll_modifiers: ScrollModifierConfig {
                reverse_direction: false,
                horizontal: None,
                zoom: None,
                precise: None,
            },
            buttons_enabled: false,
            buttons: self.buttons.clone(),
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
    /// Scroll vertically like a trackpad: gesture and momentum phases, so scroll views
    /// bounce at their top and bottom.
    pub trackpad_simulation_vertical: bool,
    /// Scroll horizontally like a trackpad: scroll views bounce at their sides, and swipe
    /// gestures such as going back in Safari work.
    pub trackpad_simulation_horizontal: bool,
}

impl Default for SmoothScrollConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            smoothness: Smoothness::Regular,
            speed: 1.0,
            acceleration: true,
            trackpad_simulation_vertical: true,
            trackpad_simulation_horizontal: true,
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
    #[serde(with = "cleared")]
    pub horizontal: Option<Modifier>,
    /// Held while scrolling: zoom in/out.
    #[serde(with = "cleared")]
    pub zoom: Option<Modifier>,
    /// Held while scrolling: small, unsmoothed steps for fine positioning.
    #[serde(with = "cleared")]
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

/// For an optional setting whose default is set: TOML has no null, so a cleared one is
/// written as `false` rather than left out, which would bring the default back on load.
mod cleared {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Stored<T> {
        Set(T),
        Off(bool),
        /// `null`, from the JSON config of older builds.
        Null,
    }

    pub fn serialize<T: Serialize, S: Serializer>(
        value: &Option<T>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        match value {
            Some(value) => value.serialize(serializer),
            None => serializer.serialize_bool(false),
        }
    }

    pub fn deserialize<'de, T: Deserialize<'de>, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Option<T>, D::Error> {
        match Stored::deserialize(deserializer)? {
            Stored::Set(value) => Ok(Some(value)),
            Stored::Off(false) | Stored::Null => Ok(None),
            Stored::Off(true) => Err(serde::de::Error::custom("expected a value or `false`")),
        }
    }
}

/// Loads and saves [`Config`] at `~/.config/turbo/config.toml`.
pub struct ConfigStore {
    path: PathBuf,
    /// Where builds before the TOML config kept it, as JSON. Read only while `path` is missing.
    legacy_path: PathBuf,
}

impl ConfigStore {
    /// `data_dir` is checked for a config from before the move to `~/.config`.
    pub fn new(path: PathBuf, data_dir: &Path) -> Self {
        Self {
            path,
            legacy_path: data_dir.join("config.json"),
        }
    }

    /// `~/.config/turbo/config.toml`, or `$TURBO_CONFIG` when set (`just screenshots` points it
    /// at a demo config).
    pub fn default_path() -> Result<PathBuf, CoreError> {
        if let Some(path) = std::env::var_os("TURBO_CONFIG").filter(|p| !p.is_empty()) {
            return Ok(PathBuf::from(path));
        }
        let home = std::env::home_dir().ok_or_else(|| CoreError::Other {
            message: "couldn't find the home directory".into(),
        })?;
        Ok(home.join(".config").join("turbo").join("config.toml"))
    }

    /// A missing file yields the defaults; an unreadable one is an error rather than
    /// silently discarding the user's settings. A legacy JSON config is moved over on first
    /// load and left in place.
    pub fn load(&self) -> Result<Config, CoreError> {
        match fs::read_to_string(&self.path) {
            Ok(text) => {
                match resolve_symlinks(&self.path) {
                    Ok(target) if target != self.path => log::info!(
                        "loading config from {} (a link to {})",
                        self.path.display(),
                        target.display()
                    ),
                    _ => log::info!("loading config from {}", self.path.display()),
                }
                toml::from_str(&text).map_err(|error| {
                    log::error!("couldn't parse {}: {error}", self.path.display());
                    error.into()
                })
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                match fs::read(&self.legacy_path) {
                    Ok(bytes) => {
                        log::info!("moving the legacy JSON config to {}", self.path.display());
                        let config = serde_json::from_slice(&bytes)?;
                        self.save(&config)?;
                        Ok(config)
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                        log::info!("no config at {}; using defaults", self.path.display());
                        Ok(Config::default())
                    }
                    Err(error) => Err(error.into()),
                }
            }
            Err(error) => Err(error.into()),
        }
    }

    /// Writes atomically (temp file + rename) so a crash never leaves a half-written config.
    pub fn save(&self, config: &Config) -> Result<(), CoreError> {
        self.write(&toml::to_string_pretty(config)?)
    }

    /// When the config is a symlink (into a dotfiles repo, say), the file it points to is
    /// replaced and the link left alone; renaming over the link itself would swap it for a
    /// plain file and quietly detach the config from the repo. The link is resolved on every
    /// write so repointing it while Turbo runs is picked up. A dangling link creates its
    /// target. The replaced file keeps its permissions.
    fn write(&self, text: &str) -> Result<(), CoreError> {
        let target = resolve_symlinks(&self.path)?;
        let dir = match target.parent() {
            Some(dir) if !dir.as_os_str().is_empty() => dir,
            _ => Path::new("."),
        };
        fs::create_dir_all(dir)?;
        let name = target.file_name().map_or_else(
            || "config.toml".into(),
            |name| name.to_string_lossy().into_owned(),
        );
        // Beside the target, so the rename stays on one filesystem; hidden, so a dotfiles
        // repo's `git status` doesn't flash it.
        let tmp = dir.join(format!(".{name}.tmp"));
        let result = write_replacing(&tmp, &target, text);
        if result.is_err() {
            let _ = fs::remove_file(&tmp);
        }
        result
    }
}

fn write_replacing(tmp: &Path, target: &Path, text: &str) -> Result<(), CoreError> {
    let mut file = fs::File::create(tmp)?;
    file.write_all(text.as_bytes())?;
    file.sync_all()?;
    if let Ok(metadata) = fs::metadata(target) {
        fs::set_permissions(tmp, metadata.permissions())?;
    }
    fs::rename(tmp, target)?;
    Ok(())
}

/// Most links followed before giving up, as with the kernel's `MAXSYMLINKS`.
const MAX_SYMLINKS: usize = 32;

/// Follows `path` through any chain of symlinks to the file it names, which needn't exist
/// yet. Relative links resolve against the directory holding the link. Directory symlinks
/// along the way are left as they are: renaming inside one works.
fn resolve_symlinks(path: &Path) -> std::io::Result<PathBuf> {
    let mut current = path.to_path_buf();
    for _ in 0..MAX_SYMLINKS {
        match fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                let link = fs::read_link(&current)?;
                current = match current.parent() {
                    Some(dir) if link.is_relative() => dir.join(link),
                    _ => link,
                };
            }
            Ok(_) => return Ok(current),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(current),
            Err(error) => return Err(error),
        }
    }
    Err(std::io::Error::other(format!(
        "too many levels of symbolic links at {}",
        path.display()
    )))
}

/// How long settings must stay unchanged before they're written, so dragging a slider
/// writes the file once at the end rather than on every tick.
pub const SAVE_DEBOUNCE: Duration = Duration::from_millis(500);

/// Writes the config on a background thread once changes settle. The caller applies each
/// change right away; only the file write waits.
pub struct ConfigSaver {
    shared: Arc<SaverShared>,
    thread: Option<JoinHandle<()>>,
}

struct SaverShared {
    store: ConfigStore,
    debounce: Duration,
    state: Mutex<SaverState>,
    wake: Condvar,
    /// Held for the length of a write, so the thread and `flush` never write at once.
    writing: Mutex<()>,
}

#[derive(Default)]
struct SaverState {
    /// The newest serialized config not yet written, and when it arrived.
    pending: Option<(String, Instant)>,
    /// Why the last write failed; cleared by the next one that succeeds.
    error: Option<String>,
    shutdown: bool,
}

impl ConfigSaver {
    pub fn new(store: ConfigStore, debounce: Duration) -> Self {
        let shared = Arc::new(SaverShared {
            store,
            debounce,
            state: Mutex::new(SaverState::default()),
            wake: Condvar::new(),
            writing: Mutex::new(()),
        });
        let thread_shared = Arc::clone(&shared);
        let thread = thread::Builder::new()
            .name("turbo-config-saver".into())
            .spawn(move || run_saver(&thread_shared))
            .inspect_err(|error| log::error!("couldn't start the config saver: {error}"))
            .ok();
        Self { shared, thread }
    }

    pub fn load(&self) -> Result<Config, CoreError> {
        self.shared.store.load()
    }

    /// Queues `config` to be written once changes settle. Errs if it can't be serialized,
    /// or if the last write failed, since settings then aren't reaching the file.
    pub fn save(&self, config: &Config) -> Result<(), CoreError> {
        let text = toml::to_string_pretty(config)?;
        let mut state = self.shared.state.lock();
        state.pending = Some((text, Instant::now()));
        self.shared.wake.notify_one();
        match &state.error {
            Some(message) => Err(CoreError::Io {
                message: message.clone(),
            }),
            None => Ok(()),
        }
    }

    /// Writes any queued config now, e.g. before quitting.
    pub fn flush(&self) -> Result<(), CoreError> {
        self.shared.write_pending()
    }
}

impl Drop for ConfigSaver {
    fn drop(&mut self) {
        if let Err(error) = self.flush() {
            log::error!("couldn't save the config: {error}");
        }
        self.shared.state.lock().shutdown = true;
        self.shared.wake.notify_one();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl SaverShared {
    fn write_pending(&self) -> Result<(), CoreError> {
        let _writing = self.writing.lock();
        let Some((text, _)) = self.state.lock().pending.take() else {
            return Ok(());
        };
        let result = self.store.write(&text);
        self.state.lock().error = result.as_ref().err().map(ToString::to_string);
        result
    }
}

fn run_saver(shared: &SaverShared) {
    loop {
        {
            let mut state = shared.state.lock();
            loop {
                if state.shutdown {
                    return;
                }
                match &state.pending {
                    None => shared.wake.wait(&mut state),
                    Some((_, changed)) => {
                        let quiet = changed.elapsed();
                        if quiet >= shared.debounce {
                            break;
                        }
                        let wait = shared.debounce - quiet;
                        shared.wake.wait_for(&mut state, wait);
                    }
                }
            }
        }
        if let Err(error) = shared.write_pending() {
            log::error!("couldn't save the config: {error}");
        }
    }
}

/// A setting that differs between two configs, for the log.
#[derive(Debug, PartialEq)]
pub(crate) struct Change {
    /// A feature switch (a boolean): worth logging at info. Anything else is a value,
    /// often one a slider changes on every tick, so it's for debug.
    pub toggle: bool,
    pub description: String,
}

/// Every setting that differs, by dotted path. Scalars show both values. Lists show the
/// path only: they hold snippets, quicklinks and commands, which can contain anything, and
/// every free-text field lives in one.
pub(crate) fn changes(old: &Config, new: &Config) -> Vec<Change> {
    let (Ok(old), Ok(new)) = (toml::Value::try_from(old), toml::Value::try_from(new)) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    diff("", Some(&old), Some(&new), &mut out);
    out
}

fn diff(path: &str, old: Option<&toml::Value>, new: Option<&toml::Value>, out: &mut Vec<Change>) {
    use toml::Value;
    if old == new {
        return;
    }
    match (old, new) {
        (Some(Value::Table(old)), Some(Value::Table(new))) => {
            let mut keys: Vec<&String> = old.keys().chain(new.keys()).collect();
            keys.sort();
            keys.dedup();
            for key in keys {
                let path = if path.is_empty() {
                    key.clone()
                } else {
                    format!("{path}.{key}")
                };
                diff(&path, old.get(key), new.get(key), out);
            }
        }
        (Some(Value::Array(_) | Value::Table(_)), _)
        | (_, Some(Value::Array(_) | Value::Table(_))) => {
            out.push(Change {
                toggle: false,
                description: format!("{path} changed"),
            });
        }
        _ => {
            let show = |value: Option<&Value>| {
                value.map_or_else(|| "(unset)".to_owned(), Value::to_string)
            };
            out.push(Change {
                toggle: matches!(old, Some(Value::Boolean(_)))
                    || matches!(new, Some(Value::Boolean(_))),
                description: format!("{path}: {} → {}", show(old), show(new)),
            });
        }
    }
}

/// Logs each setting that changed: switches at info, values and lists at debug.
pub(crate) fn log_changes(old: &Config, new: &Config) {
    for change in changes(old, new) {
        if change.toggle {
            log::info!("{}", change.description);
        } else {
            log::debug!("{}", change.description);
        }
    }
}

/// Logs the settings that differ from the defaults, in one line.
pub(crate) fn log_non_defaults(config: &Config) {
    let changes = changes(&Config::default(), config);
    if changes.is_empty() {
        log::info!("all settings are the defaults");
    } else {
        let list: Vec<String> = changes.into_iter().map(|c| c.description).collect();
        log::info!(
            "settings that differ from the defaults: {}",
            list.join("; ")
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("turbo-test-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    fn store(dir: &Path) -> ConfigStore {
        ConfigStore::new(dir.join("turbo.toml"), &dir.join("data"))
    }

    #[test]
    fn missing_file_loads_defaults() {
        let store = store(&temp_dir("missing"));
        assert_eq!(store.load().unwrap(), Config::default());
    }

    #[test]
    fn defaults_round_trip() {
        let dir = temp_dir("defaults");
        let store = store(&dir);
        store.save(&Config::default()).unwrap();
        assert_eq!(store.load().unwrap(), Config::default());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn cleared_settings_stay_cleared() {
        let dir = temp_dir("cleared");
        let store = store(&dir);
        let mut config = Config::default();
        config.launcher.hotkey = None;
        config.launcher.clipboard.hotkey = None;
        config.mouse.scroll_modifiers.zoom = None;
        store.save(&config).unwrap();
        assert_eq!(store.load().unwrap(), config);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn legacy_json_config_moves_over() {
        let dir = temp_dir("legacy");
        fs::create_dir_all(dir.join("data")).unwrap();
        fs::write(
            dir.join("data/config.json"),
            r#"{"mouse":{"buttons_enabled":false},"launcher":{"hotkey":null}}"#,
        )
        .unwrap();
        let config = store(&dir).load().unwrap();
        assert!(!config.mouse.buttons_enabled);
        assert_eq!(config.launcher.hotkey, None);
        assert!(dir.join("turbo.toml").exists());
        assert_eq!(store(&dir).load().unwrap(), config);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn save_then_load_round_trips() {
        let dir = temp_dir("roundtrip");
        let store = store(&dir);
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

    #[cfg(unix)]
    fn assert_link_to(link: &Path, target: &Path) {
        assert!(fs::symlink_metadata(link).unwrap().file_type().is_symlink());
        assert_eq!(resolve_symlinks(link).unwrap(), target);
    }

    #[cfg(unix)]
    #[test]
    fn saving_through_a_symlink_keeps_the_link() {
        use std::os::unix::fs::{PermissionsExt, symlink};
        let dir = temp_dir("symlink");
        let dotfiles = dir.join("dotfiles");
        fs::create_dir_all(&dotfiles).unwrap();
        let target = dotfiles.join("turbo.toml");
        fs::write(&target, "[mouse]\nbuttons_enabled = false\n").unwrap();
        fs::set_permissions(&target, fs::Permissions::from_mode(0o600)).unwrap();
        let link = dir.join("turbo.toml");
        symlink(&target, &link).unwrap();

        let store = store(&dir);
        let mut config = store.load().unwrap();
        assert!(!config.mouse.buttons_enabled);
        config.mouse.smooth_scroll.speed = 1.5;
        store.save(&config).unwrap();

        assert_link_to(&link, &target);
        assert_eq!(store.load().unwrap(), config);
        assert_eq!(
            fs::metadata(&target).unwrap().permissions().mode() & 0o777,
            0o600
        );
        let leftovers: Vec<_> = fs::read_dir(&dotfiles).unwrap().collect();
        assert_eq!(leftovers.len(), 1, "temp file left behind");
        fs::remove_dir_all(dir).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn relative_and_chained_symlinks_resolve() {
        use std::os::unix::fs::symlink;
        let dir = temp_dir("symlink-chain");
        fs::create_dir_all(dir.join("dotfiles/turbo")).unwrap();
        // turbo.toml -> dotfiles/current.toml -> turbo/config.toml (both relative).
        symlink("turbo/config.toml", dir.join("dotfiles/current.toml")).unwrap();
        symlink("dotfiles/current.toml", dir.join("turbo.toml")).unwrap();
        let target = dir.join("dotfiles/turbo/config.toml");

        // Dangling: the save creates the file at the end of the chain.
        let store = store(&dir);
        store.save(&speed(2.0)).unwrap();
        assert!(target.is_file());
        assert_link_to(&dir.join("turbo.toml"), &target);
        assert_eq!(store.load().unwrap(), speed(2.0));
        fs::remove_dir_all(dir).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn a_symlink_loop_is_an_error_not_a_hang() {
        use std::os::unix::fs::symlink;
        let dir = temp_dir("symlink-loop");
        fs::create_dir_all(&dir).unwrap();
        symlink("b.toml", dir.join("turbo.toml")).unwrap();
        symlink("turbo.toml", dir.join("b.toml")).unwrap();
        assert!(store(&dir).save(&Config::default()).is_err());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn partial_file_fills_in_defaults() {
        let dir = temp_dir("partial");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("turbo.toml"), "[mouse]\nbuttons_enabled = false\n").unwrap();
        let config = store(&dir).load().unwrap();
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
    fn keep_awake_from_before_the_rename_keeps_its_settings() {
        let config: LauncherConfig =
            serde_json::from_str(r#"{"caffeinate":{"keep_display_on":false}}"#).unwrap();
        assert!(!config.keep_awake.keep_display_on);
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

    #[test]
    fn media_keys_round_trip_and_default_off() {
        assert!(!MediaKeysConfig::default().enabled);
        let dir = temp_dir("media-keys");
        let store = store(&dir);
        let config = Config {
            media_keys: MediaKeysConfig {
                enabled: true,
                target: MediaTarget::Fixed,
                player: "com.apple.Music".into(),
                when_not_running: WhenNotRunning::PassThrough,
            },
            ..Config::default()
        };
        store.save(&config).unwrap();
        assert_eq!(store.load().unwrap(), config);
        fs::remove_dir_all(dir).unwrap();

        // A config from before media keys existed loads with them off.
        let old: Config = toml::from_str("[monitor]\nenabled = true\n").unwrap();
        assert_eq!(old.media_keys, MediaKeysConfig::default());
    }

    #[test]
    fn app_icons_round_trip_and_default_off() {
        assert!(!Config::default().app_icons.enabled);
        let config: Config = toml::from_str("[app_icons]\nenabled = true\n").unwrap();
        assert!(config.app_icons.enabled);
        let saved = toml::to_string(&config).unwrap();
        assert_eq!(toml::from_str::<Config>(&saved).unwrap(), config);
    }

    #[test]
    fn window_drag_round_trips_and_needs_a_modifier() {
        let dir = temp_dir("window-drag");
        let store = store(&dir);
        let mut config = Config::default();
        config.window_drag.modifiers = vec![Modifier::Option, Modifier::Shift];
        config.window_drag.swap_buttons = true;
        store.save(&config).unwrap();
        assert_eq!(store.load().unwrap(), config);
        fs::remove_dir_all(dir).unwrap();

        assert!(WindowDragConfig::default().is_active());
        let no_modifiers = WindowDragConfig {
            modifiers: Vec::new(),
            ..WindowDragConfig::default()
        };
        assert!(!no_modifiers.is_active());
    }

    fn speed(speed: f64) -> Config {
        let mut config = Config::default();
        config.mouse.smooth_scroll.speed = speed;
        config
    }

    #[test]
    fn saver_writes_once_changes_settle() {
        let dir = temp_dir("saver-settle");
        let saver = ConfigSaver::new(store(&dir), Duration::from_millis(50));
        saver.save(&speed(1.5)).unwrap();
        saver.save(&speed(2.0)).unwrap();
        assert!(!dir.join("turbo.toml").exists());

        let deadline = Instant::now() + Duration::from_secs(5);
        while !dir.join("turbo.toml").exists() {
            assert!(Instant::now() < deadline, "the config was never written");
            thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(store(&dir).load().unwrap(), speed(2.0));
        drop(saver);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn saver_flushes_on_request_and_on_drop() {
        let dir = temp_dir("saver-flush");
        let saver = ConfigSaver::new(store(&dir), Duration::from_secs(3600));
        saver.save(&speed(1.5)).unwrap();
        saver.flush().unwrap();
        assert_eq!(store(&dir).load().unwrap(), speed(1.5));

        saver.save(&speed(2.0)).unwrap();
        drop(saver);
        assert_eq!(store(&dir).load().unwrap(), speed(2.0));
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn saver_reports_a_failed_write_until_one_succeeds() {
        let dir = temp_dir("saver-error");
        fs::create_dir_all(&dir).unwrap();
        // A file where the config's directory should be makes every write fail.
        let blocker = dir.join("config");
        fs::write(&blocker, "").unwrap();
        let saver = ConfigSaver::new(
            ConfigStore::new(blocker.join("turbo.toml"), &dir),
            Duration::from_secs(3600),
        );
        saver.save(&speed(1.5)).unwrap();
        assert!(saver.flush().is_err());
        assert!(saver.save(&speed(2.0)).is_err());

        fs::remove_file(&blocker).unwrap();
        saver.flush().unwrap();
        assert!(saver.save(&speed(2.5)).is_ok());
        drop(saver);
        assert_eq!(
            ConfigStore::new(blocker.join("turbo.toml"), &dir)
                .load()
                .unwrap(),
            speed(2.5)
        );
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn changes_show_scalars_and_hide_lists() {
        let old = Config::default();
        let mut new = old.clone();
        new.mouse.enabled = !old.mouse.enabled;
        new.launcher.windows.gap = old.launcher.windows.gap + 4;
        new.launcher.snippets.snippets.push(Snippet {
            id: "s".into(),
            name: "s".into(),
            keyword: String::new(),
            text: "secret".into(),
        });
        let found = changes(&old, &new);
        assert_eq!(found.len(), 3, "{found:?}");
        assert!(found.contains(&Change {
            toggle: true,
            description: format!(
                "mouse.enabled: {} → {}",
                old.mouse.enabled, new.mouse.enabled
            ),
        }));
        assert!(
            found
                .iter()
                .any(|c| !c.toggle && c.description.starts_with("launcher.windows.gap: "))
        );
        assert!(found.iter().all(|c| !c.description.contains("secret")));
        assert!(changes(&old, &old).is_empty());
    }

    /// `just screenshots` runs with this; a field renamed without an alias would quietly drop
    /// out of it.
    #[test]
    fn screenshot_demo_config_loads() {
        let config: Config =
            toml::from_str(include_str!("../.github/screenshots/demo-config.toml")).unwrap();
        assert!(config.hyper_key.enabled);
        assert!(config.media_keys.enabled);
        assert!(config.app_icons.enabled);
        assert_eq!(config.launcher.app_hotkeys.len(), 3);
        assert_eq!(config.launcher.action_hotkeys.len(), 1);
        assert_eq!(config.launcher.quicklinks.len(), 5);
        assert_eq!(config.launcher.commands.len(), 4);
        assert_eq!(config.launcher.snippets.snippets.len(), 4);
    }
}
