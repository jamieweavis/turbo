//! Fuzzy matching: scores how well a typed query matches a title.
//!
//! The query must appear in the title as a subsequence (case- and diacritic-insensitive).
//! Among all the ways it can, the best-scoring alignment wins, found by dynamic programming
//! in O(query × title). Matches at the start of the title, at word starts and camelCase
//! humps, and in unbroken runs score highest, so "vsc" finds "Visual Studio Code" and
//! "saf" puts Safari first. Gaps between matched characters cost a little each.

use unicode_normalization::UnicodeNormalization;

/// Every matched character.
const BASE: i32 = 16;
/// Matching the title's first character (on top of the word-start bonus).
const FIRST_CHAR: i32 = 15;
/// Matching the first character of a word (after a space, punctuation or a digit run).
const WORD_START: i32 = 30;
/// Matching an uppercase letter that follows a lowercase one ("FaceTime" → "T").
const CAMEL_HUMP: i32 = 25;
/// Matching the character right after the previous match.
const CONSECUTIVE: i32 = 18;
/// Per title character skipped between two matches.
const GAP: i32 = 3;
/// Per title character skipped before the first match, up to `LEADING_GAP_MAX`.
const LEADING_GAP: i32 = 1;
const LEADING_GAP_MAX: i32 = 10;
/// The whole query is a prefix of the title.
const PREFIX: i32 = 30;
/// The query is the whole title.
const EXACT: i32 = 30;

const IMPOSSIBLE: i32 = i32::MIN / 2;

