//! App Icons: gives apps custom icons from `.icns` files and puts them back when an update
//! clears them. `~/.config/turbo/icons/Spotify.icns` goes on `Spotify.app`.
//!
//! An icon is set the way Finder's Get Info does it (`NSWorkspace.setIcon`): a hidden
//! `Icon\r` file in the bundle holds the icon in its resource fork, and a Finder flag on the
//! bundle says to use it. Nothing in `Contents` changes, so the app's signature still
//! verifies. An update replaces the bundle, dropping both.
//!
//! Each `Icon\r` we write is stamped with an extended attribute holding the hash of the
//! `.icns` it came from. That makes a sync stateless: a bundle whose stamp matches is done,
//! one without a matching stamp gets the icon (again), and a stamped bundle with no `.icns`
//! (the file was removed, or the feature turned off) gets its own icon back.

use std::collections::{HashMap, HashSet};
use std::ffi::{CStr, CString};
use std::fs;
use std::os::unix::ffi::OsStrExt as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use notify::{RecommendedWatcher, RecursiveMode, Watcher as _};
use objc2::AnyThread as _;
use objc2::rc::autoreleasepool;
use objc2_app_kit::{NSImage, NSWorkspace, NSWorkspaceIconCreationOptions};
use objc2_foundation::NSString;
use parking_lot::Mutex;
use sha2::{Digest, Sha256};

use crate::config::AppIconsConfig;

/// The custom icon file Finder and `NSWorkspace` keep inside a bundle.
const ICON_FILE: &str = "Icon\r";
/// On our `Icon\r` files: the hash of the `.icns` the icon came from.
const STAMP: &CStr = c"com.jamieweavis.turbo.icon";
/// An update touches a bundle many times; wait for it to finish.
const DEBOUNCE: Duration = Duration::from_secs(2);

/// One `.icns` in the icons folder and the app it's for.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct AppIcon {
    /// The file's name without `.icns`, which is the app's name.
    pub name: String,
    pub icon_path: String,
    /// The matching app, when it's installed.
    pub app_path: Option<String>,
    pub status: AppIconStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum AppIconStatus {
    /// The app shows this icon.
    Applied,
    /// App Icons is off, so the app has its own icon.
    Off,
    /// No app with this name in the Applications folders.
    AppNotFound,
    /// The app belongs to the system or another user; changing it needs an administrator.
    NeedsAdmin,
    /// macOS refused the change, which it does without the App Management permission.
    Blocked,
    /// The file isn't an image macOS can read.
    Unreadable,
}

/// Tells the shell when the icons or what they're applied to changed. Called from
/// background threads.
#[uniffi::export(with_foreign)]
pub trait AppIconsObserver: Send + Sync {
    fn app_icons_changed(&self, icons: Vec<AppIcon>);
}

pub(crate) struct AppIcons {
    shared: Arc<Shared>,
    /// Running only while the feature is on.
    watcher: Mutex<Option<Watcher>>,
}

struct Shared {
    icon_dir: PathBuf,
    app_dirs: Vec<PathBuf>,
    enabled: AtomicBool,
    /// Held for a whole sync, so two never interleave. Never taken by the shell's calls.
    refused: Mutex<Refused>,
    /// What the last sync found, for the shell.
    icons: Mutex<Vec<AppIcon>>,
    observer: Mutex<Option<Arc<dyn AppIconsObserver>>>,
}

/// Changes that failed, by app, with the stamp that was wanted (empty for a restore). A sync
/// set off by the watcher doesn't try them again: a change that keeps failing must not
/// retrigger itself, or pester with a permission notice each time.
type Refused = HashMap<PathBuf, (String, AppIconStatus)>;

impl AppIcons {
    /// `icon_dir` is the folder of `.icns` files. Syncs once in the background even when
    /// off, so icons left behind by switching it off in the config file are taken back.
    pub fn new(config: &AppIconsConfig, icon_dir: PathBuf) -> Self {
        Self::with_dirs(config, icon_dir, default_app_dirs())
    }

