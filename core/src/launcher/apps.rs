//! The app index: finds `.app` bundles in the usual folders and watches them for changes.
//!
//! Names are what Finder shows (`NSFileManager.displayName(atPath:)`): the folder name,
//! localized where the app provides it ("FindMy.app" → "Find My"). `CFBundleName` is often
//! a short internal name ("Code" for Visual Studio Code), so a differing folder or bundle
//! name is kept as an alias that still matches.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use notify::{RecommendedWatcher, RecursiveMode, Watcher as _};
use objc2::rc::autoreleasepool;
use objc2_foundation::{NSFileManager, NSString};
use plist::Value;

use super::AppInfo;
use super::fuzzy::Prepared;

/// Changes arrive in bursts (an install or update touches thousands of files), so the
/// index is rebuilt once this much time has passed without further events.
const DEBOUNCE: Duration = Duration::from_secs(1);

/// An indexed app, with its name prepared for matching.
#[derive(Debug, Clone)]
pub(crate) struct App {
    pub info: AppInfo,
    pub subtitle: String,
    pub title: Prepared,
    /// `CFBundleDisplayName` / `CFBundleName`, when it differs from the folder name.
    pub alias: Option<Prepared>,
}

/// Where to look, in order of preference: when two bundles share an id, the first wins.
fn roots() -> Vec<Root> {
    let mut roots = vec![Root::folder("/Applications", 2)];
    if let Some(home) = home_dir() {
        roots.push(Root::folder(home.join("Applications"), 2));
    }
    roots.extend([
        Root::folder("/System/Applications", 2),
        Root::bundle("/System/Library/CoreServices/Finder.app"),
        Root::folder("/System/Library/CoreServices/Applications", 1),
    ]);
    roots
}

/// Folders worth watching: the system folders only change with macOS updates.
fn watched_folders() -> Vec<PathBuf> {
    let mut folders = vec![PathBuf::from("/Applications")];
    if let Some(home) = home_dir() {
        folders.push(home.join("Applications"));
    }
    folders.into_iter().filter(|f| f.is_dir()).collect()
}

struct Root {
    path: PathBuf,
    /// How many levels of plain folders to descend into (bundles are never entered).
    depth: usize,
}

impl Root {
    fn folder(path: impl Into<PathBuf>, depth: usize) -> Self {
        Self {
            path: path.into(),
            depth,
        }
    }

    /// A single bundle, listed as-is.
    fn bundle(path: impl Into<PathBuf>) -> Self {
        Self::folder(path, 0)
    }
}

/// Scans every root. Blocking file I/O: call it off the main thread.
pub(crate) fn index() -> Vec<App> {
    let home = home_dir();
    let mut seen = HashSet::new();
    let mut apps = Vec::new();
    for root in roots() {
        let mut bundles = Vec::new();
        if is_app_bundle(&root.path) {
            bundles.push(root.path.clone());
        } else {
            collect_bundles(&root.path, root.depth, &mut bundles);
        }
        for path in bundles {
            if let Some(app) = read_bundle(&path, home.as_deref())
                && seen.insert(app.info.bundle_id.clone())
            {
                apps.push(app);
            }
        }
    }
    apps.sort_by_cached_key(|app| app.info.name.to_lowercase());
    apps
}

fn collect_bundles(dir: &Path, depth: usize, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    let mut entries: Vec<PathBuf> = entries.flatten().map(|e| e.path()).collect();
    entries.sort();
    for path in entries {
        let hidden = path
            .file_name()
            .and_then(|n| n.to_str())
            .is_none_or(|n| n.starts_with('.'));
        if hidden {
            continue;
        }
        if is_app_bundle(&path) {
            out.push(path);
        } else if depth > 0 && is_plain_folder(&path) {
            collect_bundles(&path, depth - 1, out);
        }
    }
}

fn is_app_bundle(path: &Path) -> bool {
    path.extension().is_some_and(|e| e == "app") && path.is_dir()
}

