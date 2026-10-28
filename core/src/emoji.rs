//! The emoji picker: the Unicode emoji list (CLDR names and groups, GitHub shortcodes) from
//! the `emojis` crate, searched by name and shortcode, with recently used emoji kept in
//! `<data_dir>/emoji.json`, newest first.

use parking_lot::Mutex;
use std::fs;
use std::path::{Path, PathBuf};

use emojis::{Emoji, Group};

use crate::error::CoreError;
use crate::{pasteboard, snippets};

/// Older emoji drop off the recently used row beyond this many (three rows of the grid).
const MAX_RECENT: usize = 24;

/// Search results stop here; enough to scroll through without building the whole list.
const MAX_RESULTS: usize = 400;

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct EmojiInfo {
    pub emoji: String,
    /// The CLDR name, capitalised: "Smiling face with sunglasses".
    pub name: String,
    /// The first GitHub shortcode, without colons.
    pub shortcode: Option<String>,
}

/// A titled run of the grid: "Recently Used", a group, or search results.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct EmojiSection {
    pub title: String,
    pub emojis: Vec<EmojiInfo>,
}

pub(crate) struct EmojiPicker {
    path: PathBuf,
    recent: Mutex<Vec<String>>,
}

impl EmojiPicker {
    /// Loads the recently used emoji from `data_dir`; a missing or unreadable file starts
    /// empty.
    pub fn new(data_dir: &Path) -> Self {
        let path = data_dir.join("emoji.json");
        let recent = match fs::read(&path) {
            Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_else(|error| {
                eprintln!("jaycast: ignoring unreadable emoji history: {error}");
                Vec::new()
            }),
            Err(_) => Vec::new(),
        };
        Self {
            path,
            recent: Mutex::new(recent),
        }
    }

    /// With an empty query: recently used, then every group in CLDR order. Otherwise one
    /// "Results" section, best matches first.
    pub fn search(&self, query: &str) -> Vec<EmojiSection> {
        let recent = self.recent.lock().clone();
        let words = query_words(query);
        if words.is_empty() {
            let recent = recent.iter().filter_map(|e| emojis::get(e)).map(info);
            let mut sections = vec![EmojiSection {
                title: "Recently Used".to_owned(),
                emojis: recent.collect(),
            }];
            sections.extend(Group::iter().map(|group| EmojiSection {
                title: group_title(group).to_owned(),
                emojis: group.emojis().map(info).collect(),
            }));
            sections.retain(|s| !s.emojis.is_empty());
            return sections;
        }
        let mut scored: Vec<(u32, usize, &Emoji)> = emojis::iter()
            .enumerate()
            .filter_map(|(order, emoji)| {
                let mut score = score(emoji, &words)?;
                if recent.iter().any(|r| r == emoji.as_str()) {
                    score += 5;
                }
                Some((score, order, emoji))
            })
            .collect();
        scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
        let emojis: Vec<EmojiInfo> = scored
            .into_iter()
            .take(MAX_RESULTS)
            .map(|(_, _, emoji)| info(emoji))
            .collect();
        if emojis.is_empty() {
            return Vec::new();
        }
        vec![EmojiSection {
            title: "Results".to_owned(),
            emojis,
        }]
    }

    /// Copies `emoji`, or pastes it into the frontmost app (close the palette first), and
    /// moves it to the front of recently used.
    pub fn insert(&self, emoji: &str, paste: bool) -> Result<(), CoreError> {
        if paste {
            snippets::insert_text(emoji)?;
        } else {
            pasteboard::copy(emoji)?;
        }
        let mut recent = self.recent.lock();
        record(&mut recent, emoji);
        self.save(&recent)
    }

    fn save(&self, recent: &[String]) -> Result<(), CoreError> {
        if let Some(dir) = self.path.parent() {
            fs::create_dir_all(dir)?;
        }
        let json = serde_json::to_vec(recent).map_err(|error| CoreError::Other {
            message: error.to_string(),
        })?;
        fs::write(&self.path, json)?;
        Ok(())
    }
}

fn record(recent: &mut Vec<String>, emoji: &str) {
    recent.retain(|e| e != emoji);
    recent.insert(0, emoji.to_owned());
    recent.truncate(MAX_RECENT);
}

fn info(emoji: &Emoji) -> EmojiInfo {
    let mut name = emoji.name().to_owned();
    if let Some(first) = name.get(..1) {
        name.replace_range(..1, &first.to_uppercase());
    }
    EmojiInfo {
        emoji: emoji.as_str().to_owned(),
        name,
        shortcode: emoji.shortcode().map(str::to_owned),
    }
}