    fn with_dirs(config: &AppIconsConfig, icon_dir: PathBuf, app_dirs: Vec<PathBuf>) -> Self {
        let icons = Self {
            shared: Arc::new(Shared {
                icon_dir,
                app_dirs,
                enabled: AtomicBool::new(false),
                refused: Mutex::new(Refused::new()),
                icons: Mutex::new(Vec::new()),
                observer: Mutex::new(None),
            }),
            watcher: Mutex::new(None),
        };
        icons.set_config(config);
        icons
    }

    /// On: applies the icons and keeps them applied. Off: stops watching and gives every
    /// app we changed its own icon back.
    pub fn set_config(&self, config: &AppIconsConfig) {
        let mut watcher = self.watcher.lock();
        self.shared.enabled.store(config.enabled, Ordering::Relaxed);
        if !config.enabled {
            *watcher = None;
        } else if watcher.is_none() {
            *watcher = Watcher::start(Arc::clone(&self.shared));
        }
        drop(watcher);
        self.sync();
    }

    pub fn set_observer(&self, observer: Option<Arc<dyn AppIconsObserver>>) {
        *self.shared.observer.lock() = observer;
    }

    /// What the last sync found. Never blocks on the file system.
    pub fn icons(&self) -> Vec<AppIcon> {
        self.shared.icons.lock().clone()
    }

    pub fn icon_dir(&self) -> &Path {
        &self.shared.icon_dir
    }

    /// Syncs in the background, trying again whatever failed before.
    pub fn sync(&self) {
        let shared = Arc::clone(&self.shared);
        let spawned = thread::Builder::new()
            .name("turbo-app-icons".into())
            .spawn(move || shared.sync(true));
        if let Err(error) = spawned {
            log::error!("couldn't start an app icon sync: {error}");
        }
    }
}

impl Shared {
    /// Makes the apps match the icons folder (or, when off, an empty one).
    fn sync(&self, retry: bool) {
        let mut refused = self.refused.lock();
        if retry {
            refused.clear();
        }
        let enabled = self.enabled.load(Ordering::Relaxed);
        if enabled && let Err(error) = fs::create_dir_all(&self.icon_dir) {
            log::warn!("couldn't create {}: {error}", self.icon_dir.display());
        }

        let mut themed = HashSet::new();
        let mut icons = Vec::new();
        for (name, icon_path) in icon_files(&self.icon_dir) {
            let app = find_app(&self.app_dirs, &name);
            let status = match &app {
                None => AppIconStatus::AppNotFound,
                Some(_) if !enabled => AppIconStatus::Off,
                Some(app) => {
                    themed.insert(app.clone());
                    apply(app, &icon_path, &mut refused)
                }
            };
            icons.push(AppIcon {
                name,
                icon_path: icon_path.to_string_lossy().into_owned(),
                app_path: app.map(|app| app.to_string_lossy().into_owned()),
                status,
            });
        }

        for app in self.app_dirs.iter().flat_map(|dir| apps_in(dir)) {
            if !themed.contains(&app) && stamp(&app).is_some() {
                restore(&app, &mut refused);
            }
        }
        drop(refused);

        let changed = {
            let mut current = self.icons.lock();
            let changed = *current != icons;
            current.clone_from(&icons);
            changed
        };
        let observer = self.observer.lock().clone();
        if changed && let Some(observer) = observer {
            observer.app_icons_changed(icons);
        }
    }
}

/// Puts the icon on the app unless it's already there.
fn apply(app: &Path, icon: &Path, refused: &mut Refused) -> AppIconStatus {
    let Ok(bytes) = fs::read(icon) else {
        return AppIconStatus::Unreadable;
    };
    let wanted = hex(&Sha256::digest(&bytes));
    if stamp(app).as_deref() == Some(&wanted) {
        return AppIconStatus::Applied;
    }
    if let Some((stamp, status)) = refused.get(app)
        && *stamp == wanted
    {
        return *status;
    }
    let status = if !writable(app) {
        AppIconStatus::NeedsAdmin
    } else {
        match set_icon(app, Some(icon)) {
            Ok(()) => {
                if let Err(error) = set_stamp(app, &wanted) {
                    // Unstamped it would look unapplied to every later sync.
                    log::error!("couldn't stamp {}'s icon: {error}", app.display());
                    refused.insert(app.to_owned(), (wanted, AppIconStatus::Applied));
                }
                log::info!("applied an icon to {}", app.display());
                return AppIconStatus::Applied;
            }
            Err(SetIconError::Unreadable) => AppIconStatus::Unreadable,
            Err(SetIconError::Refused) => AppIconStatus::Blocked,
        }
    };
    log::warn!("couldn't apply an icon to {}: {status:?}", app.display());
    refused.insert(app.to_owned(), (wanted, status));
    status
}

