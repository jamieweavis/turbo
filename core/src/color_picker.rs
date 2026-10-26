//! Colour picker history and formatting. The shell does the picking (`NSColorSampler`, the
//! system's magnifier loupe) and hands the sRGB result here, which records it, copies it and
//! keeps the history in `<data_dir>/colors.json`, newest first.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::config::ColorFormat;
use crate::error::CoreError;
use crate::pasteboard;

/// Older picks are dropped beyond this many.
const MAX_COLORS: usize = 50;

/// A picked sRGB colour.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, uniffi::Record)]
pub struct PickedColor {
    pub red: u8,
    pub green: u8,
    pub blue: u8,
    /// Seconds since 1970; the latest pick for colours picked more than once.
    pub picked_at: f64,
}

impl PickedColor {
    fn same_color(&self, other: &Self) -> bool {
        (self.red, self.green, self.blue) == (other.red, other.green, other.blue)
    }
}

/// `color` as text: `#1A2B3C`, `rgb(26, 43, 60)` or `hsl(210, 40%, 17%)`.
#[uniffi::export]
pub fn format_color(color: PickedColor, format: ColorFormat) -> String {
    let PickedColor {
        red, green, blue, ..
    } = color;
    match format {
        ColorFormat::Hex => format!("#{red:02X}{green:02X}{blue:02X}"),
        ColorFormat::Rgb => format!("rgb({red}, {green}, {blue})"),
        ColorFormat::Hsl => {
            let (hue, saturation, lightness) = hsl(red, green, blue);
            format!(
                "hsl({}, {}%, {}%)",
                hue.round() as u32 % 360,
                (saturation * 100.0).round() as u32,
                (lightness * 100.0).round() as u32
            )
        }
    }
}

/// Hue in degrees, saturation and lightness in 0–1.
fn hsl(red: u8, green: u8, blue: u8) -> (f64, f64, f64) {
    let [r, g, b] = [red, green, blue].map(|c| f64::from(c) / 255.0);
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let lightness = (max + min) / 2.0;
    let delta = max - min;
    if delta == 0.0 {
        return (0.0, 0.0, lightness);
    }
    let saturation = delta / (1.0 - (2.0 * lightness - 1.0).abs());
    let hue = if max == r {
        60.0 * ((g - b) / delta).rem_euclid(6.0)
    } else if max == g {
        60.0 * ((b - r) / delta + 2.0)
    } else {
        60.0 * ((r - g) / delta + 4.0)
    };
    (hue, saturation, lightness)
}

pub(crate) struct ColorHistory {
    path: PathBuf,
    colors: Mutex<Vec<PickedColor>>,
}

impl ColorHistory {
    /// Loads the history from `data_dir`; a missing or unreadable file starts it empty.
    pub fn new(data_dir: &Path) -> Self {
        let path = data_dir.join("colors.json");
        let colors = match fs::read(&path) {
            Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_else(|error| {
                eprintln!("jaycast: ignoring unreadable colour history: {error}");
                Vec::new()
            }),
            Err(_) => Vec::new(),
        };
        Self {
            path,
            colors: Mutex::new(colors),
        }
    }

    /// Newest first.
    pub fn colors(&self) -> Vec<PickedColor> {
        self.colors.lock().unwrap().clone()
    }

    /// Records a pick and copies it as `format`, returning the copied text. Picking a colour
    /// already in the history moves it to the top.
    pub fn record(
        &self,
        red: u8,
        green: u8,
        blue: u8,
        format: ColorFormat,
    ) -> Result<String, CoreError> {
        let color = PickedColor {
            red,
            green,
            blue,
            picked_at: now(),
        };
        let text = format_color(color, format);
        pasteboard::copy(&text)?;
        let mut colors = self.colors.lock().unwrap();
        insert(&mut colors, color);
        self.save(&colors)?;
        Ok(text)
    }

