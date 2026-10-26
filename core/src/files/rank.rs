//! Filters and orders Spotlight's matches (pure).
//!
//! Spotlight returns matches in no useful order, so results are ranked by how well the name
//! matches (exact, prefix, word start, anywhere), then by how recently the file changed, with
//! a small preference for shallower paths.

use unicode_normalization::UnicodeNormalization;
use unicode_normalization::char::is_combining_mark;

use super::FileResult;

/// File types that are folders on disk but files to the user; their contents never show up.
const PACKAGE_EXTENSIONS: &[&str] = &[
    "app",
    "appex",
    "bundle",
    "framework",
    "kext",
    "plugin",
    "pkg",
    "mpkg",
    "photoslibrary",
    "musiclibrary",
    "tvlibrary",
    "imovielibrary",
    "fcpbundle",
    "logicx",
    "band",
    "xcodeproj",
    "xcworkspace",
    "playground",
    "rtfd",
    "pages",
    "numbers",
    "key",
    "sparsebundle",
    "photobooth",
];

const MATCH_EXACT: i64 = 4_000;
const MATCH_PREFIX: i64 = 3_000;
const MATCH_WORD_START: i64 = 2_000;
const MATCH_ANYWHERE: i64 = 1_000;
/// Recency is worth less than one step of match quality: at most this much, for a file
/// changed just now, falling off logarithmically over days.
const RECENCY_MAX: f64 = 600.0;
const DEPTH_PENALTY: i64 = 15;

/// Whether a path should be hidden: dotfiles, anything inside a package, and `~/Library`.
pub(super) fn is_excluded(path: &str, home: &str) -> bool {
    if !home.is_empty() {
        let library = format!("{}/Library", home.trim_end_matches('/'));
        if path == library || path.starts_with(&format!("{library}/")) {
            return true;
        }
    }
    let components: Vec<&str> = path.split('/').filter(|c| !c.is_empty()).collect();
    let Some((_, ancestors)) = components.split_last() else {
        return true;
    };
    if components.iter().any(|c| c.starts_with('.')) {
        return true;
    }
    ancestors.iter().any(|c| {
        c.rsplit_once('.')
            .is_some_and(|(_, ext)| PACKAGE_EXTENSIONS.contains(&ext.to_lowercase().as_str()))
    })
}

/// Best first, at most `limit`.
pub(super) fn rank(
    query: &str,
    mut results: Vec<FileResult>,
    now: f64,
    limit: usize,
) -> Vec<FileResult> {
    let query = fold(query.trim());
    let words: Vec<String> = query.split_whitespace().map(str::to_owned).collect();
    let mut scored: Vec<(i64, FileResult)> = results
        .drain(..)
        .map(|result| (score(&query, &words, &result, now), result))
        .collect();
    scored.sort_by(|(a, ra), (b, rb)| {
        b.cmp(a)
            .then(rb.modified.total_cmp(&ra.modified))
            .then_with(|| ra.name.cmp(&rb.name))
    });
    scored.truncate(limit);
    scored.into_iter().map(|(_, result)| result).collect()
}

fn score(query: &str, words: &[String], result: &FileResult, now: f64) -> i64 {
    // Finder's display name may hide the extension, and Spotlight matched the file system
    // name, so the better of the two counts.
    let file_name = result.path.rsplit('/').next().unwrap_or(&result.name);
    let quality = [result.name.as_str(), file_name]
        .into_iter()
        .map(|name| name_quality(query, words, &fold(name)))
        .max()
        .unwrap_or(0);

    let age_days = ((now - result.modified) / 86_400.0).max(0.0);
    let recency = (RECENCY_MAX - 100.0 * (1.0 + age_days).ln()).max(0.0) as i64;
    let depth = result.path.matches('/').count() as i64;

    quality + recency - DEPTH_PENALTY * depth
}

/// The whole query as one phrase first; failing that, the weakest of its words decides, so
/// every word has to match well for a high score.
fn name_quality(query: &str, words: &[String], name: &str) -> i64 {
    let stem = name.rsplit_once('.').map_or(name, |(stem, _)| stem);
    phrase_quality(query, name, stem).unwrap_or_else(|| {
        words
            .iter()
            .map(|word| phrase_quality(word, name, stem).unwrap_or(0))
            .min()
            .unwrap_or(0)
            .min(MATCH_WORD_START)
    })
}

fn phrase_quality(phrase: &str, name: &str, stem: &str) -> Option<i64> {
    if phrase.is_empty() {
        return None;
    }
    if name == phrase || stem == phrase {
        Some(MATCH_EXACT)
    } else if name.starts_with(phrase) {
        Some(MATCH_PREFIX)
    } else if starts_a_word(name, phrase) {
        Some(MATCH_WORD_START)
    } else if name.contains(phrase) {
        Some(MATCH_ANYWHERE)
    } else {
        None
    }
}

/// Whether `phrase` occurs right after a separator or at a lower→upper case change.
fn starts_a_word(name: &str, phrase: &str) -> bool {
    name.match_indices(phrase).any(|(i, _)| {
        name[..i]
            .chars()
            .next_back()
            .is_some_and(|c| !c.is_alphanumeric())
    })
}