/// Gives an app we changed its own icon back.
fn restore(app: &Path, refused: &mut Refused) {
    if refused.contains_key(app) {
        return;
    }
    if set_icon(app, None).is_ok() {
        log::info!("restored {}'s own icon", app.display());
    } else {
        log::warn!("couldn't restore {}'s own icon", app.display());
        refused.insert(app.to_owned(), (String::new(), AppIconStatus::Blocked));
    }
}

enum SetIconError {
    /// The `.icns` didn't load as an image.
    Unreadable,
    /// `NSWorkspace` couldn't change the bundle.
    Refused,
}

/// Sets the app's custom icon, or with `None` removes it.
fn set_icon(app: &Path, icon: Option<&Path>) -> Result<(), SetIconError> {
    // `setIcon:forFile:options:` isn't thread-safe: concurrent calls, even on different
    // bundles, fail or crash. A sync already serialises its own calls, but this keeps any
    // two callers (two `Shared`s in tests, say) from overlapping.
    static SET_ICON: Mutex<()> = Mutex::new(());
    let _serial = SET_ICON.lock();
    autoreleasepool(|_| {
        let image = match icon {
            Some(icon) => {
                let path = NSString::from_str(&icon.to_string_lossy());
                let image = NSImage::initWithContentsOfFile(NSImage::alloc(), &path)
                    .ok_or(SetIconError::Unreadable)?;
                Some(image)
            }
            None => None,
        };
        let done = NSWorkspace::sharedWorkspace().setIcon_forFile_options(
            image.as_deref(),
            &NSString::from_str(&app.to_string_lossy()),
            NSWorkspaceIconCreationOptions(0),
        );
        if done {
            Ok(())
        } else {
            Err(SetIconError::Refused)
        }
    })
}

/// The `.icns` files in the folder as (app name, path), sorted by name.
fn icon_files(dir: &Path) -> Vec<(String, PathBuf)> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut files: Vec<(String, PathBuf)> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("icns"))
        })
        .filter_map(|path| {
            let name = path.file_stem()?.to_str()?.to_owned();
            (!name.starts_with('.')).then_some((name, path))
        })
        .collect();
    files.sort_by_cached_key(|(name, _)| name.to_lowercase());
    files
}

/// `<name>.app` in the first Applications folder that has it. A symlink isn't followed,
/// which leaves out the system apps macOS links into `/Applications`.
fn find_app(app_dirs: &[PathBuf], name: &str) -> Option<PathBuf> {
    app_dirs
        .iter()
        .map(|dir| dir.join(format!("{name}.app")))
        .find(|app| is_bundle(app))
}

fn apps_in(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "app") && is_bundle(path))
        .collect()
}

fn is_bundle(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok_and(|meta| meta.is_dir())
}