/// A folder that isn't a package: no extension, or Finder's `.localized` marker.
fn is_plain_folder(path: &Path) -> bool {
    path.is_dir() && path.extension().is_none_or(|e| e == "localized")
}

fn read_bundle(path: &Path, home: Option<&Path>) -> Option<App> {
    // iOS apps installed on Apple silicon keep their plist in a wrapper.
    let plist = ["Contents/Info.plist", "WrappedBundle/Info.plist"]
        .iter()
        .map(|p| path.join(p))
        .find(|p| p.is_file())?;
    let value = Value::from_file(plist).ok()?;
    let info = value.as_dictionary()?;

    let bundle_id = info.get("CFBundleIdentifier")?.as_string()?.to_owned();
    // Package types vary (Finder is "FNDR"), so agents are recognised by their keys alone.
    let is_agent = flag(info.get("LSUIElement")) || flag(info.get("LSBackgroundOnly"));
    if bundle_id.is_empty() || is_agent {
        return None;
    }

    let stem = path.file_stem()?.to_str()?.to_owned();
    let name = display_name(path).unwrap_or_else(|| stem.clone());
    // Whatever else the app is called still matches: its folder name ("FindMy") or
    // internal name ("Code").
    let alias = [Some(stem.as_str())]
        .into_iter()
        .chain(
            ["CFBundleDisplayName", "CFBundleName"]
                .iter()
                .map(|key| info.get(key).and_then(Value::as_string)),
        )
        .flatten()
        .find(|n| !n.is_empty() && !n.eq_ignore_ascii_case(&name));

    Some(App {
        subtitle: subtitle(path, home),
        title: Prepared::new(&name),
        alias: alias.map(Prepared::new),
        info: AppInfo {
            bundle_id,
            name,
            path: path.to_string_lossy().into_owned(),
        },
    })
}

/// The name Finder shows, localized for the user's language ("FindMy.app" → "Find My").
fn display_name(path: &Path) -> Option<String> {
    let name = autoreleasepool(|_| {
        let path = NSString::from_str(path.to_str()?);
        Some(
            NSFileManager::defaultManager()
                .displayNameAtPath(&path)
                .to_string(),
        )
    })?;
    // The extension is included when the user shows all extensions in Finder.
    let name = name.strip_suffix(".app").unwrap_or(&name).trim();
    (!name.is_empty()).then(|| name.to_owned())
}

/// Plist booleans show up as real booleans, `"1"`/`"YES"` strings, or integers.
fn flag(value: Option<&Value>) -> bool {
    match value {
        Some(Value::Boolean(b)) => *b,
        Some(Value::String(s)) => matches!(s.to_ascii_lowercase().as_str(), "1" | "yes" | "true"),
        Some(Value::Integer(i)) => i.as_unsigned() == Some(1),
        _ => false,
    }
}