fn group_title(group: Group) -> &'static str {
    match group {
        Group::SmileysAndEmotion => "Smileys & Emotion",
        Group::PeopleAndBody => "People & Body",
        Group::AnimalsAndNature => "Animals & Nature",
        Group::FoodAndDrink => "Food & Drink",
        Group::TravelAndPlaces => "Travel & Places",
        Group::Activities => "Activities",
        Group::Objects => "Objects",
        Group::Symbols => "Symbols",
        Group::Flags => "Flags",
    }
}

/// Lowercase words, with shortcode colons and underscores treated as spaces.
fn query_words(query: &str) -> Vec<String> {
    query
        .split(|c: char| c.is_whitespace() || c == ':' || c == '_')
        .filter(|w| !w.is_empty())
        .map(str::to_lowercase)
        .collect()
}

/// Every query word must start a word of the name or a shortcode. The whole name or a
/// shortcode matching exactly ranks highest, then the name starting with the query, then
/// fewer extra words in the name.
fn score(emoji: &Emoji, words: &[String]) -> Option<u32> {
    let name = emoji.name().to_lowercase();
    let mut candidates: Vec<&str> = name
        .split(|c: char| c.is_whitespace() || c == '-' || c == ':' || c == ',')
        .filter(|w| !w.is_empty())
        .collect();
    for shortcode in emoji.shortcodes() {
        candidates.extend(shortcode.split('_'));
    }
    if !words
        .iter()
        .all(|word| candidates.iter().any(|c| c.starts_with(word.as_str())))
    {
        return None;
    }
    let query = words.join(" ");
    let snake = words.join("_");
    let mut score = 0;
    if name == query || emoji.shortcodes().any(|s| s == snake) {
        score += 100;
    } else if name.starts_with(&query) || emoji.shortcodes().any(|s| s.starts_with(&snake)) {
        score += 50;
    }
    if words
        .iter()
        .all(|word| candidates.iter().any(|c| c == word))
    {
        score += 20;
    }
    let extra = name.split_whitespace().count().saturating_sub(words.len());
    Some(score + 10u32.saturating_sub(extra as u32))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn picker(recent: &[&str]) -> EmojiPicker {
        EmojiPicker {
            path: PathBuf::new(),
            recent: Mutex::new(recent.iter().map(|e| (*e).to_owned()).collect()),
        }
    }

    fn first(picker: &EmojiPicker, query: &str) -> String {
        picker.search(query)[0].emojis[0].emoji.clone()
    }

    #[test]
    fn empty_query_lists_recent_then_groups() {
        let sections = picker(&["🚀", "🙂"]).search(" ");
        assert_eq!(sections[0].title, "Recently Used");
        assert_eq!(sections[0].emojis[0].emoji, "🚀");
        assert_eq!(sections[1].title, "Smileys & Emotion");
        assert_eq!(sections.last().unwrap().title, "Flags");

        let sections = picker(&[]).search("");
        assert_eq!(sections[0].title, "Smileys & Emotion");
        assert_eq!(sections.len(), 9);
    }

    #[test]
    fn finds_by_name_and_shortcode() {
        let picker = picker(&[]);
        assert_eq!(first(&picker, "rocket"), "🚀");
        assert_eq!(first(&picker, ":+1:"), "👍");
        assert_eq!(first(&picker, "thumbs up"), "👍");
        assert_eq!(first(&picker, "red heart"), "❤️");
        assert_eq!(first(&picker, "sunglasses"), "🕶️");
        assert_eq!(first(&picker, "smiling sunglasses"), "😎");
        assert!(picker.search("zzzqqq").is_empty());
    }

    #[test]
    fn names_are_capitalised() {
        let sections = picker(&[]).search("rocket");
        let rocket = &sections[0].emojis[0];
        assert_eq!(rocket.name, "Rocket");
        assert_eq!(rocket.shortcode.as_deref(), Some("rocket"));
    }

    #[test]
    fn recent_use_breaks_ties() {
        let position = |picker: &EmojiPicker, emoji: &str| {
            picker.search("heart")[0]
                .emojis
                .iter()
                .position(|e| e.emoji == emoji)
                .unwrap()
        };
        let plain = picker(&[]);
        let (earlier, later) = if position(&plain, "💙") < position(&plain, "💚") {
            ("💙", "💚")
        } else {
            ("💚", "💙")
        };
        let recent = picker(&[later]);
        assert!(position(&recent, later) < position(&recent, earlier));
    }

    #[test]
    fn recording_moves_to_front_and_caps() {
        let mut recent = vec!["a".to_owned(), "b".to_owned()];
        record(&mut recent, "b");
        assert_eq!(recent, ["b", "a"]);
        for i in 0..40 {
            record(&mut recent, &i.to_string());
        }
        assert_eq!(recent.len(), MAX_RECENT);
        assert_eq!(recent[0], "39");
    }
}