/// `/Applications` and `~/Applications`, or only `$TURBO_APPS_DIR` when set (`just
/// screenshots` points it at stand-in bundles, so a demo config never changes real apps).
fn default_app_dirs() -> Vec<PathBuf> {
    if let Some(dir) = std::env::var_os("TURBO_APPS_DIR").filter(|dir| !dir.is_empty()) {
        return vec![PathBuf::from(dir)];
    }
    let mut dirs = vec![PathBuf::from("/Applications")];
    if let Some(home) = std::env::home_dir() {
        dirs.push(home.join("Applications"));
    }
    dirs
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn c_path(path: &Path) -> Option<CString> {
    CString::new(path.as_os_str().as_bytes()).ok()
}

fn writable(path: &Path) -> bool {
    // SAFETY: a valid NUL-terminated path; `access` only reads it.
    c_path(path).is_some_and(|path| unsafe { libc::access(path.as_ptr(), libc::W_OK) } == 0)
}

/// The stamp on the app's `Icon\r`, if we put its icon there.
fn stamp(app: &Path) -> Option<String> {
    let path = c_path(&app.join(ICON_FILE))?;
    let mut value = [0u8; 128];
    // SAFETY: valid NUL-terminated path and name; at most `value.len()` bytes are written
    // to `value`.
    let length = unsafe {
        libc::getxattr(
            path.as_ptr(),
            STAMP.as_ptr(),
            value.as_mut_ptr().cast(),
            value.len(),
            0,
            0,
        )
    };
    let length = usize::try_from(length).ok()?;
    Some(String::from_utf8_lossy(&value[..length]).into_owned())
}

fn set_stamp(app: &Path, stamp: &str) -> std::io::Result<()> {
    let path = c_path(&app.join(ICON_FILE)).ok_or(std::io::ErrorKind::InvalidInput)?;
    // SAFETY: valid NUL-terminated path and name; `stamp.len()` bytes are read from `stamp`.
    let status = unsafe {
        libc::setxattr(
            path.as_ptr(),
            STAMP.as_ptr(),
            stamp.as_ptr().cast(),
            stamp.len(),
            0,
            0,
        )
    };
    if status == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

/// Watches the icons folder and the Applications folders with FSEvents, syncing once each
/// burst of changes settles. Stops when dropped.
struct Watcher {
    _watcher: RecommendedWatcher,
}

impl Watcher {
    fn start(shared: Arc<Shared>) -> Option<Self> {
        if let Err(error) = fs::create_dir_all(&shared.icon_dir) {
            log::warn!("couldn't create {}: {error}", shared.icon_dir.display());
        }
        let (tx, rx) = mpsc::channel::<()>();
        let filter = Arc::clone(&shared);
        let mut watcher =
            notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
                if event.is_ok_and(|event| event.paths.iter().any(|path| filter.concerns(path))) {
                    let _ = tx.send(());
                }
            })
            .inspect_err(|error| log::error!("couldn't start the app icon watcher: {error}"))
            .ok()?;
        for folder in shared.app_dirs.iter().chain([&shared.icon_dir]) {
            if !folder.is_dir() {
                continue;
            }
            // A folder that can't be watched just won't sync live; the rest still do.
            if let Err(error) = watcher.watch(folder, RecursiveMode::Recursive) {
                log::warn!("couldn't watch {}: {error}", folder.display());
            }
        }
        thread::Builder::new()
            .name("turbo-app-icon-watcher".into())
            .spawn(move || {
                // Ends when the watcher (and with it the sender) is dropped.
                while rx.recv().is_ok() {
                    while rx.recv_timeout(DEBOUNCE).is_ok() {}
                    shared.sync(false);
                }
            })
            .inspect_err(|error| log::error!("couldn't start the app icon watcher: {error}"))
            .ok()?;
        Some(Self { _watcher: watcher })
    }
}