/// A title folded and annotated once, so each keystroke only runs the matcher.
#[derive(Debug, Clone)]
pub(crate) struct Prepared {
    /// Lowercased, diacritics removed; one entry per character of the original.
    folded: Vec<char>,
    /// Position bonus per character (first char, word start, camel hump).
    bonus: Vec<i32>,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Match {
    pub score: i32,
    /// Character indices (not byte offsets) of the matched characters in the original text.
    pub indices: Vec<usize>,
}

impl Prepared {
    pub fn new(text: &str) -> Self {
        let chars: Vec<char> = text.chars().collect();
        let folded = chars.iter().map(|&c| fold(c)).collect();
        let bonus = (0..chars.len())
            .map(|i| position_bonus(i.checked_sub(1).map(|p| chars[p]), chars[i]))
            .collect();
        Self { folded, bonus }
    }
}

/// Folds a query the same way titles are folded. Whitespace is kept: it can only match
/// the title's own spaces, which is what someone typing "system set" expects.
pub(crate) fn fold_query(query: &str) -> Vec<char> {
    query.trim().chars().map(fold).collect()
}

/// Scores `query` (already folded) against a prepared title. `None` if it isn't a
/// subsequence. An empty query matches everything with score 0.
pub(crate) fn score(query: &[char], target: &Prepared) -> Option<Match> {
    let (m, n) = (query.len(), target.folded.len());
    if m == 0 {
        return Some(Match {
            score: 0,
            indices: Vec::new(),
        });
    }
    if m > n {
        return None;
    }

    // `row[j]`: best score for query[..=i] with query[i] matched at title position j.
    // `from[i * n + j]`: where query[i - 1] was matched on that best path.
    let mut row = vec![IMPOSSIBLE; n];
    let mut next = vec![IMPOSSIBLE; n];
    let mut from = vec![usize::MAX; m * n];

    for (j, cell) in row.iter_mut().enumerate() {
        if target.folded[j] == query[0] {
            let leading = (j as i32 * LEADING_GAP).min(LEADING_GAP_MAX);
            *cell = BASE + target.bonus[j] - leading;
        }
    }

    for i in 1..m {
        next.fill(IMPOSSIBLE);
        // Best of `row[k] + GAP * k` over k <= j - 2, so a gapped transition from k costs
        // GAP * (j - k - 1) without rescanning every k.
        let mut best_gapped = IMPOSSIBLE;
        let mut best_gapped_at = usize::MAX;
        for j in i..n {
            if j >= 2 && row[j - 2] > IMPOSSIBLE {
                let candidate = row[j - 2] + GAP * (j as i32 - 2);
                if candidate > best_gapped {
                    best_gapped = candidate;
                    best_gapped_at = j - 2;
                }
            }
            if target.folded[j] != query[i] {
                continue;
            }
            let consecutive = if row[j - 1] > IMPOSSIBLE {
                row[j - 1] + CONSECUTIVE
            } else {
                IMPOSSIBLE
            };
            let gapped = if best_gapped > IMPOSSIBLE {
                best_gapped - GAP * (j as i32 - 1)
            } else {
                IMPOSSIBLE
            };
            let (previous, prev_at) = if consecutive >= gapped {
                (consecutive, j - 1)
            } else {
                (gapped, best_gapped_at)
            };
            if previous > IMPOSSIBLE {
                next[j] = previous + BASE + target.bonus[j];
                from[i * n + j] = prev_at;
            }
        }
        std::mem::swap(&mut row, &mut next);
    }

    let (end, &best) = row
        .iter()
        .enumerate()
        .max_by_key(|&(j, &s)| (s, std::cmp::Reverse(j)))?;
    if best <= IMPOSSIBLE {
        return None;
    }

    let mut indices = vec![0; m];
    let mut j = end;
    for i in (0..m).rev() {
        indices[i] = j;
        if i > 0 {
            j = from[i * n + j];
        }
    }

    let mut score = best;
    if target.folded.starts_with(query) {
        score += PREFIX;
        if m == n {
            score += EXACT;
        }
    }
    Some(Match { score, indices })
}

fn position_bonus(previous: Option<char>, current: char) -> i32 {
    match previous {
        None => FIRST_CHAR + WORD_START,
        Some(p) if !p.is_alphanumeric() && current.is_alphanumeric() => WORD_START,
        Some(p) if p.is_numeric() && current.is_alphabetic() => WORD_START,
        Some(p) if p.is_lowercase() && current.is_uppercase() => CAMEL_HUMP,
        _ => 0,
    }
}

/// Lowercase with diacritics removed, one char in, one char out ("É" → "e").
fn fold(c: char) -> char {
    if c.is_ascii() {
        return c.to_ascii_lowercase();
    }
    let base = c.nfd().next().unwrap_or(c);
    base.to_lowercase().next().unwrap_or(base)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn score_of(query: &str, title: &str) -> Option<i32> {
        score(&fold_query(query), &Prepared::new(title)).map(|m| m.score)
    }

    fn indices_of(query: &str, title: &str) -> Vec<usize> {
        score(&fold_query(query), &Prepared::new(title))
            .unwrap()
            .indices
    }

    /// Titles ranked best first for `query`; non-matches dropped.
    fn ranked<'a>(query: &str, titles: &[&'a str]) -> Vec<&'a str> {
        let mut scored: Vec<_> = titles
            .iter()
            .filter_map(|t| score_of(query, t).map(|s| (s, *t)))
            .collect();
        scored.sort_by_key(|&(score, _)| std::cmp::Reverse(score));
        scored.into_iter().map(|(_, t)| t).collect()
    }

    const APPS: &[&str] = &[
        "Safari",
        "Software Update",
        "System Settings",
        "System Information",
        "Terminal",
        "TextEdit",
        "Visual Studio Code",
        "FaceTime",
        "Font Book",
        "Activity Monitor",
        "Screenshot",
        "Time Machine",
    ];

    #[test]
    fn non_subsequences_dont_match() {
        assert_eq!(score_of("xyz", "Safari"), None);
        assert_eq!(score_of("safarii", "Safari"), None);
        assert_eq!(score_of("ifas", "Safari"), None);
    }

    #[test]
    fn empty_query_matches_with_zero() {
        assert_eq!(score_of("", "Safari"), Some(0));
        assert_eq!(score_of("   ", "Safari"), Some(0));
    }

    #[test]
    fn is_case_and_diacritic_insensitive() {
        assert!(score_of("SAF", "safari").is_some());
        assert!(score_of("cafe", "Café Résumé").is_some());
        assert!(score_of("résumé", "Resume Builder").is_some());
    }

    #[test]
    fn prefix_ranks_first() {
        assert_eq!(ranked("saf", APPS)[0], "Safari");
        assert_eq!(ranked("term", APPS)[0], "Terminal");
    }

    #[test]
    fn safari_beats_software_update_on_shared_letters() {
        let safari = score_of("sa", "Safari").unwrap();
        let software = score_of("sa", "Software Update").unwrap();
        assert!(safari > software);
    }

    #[test]
    fn acronyms_match_word_starts() {
        assert_eq!(ranked("vsc", APPS), vec!["Visual Studio Code"]);
        assert_eq!(indices_of("vsc", "Visual Studio Code"), vec![0, 7, 14]);
        assert_eq!(ranked("am", APPS)[0], "Activity Monitor");
        assert_eq!(ranked("tm", APPS)[0], "Time Machine");
    }

    #[test]
    fn word_prefixes_combine() {
        assert_eq!(ranked("sysset", APPS)[0], "System Settings");
        assert_eq!(ranked("sysinf", APPS)[0], "System Information");
        assert_eq!(
            indices_of("sysset", "System Settings"),
            vec![0, 1, 2, 7, 8, 9]
        );
    }

    #[test]
    fn camel_humps_count_as_word_starts() {
        assert_eq!(indices_of("ft", "FaceTime"), vec![0, 4]);
        assert_eq!(ranked("ft", APPS)[0], "FaceTime");
    }

    #[test]
    fn prefers_the_aligned_occurrence_over_the_first() {
        // "e" in "TextEdit": the word start E (index 4) beats the first e (index 1).
        assert_eq!(indices_of("te", "TextEdit"), vec![0, 1]);
        assert_eq!(indices_of("ted", "TextEdit"), vec![0, 4, 5]);
    }

    #[test]
    fn contiguous_beats_scattered() {
        let run = score_of("mon", "Activity Monitor").unwrap();
        let scattered = score_of("mon", "Time Machine Online").unwrap();
        assert!(run > scattered);
    }

    #[test]
    fn exact_beats_prefix() {
        assert!(score_of("notes", "Notes").unwrap() > score_of("notes", "Notes Pro").unwrap());
    }

    #[test]
    fn spaces_match_spaces() {
        assert_eq!(ranked("system s", APPS)[0], "System Settings");
    }
}
