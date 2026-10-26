//! Pure helpers for quicklink and command templates: finding the argument after a keyword,
//! expanding placeholders, and escaping what gets substituted.

pub(super) const QUERY: &str = "query";
pub(super) const CLIPBOARD: &str = "clipboard";

/// Whether the template takes a `{name}` placeholder.
pub(super) fn uses(template: &str, name: &str) -> bool {
    template.contains(&format!("{{{name}}}"))
}

/// The argument typed after `keyword`: `g rust traits` → `rust traits`. The keyword matches
/// case-insensitively, must be followed by whitespace, and may contain spaces itself (names
/// double as keywords). `Some("")` when only the keyword and a space have been typed.
pub(super) fn split_argument<'a>(query: &'a str, keyword: &str) -> Option<&'a str> {
    let query = query.trim_start();
    let keyword = keyword.trim();
    if keyword.is_empty() {
        return None;
    }
    let mut rest = query.char_indices();
    for expected in keyword.chars() {
        let (_, actual) = rest.next()?;
        if !actual.to_lowercase().eq(expected.to_lowercase()) {
            return None;
        }
    }
    let after = match rest.next() {
        Some((i, c)) if c.is_whitespace() => i,
        _ => return None,
    };
    Some(query[after..].trim())
}

/// Replaces each `{name}` that `value` knows in one pass, so substituted text is never
/// expanded again. Unknown placeholders are left as typed.
pub(super) fn expand(template: &str, mut value: impl FnMut(&str) -> Option<String>) -> String {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(open) = rest.find('{') {
        out.push_str(&rest[..open]);
        let after = &rest[open + 1..];
        match after.find('}') {
            Some(close) => {
                let name = &after[..close];
                match value(name) {
                    Some(replacement) => out.push_str(&replacement),
                    None => out.push_str(&rest[open..open + close + 2]),
                }
                rest = &after[close + 1..];
            }
            None => {
                out.push_str(&rest[open..]);
                rest = "";
            }
        }
    }
    out.push_str(rest);
    out
}

/// Percent-encodes everything outside RFC 3986's unreserved set (`A–Z a–z 0–9 - . _ ~`).
/// Spaces become `%20` rather than `+`: `%20` means a space in both paths and query strings,
/// while `+` is only a space in form-encoded queries (and a literal `+` elsewhere).
pub(super) fn url_encode(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

/// POSIX single-quoting: the result is always exactly one shell word, whatever it contains.
/// A `'` can't appear inside single quotes, so it's closed, escaped and reopened: `'\''`.
pub(super) fn shell_quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', r"'\''"))
}

/// Shortens `text` to `max` characters, ending with an ellipsis if anything was cut.
pub(super) fn truncate(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_owned();
    }
    let mut out: String = text.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn argument_follows_the_keyword_and_whitespace() {
        assert_eq!(split_argument("g rust traits", "g"), Some("rust traits"));
        assert_eq!(split_argument("  G   rust  ", "g"), Some("rust"));
        assert_eq!(split_argument("g ", "g"), Some(""));
        assert_eq!(split_argument("yt\tcats", "yt"), Some("cats"));
    }

    #[test]
    fn keyword_must_be_whole() {
        assert_eq!(split_argument("g", "g"), None);
        assert_eq!(split_argument("gh repo", "g"), None);
        assert_eq!(split_argument("google x", "g"), None);
        assert_eq!(split_argument("x g y", "g"), None);
        assert_eq!(split_argument("anything", ""), None);
    }

    #[test]
    fn multi_word_and_non_ascii_keywords() {
        assert_eq!(
            split_argument("search google rust", "Search Google"),
            Some("rust")
        );
        assert_eq!(split_argument("ÉTÉ plage", "été"), Some("plage"));
    }

    #[test]
    fn expands_known_placeholders_once() {
        let values = |name: &str| match name {
            QUERY => Some("{clipboard}".to_owned()),
            CLIPBOARD => Some("pasted".to_owned()),
            _ => None,
        };
        assert_eq!(
            expand("a {query} b {clipboard} {other} {", values),
            "a {clipboard} b pasted {other} {"
        );
        assert_eq!(expand("no placeholders", values), "no placeholders");
    }

    #[test]
    fn detects_placeholders() {
        assert!(uses("https://x.test/?q={query}", QUERY));
        assert!(!uses("https://x.test/", QUERY));
        assert!(uses("echo {clipboard}", CLIPBOARD));
    }

    #[test]
    fn url_encoding_keeps_only_unreserved() {
        assert_eq!(url_encode("rust traits"), "rust%20traits");
        assert_eq!(url_encode("a+b&c=d/e?#"), "a%2Bb%26c%3Dd%2Fe%3F%23");
        assert_eq!(url_encode("Az09-._~"), "Az09-._~");
        assert_eq!(url_encode("café"), "caf%C3%A9");
    }

    #[test]
    fn shell_quoting_survives_hostile_input() {
        assert_eq!(shell_quote("plain"), "'plain'");
        assert_eq!(shell_quote(""), "''");
        assert_eq!(shell_quote("it's"), r"'it'\''s'");
        assert_eq!(
            shell_quote("$HOME `id` $(id) \"x\""),
            "'$HOME `id` $(id) \"x\"'"
        );
        assert_eq!(shell_quote("a\nb"), "'a\nb'");
    }

    #[test]
    fn shell_quoting_round_trips_through_a_real_shell() {
        for value in [
            "it's", "$HOME", "`id`", "$(id)", "a\nb", "\\", "\"", "*", "",
        ] {
            let output = std::process::Command::new("/bin/sh")
                .arg("-c")
                .arg(format!("printf %s {}", shell_quote(value)))
                .output()
                .unwrap();
            assert_eq!(String::from_utf8(output.stdout).unwrap(), value);
        }
    }

    #[test]
    fn truncates_by_characters() {
        assert_eq!(truncate("short", 10), "short");
        assert_eq!(truncate("abcdef", 4), "abc…");
        assert_eq!(truncate("ééééé", 3), "éé…");
    }
}
