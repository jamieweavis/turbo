//! Builds Spotlight query strings from what the user typed (pure).
//!
//! Every whitespace-separated word must appear in the file's name, so `tax 2025` finds
//! "2025 Tax Return.pdf". Each word is matched case- and diacritic-insensitively (`cd`)
//! against the file system name only: also matching `kMDItemDisplayName` would catch the
//! rare localized folder name, but it isn't indexed for wildcards and made every query
//! about three times slower (~800 ms instead of ~250 ms on a typical Mac).

/// `None` when there's nothing to search for.
pub(super) fn build(query: &str) -> Option<String> {
    let clauses: Vec<String> = query
        .split_whitespace()
        .map(|word| {
            let word = escape(word);
            format!(r#"kMDItemFSName == "*{word}*"cd"#)
        })
        .collect();
    if clauses.is_empty() {
        None
    } else {
        Some(clauses.join(" && "))
    }
}

/// Escapes a word for use inside a quoted query value: `\` and `"` would end or corrupt
/// the string, and `*` / `?` are wildcards, so typed ones are matched literally.
fn escape(word: &str) -> String {
    let mut escaped = String::with_capacity(word.len());
    for c in word.chars() {
        if matches!(c, '\\' | '"' | '*' | '?') {
            escaped.push('\\');
        }
        escaped.push(c);
    }
    escaped
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_and_blank_queries_build_nothing() {
        assert_eq!(build(""), None);
        assert_eq!(build("   \t "), None);
    }

    #[test]
    fn one_word_matches_the_file_name() {
        assert_eq!(build("report").unwrap(), r#"kMDItemFSName == "*report*"cd"#);
    }

    #[test]
    fn every_word_must_match() {
        let query = build("tax  2025").unwrap();
        assert_eq!(query.matches(" && ").count(), 1);
        assert!(query.contains(r#""*tax*"cd"#));
        assert!(query.contains(r#""*2025*"cd"#));
    }

    #[test]
    fn quotes_backslashes_and_wildcards_are_escaped() {
        let query = build(r#"a"b\c*d?e"#).unwrap();
        assert!(query.contains(r#""*a\"b\\c\*d\?e*"cd"#), "{query}");
    }

    #[test]
    fn an_escaped_quote_cannot_close_the_value() {
        // Without escaping, `"` would end the value and `||` would inject a clause.
        let query = build(r#"x" || kMDItemFSName == "*"#).unwrap();
        for value in query.split(r#"== ""#).skip(1) {
            let end = value.find(r#""cd"#).unwrap();
            let inner = &value[..end];
            let mut chars = inner.chars().peekable();
            while let Some(c) = chars.next() {
                if c == '\\' {
                    chars.next();
                } else {
                    assert_ne!(c, '"', "unescaped quote in {inner}");
                }
            }
        }
    }
}