/// "Application" for the standard folders; otherwise where the app lives, e.g.
/// "~/Applications/Chrome Apps".
fn subtitle(path: &Path, home: Option<&Path>) -> String {
    let standard = ["/Applications", "/System/Applications"]
        .iter()
        .any(|root| path.starts_with(root));
    if standard || path == Path::new("/System/Library/CoreServices/Finder.app") {
        return "Application".into();
    }
    if path.starts_with("/System/Library/CoreServices") {
        return "System Utility".into();
    }
    let Some(parent) = path.parent() else {
        return "Application".into();
    };
    let shown = match home.and_then(|h| parent.strip_prefix(h).ok()) {
        Some(rest) => Path::new("~").join(rest),
        None => parent.to_path_buf(),
    };
    shown.to_string_lossy().replace(".localized", "")
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

/// Watches the app folders with FSEvents and calls `on_change` once each burst of changes
/// settles. Stops when dropped.
pub(crate) struct Watcher {
    _watcher: RecommendedWatcher,
}

impl Watcher {
    pub fn start(on_change: impl Fn() + Send + 'static) -> Option<Self> {
        let (tx, rx) = mpsc::channel::<()>();
        let mut watcher =
            notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
                if event.is_ok() {
                    let _ = tx.send(());
                }
            })
            .ok()?;
        for folder in watched_folders() {
            // A folder that can't be watched just won't refresh live; the rest still do.
            let _ = watcher.watch(&folder, RecursiveMode::Recursive);
        }
        thread::Builder::new()
            .name("jaycast-app-watcher".into())
            .spawn(move || {
                // Ends when the watcher (and with it the sender) is dropped.
                while rx.recv().is_ok() {
                    while rx.recv_timeout(DEBOUNCE).is_ok() {}
                    on_change();
                }
            })
            .ok()?;
        Some(Self { _watcher: watcher })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subtitles() {
        let home = Path::new("/Users/me");
        let s = |p: &str| subtitle(Path::new(p), Some(home));
        assert_eq!(s("/Applications/Safari.app"), "Application");
        assert_eq!(s("/Applications/Utilities/Terminal.app"), "Application");
        assert_eq!(s("/System/Applications/Notes.app"), "Application");
        assert_eq!(s("/System/Library/CoreServices/Finder.app"), "Application");
        assert_eq!(
            s("/System/Library/CoreServices/Applications/Archive Utility.app"),
            "System Utility"
        );
        assert_eq!(
            s("/Users/me/Applications/Chrome Apps.localized/Gmail.app"),
            "~/Applications/Chrome Apps"
        );
    }

    #[test]
    fn plist_flags() {
        assert!(flag(Some(&Value::Boolean(true))));
        assert!(flag(Some(&Value::String("1".into()))));
        assert!(flag(Some(&Value::String("YES".into()))));
        assert!(flag(Some(&Value::Integer(1.into()))));
        assert!(!flag(Some(&Value::Boolean(false))));
        assert!(!flag(Some(&Value::String("0".into()))));
        assert!(!flag(None));
    }

    #[test]
    fn reads_bundles_and_skips_agents() {
        let dir = std::env::temp_dir().join(format!("jaycast-apps-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let make = |rel: &str, plist: &str| {
            let contents = dir.join(rel).join("Contents");
            fs::create_dir_all(&contents).unwrap();
            fs::write(contents.join("Info.plist"), plist).unwrap();
        };
        let plist = |extra: &str| {
            format!(
                r#"<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0"><dict>
<key>CFBundleIdentifier</key><string>com.example.app</string>
<key>CFBundleName</key><string>Code</string>{extra}
</dict></plist>"#
            )
        };
        make("Visual Studio Code.app", &plist(""));
        make("Agent.app", &plist("<key>LSUIElement</key><true/>"));
        fs::create_dir_all(dir.join("NoPlist.app")).unwrap();

        let app = read_bundle(&dir.join("Visual Studio Code.app"), None).unwrap();
        assert_eq!(app.info.name, "Visual Studio Code");
        assert_eq!(app.info.bundle_id, "com.example.app");
        assert!(app.alias.is_some(), "CFBundleName kept as an alias");
        assert!(read_bundle(&dir.join("Agent.app"), None).is_none());
        assert!(read_bundle(&dir.join("NoPlist.app"), None).is_none());

        let mut found = Vec::new();
        collect_bundles(&dir, 1, &mut found);
        assert_eq!(found.len(), 3, "bundles are found but never entered");
        fs::remove_dir_all(dir).unwrap();
    }

    /// Indexes this machine's apps. Run with `cargo test -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn index_this_machine() {
        let start = std::time::Instant::now();
        let apps = index();
        println!("indexed {} apps in {:?}", apps.len(), start.elapsed());
        for app in apps.iter().take(15) {
            println!(
                "  {} ({}) — {}",
                app.info.name, app.info.bundle_id, app.subtitle
            );
        }
        assert!(apps.iter().any(|a| a.info.bundle_id == "com.apple.finder"));
        assert!(apps.iter().any(|a| a.info.bundle_id == "com.apple.Safari"));
    }
}
