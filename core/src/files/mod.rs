//! File search through Spotlight, scoped to the folders the user chose.
//!
//! [`query`] turns the typed words into a Spotlight query, [`spotlight`] runs it with the
//! `MDQuery` C API, and [`rank`] filters out hidden and package-internal files and orders the
//! rest by name match, recency and depth. Searches block briefly, so the shell calls them off
//! the main thread.

mod query;
mod rank;
mod spotlight;

use parking_lot::Mutex;
use std::path::Path;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use objc2::rc::autoreleasepool;
use objc2_app_kit::NSWorkspace;
use objc2_foundation::{NSArray, NSString, NSURL};

use crate::config::FileSearchConfig;
use crate::error::CoreError;

/// Spotlight is asked for more than the caller wants, since some matches are filtered out
/// (hidden, inside packages) and ranking needs a pool to choose from.
const SPOTLIGHT_MAX: usize = 400;

/// Spotlight queries slower than this are logged (at debug level).
const SLOW_SEARCH: Duration = Duration::from_millis(50);

#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct FileResult {
    pub path: String,
    /// Finder's display name.
    pub name: String,
    /// The containing folder, abbreviated with `~`.
    pub parent: String,
    /// Uniform type identifier, e.g. `com.adobe.pdf`, `public.folder`.
    pub content_type: String,
    /// Seconds since 1970.
    pub modified: f64,
}

pub(crate) struct FileSearch {
    config: Mutex<FileSearchConfig>,
}

impl FileSearch {
    pub fn new(config: FileSearchConfig) -> Self {
        Self {
            config: Mutex::new(config),
        }
    }

    pub fn set_config(&self, config: FileSearchConfig) {
        *self.config.lock() = config;
    }

    /// Spotlight matches for `query` inside the configured folders, best first. Blocks for
    /// the query (bounded to a fraction of a second): call it off the main thread.
    pub fn search(&self, query: &str, limit: usize) -> Result<Vec<FileResult>, CoreError> {
        let Some(query_string) = query::build(query) else {
            return Ok(Vec::new());
        };
        let home = home_dir();
        let scopes = existing_scopes(&self.config.lock().scopes, &home);
        // No folders means no results, never "search everything".
        if scopes.is_empty() || limit == 0 {
            return Ok(Vec::new());
        }

        let started = Instant::now();
        let results: Vec<FileResult> = spotlight::run(&query_string, &scopes, SPOTLIGHT_MAX)
            .inspect_err(|error| log::error!("Spotlight search failed: {error}"))?
            .into_iter()
            .filter(|hit| !rank::is_excluded(&hit.path, &home))
            .map(|hit| FileResult {
                parent: abbreviate(parent_of(&hit.path), &home),
                path: hit.path,
                name: hit.name,
                content_type: hit.content_type,
                modified: hit.modified,
            })
            .collect();
        // The time and count only: never the query.
        if started.elapsed() > SLOW_SEARCH {
            log::debug!(
                "slow file search: {} hits in {:?}",
                results.len(),
                started.elapsed()
            );
        }
        Ok(rank::rank(query, results, now(), limit))
    }

    /// Opens the file with its default app.
    pub fn open(&self, path: &str) -> Result<(), CoreError> {
        require_exists(path)?;
        let opened = autoreleasepool(|_| {
            let url = NSURL::fileURLWithPath(&NSString::from_str(path));
            NSWorkspace::sharedWorkspace().openURL(&url)
        });
        if opened {
            Ok(())
        } else {
            Err(CoreError::Other {
                message: format!("couldn't open {path}"),
            })
        }
    }

    /// Selects the file in a Finder window.
    pub fn reveal(&self, path: &str) -> Result<(), CoreError> {
        require_exists(path)?;
        autoreleasepool(|_| {
            let url = NSURL::fileURLWithPath(&NSString::from_str(path));
            let urls = NSArray::from_retained_slice(&[url]);
            NSWorkspace::sharedWorkspace().activateFileViewerSelectingURLs(&urls);
        });
        Ok(())
    }
}

fn require_exists(path: &str) -> Result<(), CoreError> {
    if Path::new(path).exists() {
        Ok(())
    } else {
        Err(CoreError::Other {
            message: format!("{path} no longer exists"),
        })
    }
}

fn home_dir() -> String {
    std::env::var("HOME").unwrap_or_default()
}

