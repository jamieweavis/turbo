//! Snippets: text found in palette search and inserted into the frontmost app, or expanded
//! as soon as its keyword is typed in any app.
//!
//! - `placeholders`: `{clipboard}`, `{date}`, `{cursor}`, … (pure).
//! - `matcher`: recent typing and keyword completion (pure).
//! - `insert`: pasting a snippet through the clipboard and putting the clipboard back.
//! - `tap`: the listen-only key tap and the worker that erases keywords and inserts.
//!
//! Inserting goes through the pasteboard and ⌘V rather than synthesising each character:
//! one undo step, any length, any keyboard layout, at the cost of borrowing the clipboard
//! for about half a second (see `insert`). `{cursor}` is honoured with Left Arrow presses,
//! counted in grapheme clusters; an app whose Left Arrow does something else (a terminal
//! in some modes, a spreadsheet cell) ends up with the caret elsewhere.
//!
//! Expansion watches key-downs and keeps only as many recent characters as the longest
//! keyword, in memory, never logged or saved, and nothing at all while secure input is on.
//! It sees keys, not text: it forgets what it has on clicks, caret movement, shortcuts and
//! app switches, and it can't see text changed any other way (autocorrect, dictation,
//! input methods that compose characters). Keywords match anywhere, so `btw` would also
//! fire inside `abtw`; a prefix such as `;` avoids that. Keys typed while an expansion is
//! being pasted can land in the middle of it.

mod insert;
mod matcher;
mod placeholders;
mod tap;

use std::sync::Mutex;
use std::thread;
use std::time::Duration;

use unicode_segmentation::UnicodeSegmentation;

use crate::config::{Snippet, SnippetsConfig};
use crate::error::CoreError;
use crate::launcher::fuzzy::{self, Prepared};
use crate::launcher::{KEYWORD_SCORE, ResultKind, SearchResult, best_score};
use tap::ExpansionTap;

#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    fn AXIsProcessTrusted() -> u8;
}

/// Lets the palette close and focus settle on the target app before ⌘V lands.
const PASTE_DELAY: Duration = Duration::from_millis(100);
/// Characters of the text shown under the name.
const PREVIEW_MAX: usize = 60;
const SYMBOL: &str = "text.quote";

/// Snippets matching `query` by name or keyword, scored like other launcher results
/// (`snippet:<id>`, `ResultKind::Snippet`). Pure and cheap: called on every keystroke.
///
/// Typing a keyword exactly puts its snippet at `KEYWORD_SCORE`, above fuzzy matches;
/// otherwise names match like app names, keywords at the usual alias discount.
pub(crate) fn results(query: &str, config: &SnippetsConfig) -> Vec<(i32, SearchResult)> {
    let folded = fuzzy::fold_query(query);
    if folded.is_empty() {
        return Vec::new();
    }
    config
        .snippets
        .iter()
        .filter_map(|snippet| {
            let keyword = snippet.keyword.trim();
            let score = if !keyword.is_empty() && fuzzy::fold_query(keyword) == folded {
                KEYWORD_SCORE
            } else {
                let name = Prepared::new(snippet.name.trim());
                let keyword = (!keyword.is_empty()).then(|| Prepared::new(keyword));
                best_score(&folded, &name, keyword.iter())?
            };
            Some((score, result(snippet)))
        })
        .collect()
}

/// Every snippet as listed with an empty query, in configured order.
pub(crate) fn all(config: &SnippetsConfig) -> Vec<SearchResult> {
    config.snippets.iter().map(result).collect()
}

fn result(snippet: &Snippet) -> SearchResult {
    let name = snippet.name.trim();
    let keyword = snippet.keyword.trim();
    let preview = preview(&snippet.text);
    let title = match (name.is_empty(), keyword.is_empty()) {
        (false, _) => name.to_owned(),
        (true, false) => keyword.to_owned(),
        (true, true) => "Snippet".to_owned(),
    };
    let subtitle = if keyword.is_empty() || name.is_empty() {
        preview
    } else if preview.is_empty() {
        keyword.to_owned()
    } else {
        format!("{preview}  ·  {keyword}")
    };
    SearchResult {
        id: format!("snippet:{}", snippet.id),
        title,
        subtitle,
        kind: ResultKind::Snippet,
        path: None,
        symbol: Some(SYMBOL.into()),
        needs_confirmation: false,
        completion: None,
    }
}

/// The text on one line: runs of whitespace (newlines included) become one space, and
/// anything past `PREVIEW_MAX` grapheme clusters becomes "…". Placeholders show as typed.
fn preview(text: &str) -> String {
    let collapsed = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut graphemes = collapsed.graphemes(true);
    let mut preview: String = graphemes.by_ref().take(PREVIEW_MAX).collect();
    if graphemes.next().is_some() {
        preview.truncate(preview.trim_end().len());
        preview.push('…');
    }
    preview
}

