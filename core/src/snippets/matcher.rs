//! What the expander remembers of recent typing, and when it ends with a keyword. Pure:
//! the tap turns each key-down into a [`Key`] and feeds it here.
//!
//! The buffer only ever holds as many characters as the longest keyword, lives in memory
//! on the tap thread, and is never logged or persisted.

use unicode_segmentation::UnicodeSegmentation;

// Virtual key codes (`kVK_*` from HIToolbox/Events.h).
const KEY_RETURN: u16 = 0x24;
const KEY_TAB: u16 = 0x30;
const KEY_DELETE: u16 = 0x33;
const KEY_ESCAPE: u16 = 0x35;
const KEY_KEYPAD_ENTER: u16 = 0x4C;
const KEY_HOME: u16 = 0x73;
const KEY_PAGE_UP: u16 = 0x74;
const KEY_FORWARD_DELETE: u16 = 0x75;
const KEY_END: u16 = 0x77;
const KEY_PAGE_DOWN: u16 = 0x79;
const KEY_LEFT_ARROW: u16 = 0x7B;
const KEY_RIGHT_ARROW: u16 = 0x7C;
const KEY_DOWN_ARROW: u16 = 0x7D;
const KEY_UP_ARROW: u16 = 0x7E;

/// Keys after which the caret is somewhere we can't follow (or the text was committed).
const RESET_KEYS: &[u16] = &[
    KEY_RETURN,
    KEY_TAB,
    KEY_ESCAPE,
    KEY_KEYPAD_ENTER,
    KEY_HOME,
    KEY_PAGE_UP,
    KEY_END,
    KEY_PAGE_DOWN,
    KEY_LEFT_ARROW,
    KEY_RIGHT_ARROW,
    KEY_DOWN_ARROW,
    KEY_UP_ARROW,
];

/// A key-down, as far as the buffer is concerned.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Key {
    /// Typed text (usually one character).
    Text(String),
    /// Deletes the character before the caret.
    Backspace,
    /// The caret moved, the text was committed, or a shortcut ran: forget everything.
    Reset,
    /// Changes nothing before the caret (a function key, forward delete, a lone modifier).
    Ignore,
}

/// Classifies a key-down from its virtual key code, the ⌘/⌃ and ⌥ modifier state and the
/// text it produces (`CGEventKeyboardGetUnicodeString`).
pub(super) fn classify(key_code: u16, command_or_control: bool, option: bool, text: &str) -> Key {
    if command_or_control || RESET_KEYS.contains(&key_code) {
        return Key::Reset;
    }
    match key_code {
        // ⌥⌫ deletes a word, which we'd have to guess the bounds of.
        KEY_DELETE if option => Key::Reset,
        KEY_DELETE => Key::Backspace,
        KEY_FORWARD_DELETE => Key::Ignore,
        _ => {
            // Function and navigation keys report private-use characters (U+F700…), and
            // some keys report control characters; neither is typed text.
            let typed: String = text
                .chars()
                .filter(|&c| !c.is_control() && !('\u{F700}'..='\u{F8FF}').contains(&c))
                .collect();
            if typed.is_empty() {
                Key::Ignore
            } else {
                Key::Text(typed)
            }
        }
    }
}

/// A keyword and the snippet it expands to (an index into the caller's list).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Keyword {
    pub text: String,
    pub snippet: usize,
}

/// Recent typing, matched against keywords after every character.
#[derive(Debug, Default)]
pub(super) struct Matcher {
    /// Longest first, so the longest of two keywords sharing a suffix wins.
    keywords: Vec<Keyword>,
    /// In characters.
    longest: usize,
    buffer: String,
}

/// A keyword was just completed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Completed {
    pub snippet: usize,
    /// Backspaces needed to erase the typed keyword: its grapheme clusters, which is what
    /// one Backspace deletes.
    pub erase: usize,
}

