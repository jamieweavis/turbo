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
    /// Keep gliding after the wheel stops, like a trackpad flick.
    pub momentum: bool,
}

impl Default for SmoothScrollConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            smoothness: Smoothness::Regular,
            speed: 1.0,
            acceleration: true,
            momentum: true,
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