fn is_trusted() -> bool {
    // SAFETY: no arguments; reads the process's TCC state.
    unsafe { AXIsProcessTrusted() != 0 }
}

/// Expands the snippet's placeholders and types the text into the frontmost app, then
/// leaves the caret at `{cursor}`. Needs Accessibility. The shell closes the palette first.
///
/// Returns at once: the paste follows shortly on its own thread, and the clipboard is put
/// back about half a second later.
pub(crate) fn insert(snippet: &Snippet) -> Result<(), CoreError> {
    insert_expanding(snippet.text.clone())
}

/// Expands the snippet's placeholders and copies the text (`{cursor}` dropped). Returns
/// what was copied.
pub(crate) fn copy(snippet: &Snippet) -> Result<String, CoreError> {
    let text = insert::expand_now(&snippet.text).text;
    crate::pasteboard::copy(&text)?;
    Ok(text)
}

/// Types `text` into the frontmost app as it is, like a snippet without placeholders.
pub(crate) fn insert_text(text: &str) -> Result<(), CoreError> {
    insert_expanding(text.replace('{', "{{").replace('}', "}}"))
}

fn insert_expanding(text: String) -> Result<(), CoreError> {
    if !is_trusted() {
        return Err(CoreError::PermissionDenied);
    }
    thread::Builder::new()
        .name("jaycast-snippet-insert".into())
        .spawn(move || insert::run(&text, 0, PASTE_DELAY))?;
    Ok(())
}

/// True when the expander has something to do: snippets and expansion are on and some
/// snippet has a keyword.
fn wants_tap(config: &SnippetsConfig) -> bool {
    config.enabled
        && config.expansion
        && config.snippets.iter().any(|s| !s.keyword.trim().is_empty())
}

/// A keyboard event tap that watches what's typed and replaces a snippet keyword with the
/// snippet as soon as the keyword is complete. Runs only while started and expansion is on.
pub(crate) struct SnippetExpander {
    state: Mutex<State>,
    tap: ExpansionTap,
}

struct State {
    /// `start` was called (and `stop` hasn't been since).
    started: bool,
    config: SnippetsConfig,
}

impl SnippetExpander {
    pub fn new(config: SnippetsConfig) -> Self {
        Self {
            tap: ExpansionTap::new(config.clone()),
            state: Mutex::new(State {
                started: false,
                config,
            }),
        }
    }

    /// Applied live: turning expansion off (or removing every keyword) stops the tap.
    pub fn set_config(&self, config: SnippetsConfig) {
        let mut state = self.state.lock().unwrap();
        self.tap.set_config(config.clone());
        state.config = config;
        if let Err(error) = self.sync(&state) {
            eprintln!("jaycast: couldn't start snippet expansion: {error}");
        }
    }

    /// Fails with `PermissionDenied` without Accessibility. A no-op while running.
    pub fn start(&self) -> Result<(), CoreError> {
        let mut state = self.state.lock().unwrap();
        if !is_trusted() {
            return Err(CoreError::PermissionDenied);
        }
        state.started = true;
        let result = self.sync(&state);
        if result.is_err() {
            state.started = false;
        }
        result
    }

    /// While the palette shows, typing goes to it, not to the frontmost app: don't expand.
    pub fn set_palette_visible(&self, visible: bool) {
        self.tap.set_palette_visible(visible);
    }

    pub fn stop(&self) {
        let mut state = self.state.lock().unwrap();
        state.started = false;
        self.tap.stop();
    }

    /// Runs the tap exactly when it's wanted.
    fn sync(&self, state: &State) -> Result<(), CoreError> {
        if state.started && wants_tap(&state.config) {
            self.tap.start()
        } else {
            self.tap.stop();
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snippet(id: &str, name: &str, keyword: &str, text: &str) -> Snippet {
        Snippet {
            id: id.into(),
            name: name.into(),
            keyword: keyword.into(),
            text: text.into(),
        }
    }

    fn config(snippets: Vec<Snippet>) -> SnippetsConfig {
        SnippetsConfig {
            enabled: true,
            expansion: true,
            paste_on_open: true,
            snippets,
        }
    }

    fn sample() -> SnippetsConfig {
        config(vec![
            snippet(
                "addr",
                "Home Address",
                ";addr",
                "1 Infinite Loop\nCupertino, CA",
            ),
            snippet("sig", "Email Signature", ";sig", "Kind regards,\n\nJamie"),
            snippet("shrug", "", "shrug", "¯\\_(ツ)_/¯"),
            snippet("palette", "Meeting Notes", "", "## {date}\n\n{cursor}"),
        ])
    }

    fn ids(results: &[(i32, SearchResult)]) -> Vec<&str> {
        results.iter().map(|(_, r)| r.id.as_str()).collect()
    }

    #[test]
    fn exact_keyword_scores_as_a_keyword() {
        let results = results(";addr", &sample());
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].0, KEYWORD_SCORE);
        assert_eq!(results[0].1.id, "snippet:addr");
        // Case and surrounding spaces don't matter in search.
        assert_eq!(super::results("  ;ADDR ", &sample())[0].0, KEYWORD_SCORE);
    }