impl Shared {
    /// Whether a changed path could change what a sync does: an icon file, a bundle
    /// appearing or going, or something directly inside a bundle (its `Icon\r`). The
    /// thousands of files deeper in a bundle that an update rewrites don't count.
    fn concerns(&self, path: &Path) -> bool {
        path.starts_with(&self.icon_dir)
            || self.app_dirs.iter().any(|dir| {
                path.strip_prefix(dir)
                    .is_ok_and(|rest| rest.components().count() <= 2)
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ICNS: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/Resources/AppIcon.icns");

    struct Fixture {
        root: PathBuf,
        shared: Shared,
    }

    impl Fixture {
        fn new(name: &str) -> Self {
            let root =
                std::env::temp_dir().join(format!("turbo-app-icons-{name}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&root);
            fs::create_dir_all(root.join("icons")).unwrap();
            fs::create_dir_all(root.join("apps")).unwrap();
            let shared = Shared {
                icon_dir: root.join("icons"),
                app_dirs: vec![root.join("apps")],
                enabled: AtomicBool::new(true),
                refused: Mutex::new(Refused::new()),
                icons: Mutex::new(Vec::new()),
                observer: Mutex::new(None),
            };
            Self { root, shared }
        }

        fn app(&self, name: &str) -> PathBuf {
            let app = self.root.join("apps").join(format!("{name}.app"));
            fs::create_dir_all(app.join("Contents")).unwrap();
            app
        }

        fn icon(&self, name: &str) -> PathBuf {
            let icon = self.root.join("icons").join(format!("{name}.icns"));
            fs::copy(ICNS, &icon).unwrap();
            icon
        }

        fn statuses(&self) -> Vec<(String, AppIconStatus)> {
            self.shared.sync(false);
            let icons = self.shared.icons.lock();
            icons.iter().map(|i| (i.name.clone(), i.status)).collect()
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    #[test]
    fn matches_icons_to_apps_by_name() {
        let fixture = Fixture::new("match");
        fixture.shared.enabled.store(false, Ordering::Relaxed);
        fixture.app("Beta");
        fixture.icon("beta-less");
        fixture.icon("Beta");
        fs::write(fixture.root.join("icons/notes.txt"), "x").unwrap();
        fs::write(fixture.root.join("icons/.hidden.icns"), "x").unwrap();
        assert_eq!(
            fixture.statuses(),
            [
                ("Beta".to_owned(), AppIconStatus::Off),
                ("beta-less".to_owned(), AppIconStatus::AppNotFound),
            ]
        );
    }

    /// Sets a real custom icon, on a folder of its own.
    #[test]
    fn applies_reapplies_and_restores() {
        let fixture = Fixture::new("apply");
        let app = fixture.app("Foo");
        let icon = fixture.icon("Foo");
        let applied = [("Foo".to_owned(), AppIconStatus::Applied)];

        assert_eq!(fixture.statuses(), applied);
        let first = stamp(&app).expect("stamped");
        assert!(app.join(ICON_FILE).exists());

        // An update replaces the bundle, losing the icon; the next sync puts it back.
        fs::remove_dir_all(&app).unwrap();
        fixture.app("Foo");
        assert_eq!(stamp(&app), None);
        assert_eq!(fixture.statuses(), applied);
        assert_eq!(stamp(&app), Some(first.clone()));

        // A different image under the same name replaces the icon.
        let mut bytes = fs::read(&icon).unwrap();
        bytes.push(0);
        fs::write(&icon, bytes).unwrap();
        assert_eq!(fixture.statuses(), applied);
        assert_ne!(stamp(&app), Some(first));

        // Off gives the app its own icon back.
        fixture.shared.enabled.store(false, Ordering::Relaxed);
        assert_eq!(fixture.statuses(), [("Foo".to_owned(), AppIconStatus::Off)]);
        assert!(!app.join(ICON_FILE).exists());

        // So does removing the file while on.
        fixture.shared.enabled.store(true, Ordering::Relaxed);
        assert_eq!(fixture.statuses(), applied);
        fs::remove_file(&icon).unwrap();
        assert_eq!(fixture.statuses(), []);
        assert!(!app.join(ICON_FILE).exists());
    }

    #[test]
    fn leaves_icons_it_did_not_set() {
        let fixture = Fixture::new("foreign");
        let app = fixture.app("Mine");
        assert!(set_icon(&app, Some(Path::new(ICNS))).is_ok());
        assert_eq!(fixture.statuses(), []);
        assert!(app.join(ICON_FILE).exists());
    }

    #[test]
    fn a_file_that_is_not_an_image_is_unreadable() {
        let fixture = Fixture::new("unreadable");
        fixture.app("Bad");
        fs::write(fixture.root.join("icons/Bad.icns"), "not an image").unwrap();
        assert_eq!(
            fixture.statuses(),
            [("Bad".to_owned(), AppIconStatus::Unreadable)]
        );
        // Remembered, so the watcher doesn't keep trying.
        assert_eq!(fixture.shared.refused.lock().len(), 1);
    }

    #[test]
    fn only_shallow_paths_concern_the_watcher() {
        let fixture = Fixture::new("concerns");
        let apps = fixture.root.join("apps");
        let concerns = |path: PathBuf| fixture.shared.concerns(&path);
        assert!(concerns(fixture.root.join("icons/Foo.icns")));
        assert!(concerns(apps.join("Foo.app")));
        assert!(concerns(apps.join("Foo.app").join(ICON_FILE)));
        assert!(!concerns(apps.join("Foo.app/Contents/Info.plist")));
        assert!(!concerns(fixture.root.join("elsewhere")));
    }
}