/// Lowercase without diacritics, so "Résumé" matches "resume".
fn fold(s: &str) -> String {
    s.nfd()
        .filter(|c| !is_combining_mark(*c))
        .collect::<String>()
        .to_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: f64 = 1_800_000_000.0;
    const DAY: f64 = 86_400.0;

    fn file(path: &str, age_days: f64) -> FileResult {
        let name = path.rsplit('/').next().unwrap().to_owned();
        FileResult {
            path: path.into(),
            name,
            parent: String::new(),
            content_type: "public.data".into(),
            modified: NOW - age_days * DAY,
        }
    }

    fn names(results: &[FileResult]) -> Vec<&str> {
        results.iter().map(|r| r.name.as_str()).collect()
    }

    #[test]
    fn exact_beats_prefix_beats_word_start_beats_anywhere() {
        let results = vec![
            file("/Users/me/Documents/misreport.txt", 0.0),
            file("/Users/me/Documents/Q3 report.txt", 0.0),
            file("/Users/me/Documents/report draft.txt", 0.0),
            file("/Users/me/Documents/report.txt", 0.0),
        ];
        assert_eq!(
            names(&rank("report", results, NOW, 10)),
            vec![
                "report.txt",
                "report draft.txt",
                "Q3 report.txt",
                "misreport.txt"
            ]
        );
    }

    #[test]
    fn recency_breaks_ties_but_not_match_quality() {
        let results = vec![
            file("/Users/me/Documents/budget old.xlsx", 400.0),
            file("/Users/me/Documents/budget new.xlsx", 1.0),
            file("/Users/me/Documents/my budget.xlsx", 0.0),
        ];
        assert_eq!(
            names(&rank("budget", results, NOW, 10)),
            vec!["budget new.xlsx", "budget old.xlsx", "my budget.xlsx"]
        );
    }

    #[test]
    fn shallower_paths_win_otherwise_equal_matches() {
        let results = vec![
            file("/Users/me/Documents/a/b/c/notes.md", 3.0),
            file("/Users/me/Documents/notes.md", 3.0),
        ];
        let ranked = rank("notes", results, NOW, 10);
        assert_eq!(ranked[0].path, "/Users/me/Documents/notes.md");
    }

    #[test]
    fn every_word_counts_and_order_does_not_matter() {
        let results = vec![
            file("/Users/me/Documents/2025 Tax Return.pdf", 10.0),
            file("/Users/me/Documents/tax notes.txt", 0.0),
        ];
        let ranked = rank("tax 2025", results, NOW, 10);
        assert_eq!(ranked[0].name, "2025 Tax Return.pdf");
    }

    #[test]
    fn matching_ignores_case_and_diacritics() {
        let results = vec![
            file("/Users/me/Documents/other.txt", 0.0),
            file("/Users/me/Documents/Résumé.pdf", 30.0),
        ];
        assert_eq!(rank("resume", results, NOW, 10)[0].name, "Résumé.pdf");
    }

    #[test]
    fn a_hidden_extension_still_counts() {
        // Finder hides ".pdf" in the display name; the file name still has it.
        let mut invoice = file("/Users/me/Documents/invoice.pdf", 5.0);
        invoice.name = "invoice".into();
        let results = vec![file("/Users/me/Documents/pdf tools.txt", 0.0), invoice];
        let ranked = rank("invoice.pdf", results, NOW, 10);
        assert_eq!(ranked[0].path, "/Users/me/Documents/invoice.pdf");
    }

    #[test]
    fn respects_the_limit() {
        let results = (0..20)
            .map(|i| file(&format!("/Users/me/Documents/file{i}.txt"), i as f64))
            .collect();
        assert_eq!(rank("file", results, NOW, 5).len(), 5);
    }

    #[test]
    fn hidden_files_packages_and_library_are_excluded() {
        let home = "/Users/me";
        assert!(is_excluded("/Users/me/.config/x.toml", home));
        assert!(is_excluded("/Users/me/Documents/.hidden", home));
        assert!(is_excluded("/Users/me/Library/Caches/thing", home));
        assert!(is_excluded("/Users/me/Library", home));
        assert!(is_excluded(
            "/Users/me/Downloads/Foo.app/Contents/Info.plist",
            home
        ));
        assert!(is_excluded(
            "/Users/me/Documents/Talk.key/Data/image.png",
            home
        ));
        assert!(is_excluded(
            "/Users/me/Dev/App.xcodeproj/project.pbxproj",
            home
        ));

        // The package itself is a normal result, and so are look-alikes.
        assert!(!is_excluded("/Users/me/Downloads/Foo.app", home));
        assert!(!is_excluded("/Users/me/Documents/Talk.key", home));
        assert!(!is_excluded(
            "/Users/me/Documents/my.app.notes/readme.md",
            home
        ));
        assert!(!is_excluded("/Users/me/LibraryBooks/list.txt", home));
        assert!(!is_excluded("/Users/me/Documents/report.pdf", home));
    }
}