    #[test]
    fn keyword_prefix_and_fuzzy_name_score_lower() {
        let config = sample();
        let prefix = results(";ad", &config);
        assert_eq!(ids(&prefix), vec!["snippet:addr"]);
        assert!(prefix[0].0 < KEYWORD_SCORE);
        let name = results("email", &config);
        assert_eq!(ids(&name), vec!["snippet:sig"]);
        assert!(name[0].0 > 0 && name[0].0 < KEYWORD_SCORE);
        assert_eq!(ids(&results("mtg nts", &config)), vec!["snippet:palette"]);
        assert!(results("zzz", &config).is_empty());
        assert!(results("  ", &config).is_empty());
    }

    #[test]
    fn name_match_beats_keyword_match_of_equal_quality() {
        let config = config(vec![
            snippet("a", "Reply", "x", "a"),
            snippet("b", "Other", "reply", "b"),
        ]);
        let scored = results("rep", &config);
        let score = |id: &str| scored.iter().find(|(_, r)| r.id == id).unwrap().0;
        assert!(score("snippet:a") > score("snippet:b"));
    }

    #[test]
    fn result_fields() {
        let results = results("home", &sample());
        let result = &results[0].1;
        assert_eq!(result.title, "Home Address");
        assert_eq!(result.subtitle, "1 Infinite Loop Cupertino, CA  ·  ;addr");
        assert_eq!(result.kind, ResultKind::Snippet);
        assert_eq!(result.symbol.as_deref(), Some(SYMBOL));
        assert_eq!(result.path, None);
        assert_eq!(result.completion, None);
        assert!(!result.needs_confirmation);
    }

    #[test]
    fn untitled_snippet_uses_its_keyword() {
        let results = results("shrug", &sample());
        assert_eq!(results[0].0, KEYWORD_SCORE);
        assert_eq!(results[0].1.title, "shrug");
        assert_eq!(results[0].1.subtitle, "¯\\_(ツ)_/¯");
    }

    #[test]
    fn palette_only_snippet_has_no_keyword_in_subtitle() {
        let results = results("meeting", &sample());
        assert_eq!(results[0].1.subtitle, "## {date} {cursor}");
    }

    #[test]
    fn preview_collapses_whitespace_and_truncates() {
        assert_eq!(preview("  a\n\n b\t c  "), "a b c");
        assert_eq!(preview(""), "");
        let long = "word ".repeat(30);
        let short = preview(&long);
        assert!(
            short.ends_with("word…") || short.ends_with("wor…"),
            "{short}"
        );
        assert!(short.chars().count() <= PREVIEW_MAX + 1);
        // Exactly at the limit: no ellipsis.
        assert_eq!(preview(&"x".repeat(PREVIEW_MAX)), "x".repeat(PREVIEW_MAX));
        // Grapheme clusters aren't split.
        let flags = "🇬🇧".repeat(PREVIEW_MAX + 5);
        assert_eq!(preview(&flags), format!("{}…", "🇬🇧".repeat(PREVIEW_MAX)));
    }

    #[test]
    fn tap_runs_only_with_expansion_and_a_keyword() {
        assert!(wants_tap(&sample()));
        assert!(!wants_tap(&SnippetsConfig {
            enabled: false,
            ..sample()
        }));
        assert!(!wants_tap(&SnippetsConfig {
            expansion: false,
            ..sample()
        }));
        assert!(!wants_tap(&config(vec![snippet("a", "A", " ", "text")])));
        assert!(!wants_tap(&config(Vec::new())));
    }

    #[test]
    fn expander_matches_configured_keywords() {
        let config = sample();
        let mut matcher = tap::matcher_for(&config);
        let completed = ";sig"
            .chars()
            .filter_map(|c| matcher.key(matcher::Key::Text(c.to_string())))
            .last()
            .unwrap();
        assert_eq!(config.snippets[completed.snippet].id, "sig");
        assert_eq!(completed.erase, 4);
    }

    /// Never starts a tap: without `start`, config changes leave it stopped.
    #[test]
    fn config_changes_before_start_leave_the_tap_stopped() {
        let expander = SnippetExpander::new(sample());
        expander.set_config(config(Vec::new()));
        expander.set_config(sample());
        assert!(!expander.tap.is_running());
        expander.stop();
        assert!(!expander.tap.is_running());
    }
}
