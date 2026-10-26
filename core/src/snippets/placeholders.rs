//! Placeholder expansion. Pure: the values (clipboard, formatted dates) are passed in, so
//! the parsing and caret arithmetic are testable without AppKit.
//!
//! `{name}` is replaced when `name` is known and left exactly as typed otherwise, so a
//! snippet of code or JSON with braces in it survives. `{{` and `}}` are literal braces.

use unicode_segmentation::UnicodeSegmentation;

pub(super) const CLIPBOARD: &str = "clipboard";
pub(super) const DATE: &str = "date";
pub(super) const TIME: &str = "time";
pub(super) const DATETIME: &str = "datetime";
pub(super) const CURSOR: &str = "cursor";

/// A snippet ready to insert.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Expanded {
    pub text: String,
    /// How many Left Arrow presses put the caret at `{cursor}` after pasting `text`:
    /// extended grapheme clusters between the cursor and the end, which is what one press
    /// moves over in Cocoa text views and browsers. Zero without `{cursor}`.
    pub caret_left: usize,
}

/// The values placeholders expand to, asked for only when the template uses them (so the
/// clipboard is read only for `{clipboard}`). `None` leaves the placeholder as typed.
pub(super) trait Values {
    fn value(&mut self, name: &str) -> Option<String>;
}

impl<F: FnMut(&str) -> Option<String>> Values for F {
    fn value(&mut self, name: &str) -> Option<String> {
        self(name)
    }
}

/// Replaces placeholders in `template`. `{cursor}` is removed and marks where the caret
/// should end up; only the first one counts and any later ones are simply removed.
pub(super) fn expand(template: &str, mut values: impl Values) -> Expanded {
    let mut text = String::with_capacity(template.len());
    let mut cursor_at = None;
    let mut rest = template;
    while let Some(i) = rest.find(['{', '}']) {
        text.push_str(&rest[..i]);
        let tail = &rest[i..];
        if tail.starts_with("{{") || tail.starts_with("}}") {
            text.push_str(&tail[..1]);
            rest = &tail[2..];
            continue;
        }
        if let Some(after) = tail.strip_prefix('}') {
            text.push('}');
            rest = after;
            continue;
        }
        // An opening brace: a placeholder runs to the next closing brace, unless another
        // opening brace comes first (then this one is just a brace).
        let body = &tail[1..];
        match body.find(['{', '}']) {
            Some(end) if body[end..].starts_with('}') => {
                let name = &body[..end];
                if name == CURSOR {
                    cursor_at.get_or_insert(text.len());
                } else if let Some(value) = values.value(name) {
                    text.push_str(&value);
                } else {
                    text.push_str(&tail[..end + 2]);
                }
                rest = &body[end + 1..];
            }
            _ => {
                text.push('{');
                rest = body;
            }
        }
    }
    text.push_str(rest);
    let caret_left = cursor_at.map_or(0, |at| text[at..].graphemes(true).count());
    Expanded { text, caret_left }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn values(name: &str) -> Option<String> {
        match name {
            CLIPBOARD => Some("copied".into()),
            DATE => Some("26 Oct 2026".into()),
            TIME => Some("14:05".into()),
            DATETIME => Some("26 Oct 2026 at 14:05".into()),
            _ => None,
        }
    }

    fn run(template: &str) -> (String, usize) {
        let expanded = expand(template, values);
        (expanded.text, expanded.caret_left)
    }

    #[test]
    fn plain_text_is_unchanged() {
        assert_eq!(
            run("Kind regards,\nJamie"),
            ("Kind regards,\nJamie".into(), 0)
        );
        assert_eq!(run(""), (String::new(), 0));
    }

    #[test]
    fn fills_known_placeholders() {
        assert_eq!(
            run("Pasted {clipboard} on {date} at {time} ({datetime})").0,
            "Pasted copied on 26 Oct 2026 at 14:05 (26 Oct 2026 at 14:05)"
        );
    }

    #[test]
    fn values_are_not_expanded_again() {
        let expanded = expand("{clipboard}", |_: &str| Some("{date} {cursor}".to_owned()));
        assert_eq!(expanded.text, "{date} {cursor}");
        assert_eq!(expanded.caret_left, 0);
    }

    #[test]
    fn unknown_placeholders_and_missing_values_stay_as_typed() {
        assert_eq!(run("{name} and {Date}").0, "{name} and {Date}");
        assert_eq!(run("{ date }").0, "{ date }");
        assert_eq!(run("{}").0, "{}");
        let expanded = expand("[{clipboard}]", |_: &str| None);
        assert_eq!(expanded.text, "[{clipboard}]");
    }

    #[test]
    fn double_braces_are_literal() {
        assert_eq!(run("{{date}}").0, "{date}");
        assert_eq!(run("fn main() {{ }}").0, "fn main() { }");
        assert_eq!(run("{{{date}}}").0, "{26 Oct 2026}");
    }

    #[test]
    fn stray_braces_are_kept() {
        assert_eq!(run("a } b").0, "a } b");
        assert_eq!(run("{ open").0, "{ open");
        assert_eq!(run("{a{date}").0, "{a26 Oct 2026");
        assert_eq!(run("trailing {").0, "trailing {");
        assert_eq!(run("{\"key\": 1}").0, "{\"key\": 1}");
    }

    #[test]
    fn cursor_is_removed_and_measured_from_the_end() {
        assert_eq!(run("Hello {cursor}!"), ("Hello !".into(), 1));
        assert_eq!(run("{cursor}abc"), ("abc".into(), 3));
        assert_eq!(run("abc{cursor}"), ("abc".into(), 0));
        assert_eq!(
            run("<b>{cursor}</b> {date}"),
            ("<b></b> 26 Oct 2026".into(), 16)
        );
    }

    #[test]
    fn only_the_first_cursor_counts() {
        assert_eq!(run("a{cursor}b{cursor}c"), ("abc".into(), 2));
    }

    #[test]
    fn caret_counts_grapheme_clusters() {
        // A family emoji (one cluster of 7 chars), an accented e as e + combining acute,
        // and CRLF, which a single Left Arrow also crosses.
        let (text, left) = run("{cursor}👨‍👩‍👧‍👦e\u{301}\r\n");
        assert_eq!(text, "👨‍👩‍👧‍👦e\u{301}\r\n");
        assert_eq!(left, 3);
        assert_eq!(run("{cursor}日本語").1, 3);
    }

    #[test]
    fn asks_only_for_used_values() {
        let mut asked = Vec::new();
        expand("{{clipboard}} {date} {cursor} {nope}", |name: &str| {
            asked.push(name.to_owned());
            None
        });
        assert_eq!(asked, ["date", "nope"]);
    }

    #[test]
    fn escaped_cursor_is_literal() {
        assert_eq!(run("{{cursor}}"), ("{cursor}".into(), 0));
    }
}