/// Expands `~` and drops folders that don't exist (Spotlight errors on bad scopes) and
/// duplicates.
fn existing_scopes(scopes: &[String], home: &str) -> Vec<String> {
    let mut existing: Vec<String> = Vec::new();
    for scope in scopes {
        let expanded = expand_tilde(scope.trim(), home);
        if expanded.is_empty() || existing.contains(&expanded) {
            continue;
        }
        if Path::new(&expanded).is_dir() {
            existing.push(expanded);
        }
    }
    existing
}

fn expand_tilde(path: &str, home: &str) -> String {
    if path == "~" {
        home.to_owned()
    } else if let Some(rest) = path.strip_prefix("~/") {
        format!("{}/{rest}", home.trim_end_matches('/'))
    } else {
        path.to_owned()
    }
}

fn parent_of(path: &str) -> &str {
    match path.trim_end_matches('/').rsplit_once('/') {
        Some(("", _)) => "/",
        Some((parent, _)) => parent,
        None => "",
    }
}

/// `/Users/me/Documents` → `~/Documents`.
fn abbreviate(path: &str, home: &str) -> String {
    let home = home.trim_end_matches('/');
    if home.is_empty() {
        return path.to_owned();
    }
    if path == home {
        "~".to_owned()
    } else if let Some(rest) = path.strip_prefix(home).and_then(|r| r.strip_prefix('/')) {
        format!("~/{rest}")
    } else {
        path.to_owned()
    }
}

fn now() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0.0, |d| d.as_secs_f64())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tilde_expands_against_home() {
        assert_eq!(expand_tilde("~", "/Users/me"), "/Users/me");
        assert_eq!(
            expand_tilde("~/Documents", "/Users/me/"),
            "/Users/me/Documents"
        );
        assert_eq!(expand_tilde("/Volumes/Work", "/Users/me"), "/Volumes/Work");
        assert_eq!(expand_tilde("~other/x", "/Users/me"), "~other/x");
    }

    #[test]
    fn parents_are_abbreviated_with_tilde() {
        assert_eq!(
            abbreviate("/Users/me/Documents", "/Users/me"),
            "~/Documents"
        );
        assert_eq!(abbreviate("/Users/me", "/Users/me"), "~");
        assert_eq!(
            abbreviate("/Users/meghan/x", "/Users/me"),
            "/Users/meghan/x"
        );
        assert_eq!(abbreviate("/Volumes/Work", "/Users/me"), "/Volumes/Work");
        assert_eq!(
            parent_of("/Users/me/Documents/a.txt"),
            "/Users/me/Documents"
        );
        assert_eq!(parent_of("/a.txt"), "/");
    }

    #[test]
    fn missing_and_duplicate_scopes_are_dropped() {
        let home = std::env::temp_dir().to_string_lossy().into_owned();
        let scopes = vec![
            "~".to_owned(),
            home.clone(),
            "~/definitely-not-a-folder-jaycast".to_owned(),
            "  ".to_owned(),
        ];
        assert_eq!(existing_scopes(&scopes, &home), vec![home]);
    }

    #[test]
    fn blank_query_or_no_scopes_returns_nothing() {
        let search = FileSearch::new(FileSearchConfig::default());
        assert!(search.search("   ", 10).unwrap().is_empty());

        let search = FileSearch::new(FileSearchConfig {
            enabled: true,
            hotkey: None,
            scopes: Vec::new(),
        });
        assert!(search.search("report", 10).unwrap().is_empty());
    }

    #[test]
    fn opening_a_missing_file_is_an_error() {
        let search = FileSearch::new(FileSearchConfig::default());
        assert!(search.open("/definitely/not/here.txt").is_err());
        assert!(search.reveal("/definitely/not/here.txt").is_err());
    }

    /// Searches this Mac's real folders; run with `cargo test -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn live_search_timings() {
        let search = FileSearch::new(FileSearchConfig {
            enabled: true,
            hotkey: None,
            scopes: ["~/Desktop", "~/Documents", "~/Downloads"]
                .map(String::from)
                .to_vec(),
        });
        for query in ["a", "superwhisper", "max", "log", "default json", "zzqqxx"] {
            let start = std::time::Instant::now();
            let results = search.search(query, 8).unwrap();
            println!(
                "{query:>12} {:>7.1?}: {} results {:?}",
                start.elapsed(),
                results.len(),
                results
                    .iter()
                    .take(4)
                    .map(|r| format!("{} ({})", r.name, r.parent))
                    .collect::<Vec<_>>()
            );
        }
    }
}