impl Matcher {
    /// `keywords` are `(keyword, snippet index)`; blank keywords are dropped and keywords
    /// are trimmed, matching how settings show them.
    pub fn new<'a>(keywords: impl IntoIterator<Item = (&'a str, usize)>) -> Self {
        let mut keywords: Vec<Keyword> = keywords
            .into_iter()
            .map(|(text, snippet)| (text.trim(), snippet))
            .filter(|(text, _)| !text.is_empty())
            .map(|(text, snippet)| Keyword {
                text: text.to_owned(),
                snippet,
            })
            .collect();
        // Stable, so among identical keywords the first snippet wins.
        keywords.sort_by_key(|k| std::cmp::Reverse(k.text.chars().count()));
        let longest = keywords.first().map_or(0, |k| k.text.chars().count());
        Self {
            keywords,
            longest,
            buffer: String::new(),
        }
    }

    pub fn reset(&mut self) {
        self.buffer.clear();
    }

    /// Applies a key; returns the keyword it completed, if any (and then starts afresh, so
    /// the expansion itself can't complete another keyword).
    pub fn key(&mut self, key: Key) -> Option<Completed> {
        match key {
            Key::Text(text) => {
                self.buffer.push_str(&text);
                self.trim();
                let completed = self.completed();
                if completed.is_some() {
                    self.buffer.clear();
                }
                completed
            }
            Key::Backspace => {
                if let Some((at, _)) = self.buffer.grapheme_indices(true).next_back() {
                    self.buffer.truncate(at);
                }
                None
            }
            Key::Reset => {
                self.buffer.clear();
                None
            }
            Key::Ignore => None,
        }
    }

    fn completed(&self) -> Option<Completed> {
        self.keywords
            .iter()
            .find(|k| self.buffer.ends_with(&k.text))
            .map(|k| Completed {
                snippet: k.snippet,
                erase: k.text.graphemes(true).count(),
            })
    }

    /// Keeps only the last `longest` characters: nothing older can be part of a keyword.
    fn trim(&mut self) {
        let excess = self.buffer.chars().count().saturating_sub(self.longest);
        if excess > 0 {
            let cut = self
                .buffer
                .char_indices()
                .nth(excess)
                .map_or(self.buffer.len(), |(i, _)| i);
            self.buffer.drain(..cut);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn matcher(keywords: &[&str]) -> Matcher {
        Matcher::new(keywords.iter().enumerate().map(|(i, k)| (*k, i)))
    }

    /// Types `text` one character at a time; returns every completion.
    fn type_text(matcher: &mut Matcher, text: &str) -> Vec<Completed> {
        text.chars()
            .filter_map(|c| matcher.key(Key::Text(c.to_string())))
            .collect()
    }

    fn done(snippet: usize, erase: usize) -> Completed {
        Completed { snippet, erase }
    }

    #[test]
    fn completes_a_keyword_typed_anywhere() {
        let mut m = matcher(&[";addr", ";sig"]);
        assert_eq!(type_text(&mut m, "my ;addr"), vec![done(0, 5)]);
        assert_eq!(type_text(&mut m, ";sig"), vec![done(1, 4)]);
        assert!(type_text(&mut m, ";si g").is_empty());
    }

    #[test]
    fn matching_is_case_sensitive() {
        let mut m = matcher(&[";Addr"]);
        assert!(type_text(&mut m, ";addr").is_empty());
        assert_eq!(type_text(&mut m, ";Addr"), vec![done(0, 5)]);
    }

    #[test]
    fn longest_keyword_wins_when_one_is_a_suffix_of_another() {
        let mut m = matcher(&["dd", ";dd"]);
        assert_eq!(type_text(&mut m, ";dd"), vec![done(1, 3)]);
        assert_eq!(type_text(&mut m, "xdd"), vec![done(0, 2)]);
    }

    #[test]
    fn a_shorter_keyword_inside_a_longer_one_fires_first() {
        // Expected behaviour (and why keywords usually start with a prefix like `;`):
        // "dd" completes before ";ddx" can.
        let mut m = matcher(&["dd", ";ddx"]);
        assert_eq!(type_text(&mut m, ";ddx"), vec![done(0, 2)]);
    }

    #[test]
    fn buffer_starts_afresh_after_a_completion() {
        let mut m = matcher(&["aa"]);
        assert_eq!(type_text(&mut m, "aaa"), vec![done(0, 2)]);
        assert_eq!(type_text(&mut m, "a"), vec![done(0, 2)]);
    }

    #[test]
    fn backspace_corrects_a_typo() {
        let mut m = matcher(&[";addr"]);
        type_text(&mut m, ";adf");
        assert_eq!(m.key(Key::Backspace), None);
        assert_eq!(type_text(&mut m, "dr"), vec![done(0, 5)]);
    }

    #[test]
    fn backspace_removes_a_whole_grapheme_cluster() {
        let mut m = matcher(&["x-y"]);
        type_text(&mut m, "x");
        m.key(Key::Text("e\u{301}".into()));
        m.key(Key::Backspace);
        assert_eq!(type_text(&mut m, "-y"), vec![done(0, 3)]);
        // Backspace on an empty buffer is harmless.
        m.key(Key::Backspace);
        m.key(Key::Backspace);
    }

    #[test]
    fn reset_forgets_what_was_typed() {
        let mut m = matcher(&[";addr"]);
        type_text(&mut m, ";ad");
        m.key(Key::Reset);
        assert!(type_text(&mut m, "dr").is_empty());
        type_text(&mut m, ";ad");
        m.reset();
        assert!(type_text(&mut m, "dr").is_empty());
    }

    #[test]
    fn ignored_keys_change_nothing() {
        let mut m = matcher(&[";addr"]);
        type_text(&mut m, ";ad");
        assert_eq!(m.key(Key::Ignore), None);
        assert_eq!(type_text(&mut m, "dr"), vec![done(0, 5)]);
    }

    #[test]
    fn buffer_is_bounded_by_the_longest_keyword() {
        let mut m = matcher(&["abc"]);
        type_text(&mut m, &"x".repeat(10_000));
        assert!(m.buffer.chars().count() <= 3);
        assert_eq!(type_text(&mut m, "abc"), vec![done(0, 3)]);
    }

    #[test]
    fn non_ascii_keywords() {
        let mut m = matcher(&["→é", "日本"]);
        assert_eq!(type_text(&mut m, "x→é"), vec![done(0, 2)]);
        assert_eq!(type_text(&mut m, "日本"), vec![done(1, 2)]);
        // e + combining acute is two characters but one cluster: one Backspace erases it.
        let mut m = matcher(&[";e\u{301}"]);
        assert_eq!(type_text(&mut m, ";e\u{301}"), vec![done(0, 2)]);
    }

    #[test]
    fn blank_keywords_are_dropped_and_others_trimmed() {
        let m = matcher(&["", "  "]);
        assert!(m.keywords.is_empty());
        let mut m = matcher(&["", " ;x "]);
        assert_eq!(m.keywords.len(), 1);
        assert_eq!(type_text(&mut m, ";x"), vec![done(1, 2)]);
    }

    #[test]
    fn duplicate_keywords_pick_the_first_snippet() {
        let mut m = matcher(&[";a", ";a"]);
        assert_eq!(type_text(&mut m, ";a"), vec![done(0, 2)]);
    }

    #[test]
    fn classifies_keys() {
        assert_eq!(classify(0x00, false, false, "a"), Key::Text("a".into()));
        assert_eq!(classify(0x00, false, true, "å"), Key::Text("å".into()));
        assert_eq!(classify(0x31, false, false, " "), Key::Text(" ".into()));
        assert_eq!(classify(0x00, true, false, "a"), Key::Reset);
        assert_eq!(classify(KEY_DELETE, false, false, "\u{8}"), Key::Backspace);
        assert_eq!(classify(KEY_DELETE, false, true, "\u{8}"), Key::Reset);
        assert_eq!(classify(KEY_DELETE, true, false, "\u{8}"), Key::Reset);
        for key in [
            KEY_RETURN,
            KEY_TAB,
            KEY_ESCAPE,
            KEY_LEFT_ARROW,
            KEY_UP_ARROW,
            KEY_HOME,
            KEY_PAGE_DOWN,
        ] {
            assert_eq!(classify(key, false, false, ""), Key::Reset, "{key:#x}");
        }
        assert_eq!(
            classify(KEY_FORWARD_DELETE, false, false, "\u{7f}"),
            Key::Ignore
        );
        // F5 reports U+F708.
        assert_eq!(classify(0x60, false, false, "\u{F708}"), Key::Ignore);
        assert_eq!(classify(0x38, false, false, ""), Key::Ignore);
    }
}