    pub fn delete(&self, color: PickedColor) -> Result<(), CoreError> {
        let mut colors = self.colors.lock().unwrap();
        colors.retain(|c| !c.same_color(&color));
        self.save(&colors)
    }

    pub fn clear(&self) -> Result<(), CoreError> {
        let mut colors = self.colors.lock().unwrap();
        colors.clear();
        self.save(&colors)
    }

    /// Writes atomically, like the config.
    fn save(&self, colors: &[PickedColor]) -> Result<(), CoreError> {
        let tmp = self.path.with_extension("json.tmp");
        fs::write(&tmp, serde_json::to_vec_pretty(colors)?)?;
        fs::rename(&tmp, &self.path)?;
        Ok(())
    }
}

fn insert(colors: &mut Vec<PickedColor>, color: PickedColor) {
    colors.retain(|c| !c.same_color(&color));
    colors.insert(0, color);
    colors.truncate(MAX_COLORS);
}

fn now() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0.0, |elapsed| elapsed.as_secs_f64())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn color(red: u8, green: u8, blue: u8) -> PickedColor {
        PickedColor {
            red,
            green,
            blue,
            picked_at: 0.0,
        }
    }

    #[test]
    fn formats_hex_and_rgb() {
        let c = color(26, 43, 60);
        assert_eq!(format_color(c, ColorFormat::Hex), "#1A2B3C");
        assert_eq!(format_color(c, ColorFormat::Rgb), "rgb(26, 43, 60)");
        assert_eq!(format_color(color(0, 0, 0), ColorFormat::Hex), "#000000");
    }

    #[test]
    fn formats_hsl() {
        assert_eq!(
            format_color(color(255, 0, 0), ColorFormat::Hsl),
            "hsl(0, 100%, 50%)"
        );
        assert_eq!(
            format_color(color(0, 255, 0), ColorFormat::Hsl),
            "hsl(120, 100%, 50%)"
        );
        assert_eq!(
            format_color(color(0, 0, 255), ColorFormat::Hsl),
            "hsl(240, 100%, 50%)"
        );
        assert_eq!(
            format_color(color(128, 128, 128), ColorFormat::Hsl),
            "hsl(0, 0%, 50%)"
        );
        assert_eq!(
            format_color(color(255, 255, 255), ColorFormat::Hsl),
            "hsl(0, 0%, 100%)"
        );
        assert_eq!(
            format_color(color(26, 43, 60), ColorFormat::Hsl),
            "hsl(210, 40%, 17%)"
        );
        // Magenta-ish reds wrap below 360.
        assert_eq!(
            format_color(color(255, 0, 1), ColorFormat::Hsl),
            "hsl(0, 100%, 50%)"
        );
    }

    #[test]
    fn repicks_move_to_top_and_history_is_capped() {
        let mut colors = Vec::new();
        insert(&mut colors, color(1, 0, 0));
        insert(&mut colors, color(2, 0, 0));
        insert(&mut colors, color(1, 0, 0));
        assert_eq!(colors, vec![color(1, 0, 0), color(2, 0, 0)]);

        for red in 0..=255 {
            insert(&mut colors, color(red, 9, 9));
        }
        assert_eq!(colors.len(), MAX_COLORS);
        assert_eq!(colors[0], color(255, 9, 9));
    }

    #[test]
    fn history_survives_a_reload() {
        let dir = std::env::temp_dir().join(format!("jaycast-colors-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let history = ColorHistory::new(&dir);
        {
            let mut colors = history.colors.lock().unwrap();
            insert(&mut colors, color(10, 20, 30));
            insert(&mut colors, color(40, 50, 60));
            history.save(&colors).unwrap();
        }
        history.delete(color(10, 20, 30)).unwrap();
        assert_eq!(ColorHistory::new(&dir).colors(), vec![color(40, 50, 60)]);
        history.clear().unwrap();
        assert!(ColorHistory::new(&dir).colors().is_empty());
        fs::remove_dir_all(&dir).unwrap();
    }
}
