//! The launcher: app index, fuzzy search ranked by usage, system actions and hotkeys.
//!
//! Module ownership (so parallel work doesn't collide):
//! - `mod.rs`, `apps`, `fuzzy`, `ranking`: the app index (with change watching), matching,
//!   usage-based ranking, and `search` / `open`.
//! - `system`: the system actions (lock, sleep, dark mode, …) listed by `system::actions`
//!   and run by `system::perform`.
//! - `settings_panes`: System Settings panes and privacy sections, opened by URL.
//! - `hotkeys`: global hotkeys (palette + per-app) and the focus-or-hide app toggle.
//! - `running`: running apps for the palette's actions menu (is running, hide, quit).
//! - `calc`: calculator, unit and currency conversion (`Calculator::evaluate`).
//! - `extensions`: quicklinks and shell commands (`extensions::results` / `open`).
//!
//! Window actions (`crate::windows`) and snippets (`crate::snippets`) are listed and opened
//! from here, but implemented in their own modules.

mod apps;
mod calc;
mod extensions;
pub(crate) mod fuzzy;
mod hotkeys;
mod ranking;
pub(crate) mod running;
mod settings_panes;
mod system;

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex, RwLock};
use std::thread;
use std::time::{SystemTime, UNIX_EPOCH};

use objc2::rc::autoreleasepool;
use objc2_app_kit::{NSWorkspace, NSWorkspaceOpenConfiguration};
use objc2_foundation::{NSString, NSURL};

use crate::config::{LauncherConfig, WindowAction};
use crate::error::CoreError;
use crate::keep_awake::{KeepAwake, KeepAwakeCommand};
use crate::windows::{self, WindowManager};
use apps::App;
use calc::Calculator;
use fuzzy::Prepared;
use ranking::{Usage, UsageStore};

pub(crate) use hotkeys::HotkeyManager;

#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct SearchResult {
    /// Opaque; pass back to `Core::open_result`. Apps use `app:<bundle id>`, system actions
    /// `system:<action id>`, settings panes `settings:<pane id>`, window actions
    /// `window:<action id>`, snippets `snippet:<id>`.
    pub id: String,
    pub title: String,
    pub subtitle: String,
    pub kind: ResultKind,
    /// The app (or settings pane) bundle's path, for the shell to draw its icon.
    pub path: Option<String>,
    /// An SF Symbol name, for results without a bundle (system actions).
    pub symbol: Option<String>,
    /// Destructive or disruptive (restart, empty trash, …): the shell confirms first.
    pub needs_confirmation: bool,
    /// When set, choosing the result fills the search field with this instead of opening it,
    /// e.g. `g ` for a quicklink that still needs its argument.
    pub completion: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum ResultKind {
    Application,
    SystemAction,
    /// A System Settings pane or section.
    SettingsPane,
    Calculator,
    Quicklink,
    Command,
    /// Acts on the frontmost app's focused window: the shell closes the palette first.
    WindowAction,
    /// Types into the frontmost app: the shell closes the palette first.
    Snippet,
}

/// What opening a result did, so the palette can confirm it.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Enum)]
pub enum OpenOutcome {
    Done,
    /// The text was copied to the clipboard (calculator results).
    Copied {
        text: String,
    },
    /// Switch the palette to another mode (clipboard history, file search, colours).
    ShowMode {
        mode: PaletteMode,
    },
    /// Close the palette and show the colour picker's magnifier.
    PickColor,
    /// Keep Awake was turned on or off: close the palette and confirm it.
    KeepAwake {
        active: bool,
    },
}

/// What the palette is showing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, uniffi::Enum)]
pub enum PaletteMode {
    Search,
    Clipboard,
    Files,
    /// The colour picker's history.
    Colors,
    /// The emoji picker.
    Emoji,
}

/// Palette modes listed in search, so they can be found without their hotkeys:
/// (mode, id, title, keywords, SF Symbol).
type ModeEntry = (
    PaletteMode,
    &'static str,
    &'static str,
    &'static [&'static str],
    &'static str,
);

const MODES: &[ModeEntry] = &[
    (
        PaletteMode::Clipboard,
        "clipboard",
        "Clipboard History",
        &["paste", "copied", "pasteboard"],
        "doc.on.clipboard",
    ),
    (
        PaletteMode::Files,
        "files",
        "Search Files",
        &["find", "documents", "finder", "spotlight"],
        "doc.text.magnifyingglass",
    ),
    (
        PaletteMode::Colors,
        "colors",
        "List Colours",
        &[
            "list colors",
            "colour history",
            "color history",
            "picked",
            "swatches",
        ],
        "swatchpalette",
    ),
    (
        PaletteMode::Emoji,
        "emoji",
        "Search Emoji",
        &[
            "emoji picker",
            "emojis",
            "symbols",
            "smiley",
            "insert emoji",
        ],
        "face.smiling",
    ),
];

/// The launcher result that opens the colour picker's magnifier.
const PICK_COLOR_ID: &str = "color:pick";

/// Keep Awake's commands, like its hotkeys: (command, id, title, SF Symbol). Results use
/// `keep-awake:<id>`.
const KEEP_AWAKE_COMMANDS: &[(KeepAwakeCommand, &str, &str, &str)] = &[
    (
        KeepAwakeCommand::Toggle,
        "toggle",
        "Toggle Keep Awake",
        "eye",
    ),
    (KeepAwakeCommand::On, "on", "Turn On Keep Awake", "eye.fill"),
    (
        KeepAwakeCommand::Off,
        "off",
        "Turn Off Keep Awake",
        "eye.slash",
    ),
];

/// The coffee-flavoured names it went by, so they still find it.
const KEEP_AWAKE_KEYWORDS: &[&str] = &[
    "caffeinate",
    "caffeine",
    "coffee",
    "amphetamine",
    "stay awake",
    "prevent sleep",
    "no sleep",
];

fn keep_awake_command_for_id(id: &str) -> Option<KeepAwakeCommand> {
    KEEP_AWAKE_COMMANDS.iter().find(|c| c.1 == id).map(|c| c.0)
}

fn mode_for_id(id: &str) -> Option<PaletteMode> {
    MODES.iter().find(|m| m.1 == id).map(|m| m.0)
}

/// A system action that can be given a hotkey in settings.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct SystemActionInfo {
    /// Stable id, stored in `ActionHotkey::action`.
    pub id: String,
    pub title: String,
    /// SF Symbol name.
    pub symbol: String,
}

/// Every system action, in menu order (for choosing action hotkeys in settings).
#[uniffi::export]
pub fn system_actions() -> Vec<SystemActionInfo> {
    system::actions()
        .iter()
        .map(|action| SystemActionInfo {
            id: action.id.to_owned(),
            title: action.title.to_owned(),
            symbol: action.symbol.to_owned(),
        })
        .collect()
}

/// An app or built-in command, as listed in the launcher settings to set its hotkey, aliases
/// and whether it shows in search.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct LauncherItem {
    /// The search result id, used as `ItemSettings::id`.
    pub id: String,
    pub title: String,
    /// "Application", "Jaycast", "System", "System Settings" or "Window".
    pub category: String,
    /// The bundle to draw the icon from (apps, settings panes).
    pub path: Option<String>,
    /// An SF Symbol, for items without a bundle.
    pub symbol: Option<String>,
    /// Set for window actions, whose hotkeys live in the window settings.
    pub window_action: Option<WindowAction>,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct AppInfo {
    pub bundle_id: String,
    pub name: String,
    pub path: String,
}

/// Lets the shell react to launcher events. Called on the main thread for hotkeys, and from
/// a background thread for `apps_changed`.
#[uniffi::export(with_foreign)]
pub trait LauncherObserver: Send + Sync {
    /// The palette hotkey was pressed.
    fn toggle_palette(&self);
    /// The app index changed (an app was installed, removed or renamed).
    fn apps_changed(&self);
    /// A shell command with `CommandOutput::Show` finished (or any command failed).
    /// Called from a background thread.
    fn command_finished(&self, name: String, output: String, success: bool);
    /// A mode's hotkey was pressed: show the palette in that mode (or hide it if it's
    /// already showing that mode). Called on the main thread.
    fn show_mode(&self, mode: PaletteMode);
    /// Clipboard history changed (a new copy, or items expired). Called from a background
    /// thread.
    fn clipboard_changed(&self);
    /// A keep awake hotkey turned keeping the Mac awake on or off. Called on the main
    /// thread.
    fn keep_awake_changed(&self, active: bool);
    /// The colour picker hotkey was pressed: show the magnifier. Called on the main thread.
    fn pick_color(&self);
}

/// Calculator results go above everything; a quicklink or command whose keyword was typed
/// with an argument goes above fuzzy matches.
pub(crate) const CALCULATOR_SCORE: i32 = 20_000;
pub(crate) const KEYWORD_SCORE: i32 = 10_000;

/// Alias and keyword matches count for three quarters of a match on the title itself.
const SECONDARY_MATCH_WEIGHT: i32 = 3;
const SECONDARY_MATCH_DIVISOR: i32 = 4;

/// The user's per-item settings, prepared for search.
#[derive(Default)]
struct Prefs {
    disabled: std::collections::HashSet<String>,
    /// Result id → its aliases.
    aliases: HashMap<String, Vec<Alias>>,
}

struct Alias {
    prepared: Prepared,
    folded: Vec<char>,
}

impl Prefs {
    fn new(config: &LauncherConfig) -> Self {
        let mut prefs = Self::default();
        for item in &config.items {
            if !item.enabled {
                prefs.disabled.insert(item.id.clone());
            }
            let aliases: Vec<Alias> = item
                .aliases
                .iter()
                .map(|alias| alias.trim())
                .filter(|alias| !alias.is_empty())
                .map(|alias| Alias {
                    prepared: Prepared::new(alias),
                    folded: fuzzy::fold_query(alias),
                })
                .collect();
            if !aliases.is_empty() {
                prefs.aliases.insert(item.id.clone(), aliases);
            }
        }
        prefs
    }

    fn is_enabled(&self, id: &str) -> bool {
        !self.disabled.contains(id)
    }

    /// The best alias score for `id`: a full-weight match, or above every fuzzy match when
    /// the query is the alias itself.
    fn alias_score(&self, query: &[char], id: &str) -> Option<i32> {
        self.aliases
            .get(id)?
            .iter()
            .filter_map(|alias| {
                if alias.folded == query {
                    return Some(KEYWORD_SCORE);
                }
                fuzzy::score(query, &alias.prepared).map(|m| m.score)
            })
            .max()
    }
}

pub(crate) struct Launcher {
    index: RwLock<Arc<Index>>,
    system: Vec<SystemEntry>,
    usage: Mutex<Usage>,
    store: UsageStore,
    observer: Mutex<Option<Arc<dyn LauncherObserver>>>,
    watcher: Mutex<Option<apps::Watcher>>,
    config: RwLock<Arc<LauncherConfig>>,
    prefs: RwLock<Arc<Prefs>>,
    calculator: Calculator,
    windows: Arc<WindowManager>,
    keep_awake: Arc<KeepAwake>,
}

#[derive(Default)]
struct Index {
    /// Sorted by name.
    apps: Vec<App>,
    /// Result id (`app:<bundle id>`) → position in `apps`.
    by_id: HashMap<String, usize>,
}

impl Index {
    fn new(apps: Vec<App>) -> Self {
        let by_id = apps
            .iter()
            .enumerate()
            .map(|(i, app)| (app_result_id(&app.info.bundle_id), i))
            .collect();
        Self { apps, by_id }
    }

    fn same_apps(&self, other: &Index) -> bool {
        self.apps.len() == other.apps.len()
            && self
                .apps
                .iter()
                .zip(&other.apps)
                .all(|(a, b)| a.info == b.info)
    }
}

/// A built-in result (system action, settings pane, …) with its title and keywords prepared
/// for matching.
struct SystemEntry {
    id: String,
    title: String,
    subtitle: &'static str,
    title_match: Prepared,
    keywords: Vec<Prepared>,
    symbol: String,
    /// A bundle to draw the icon from, instead of `symbol`.
    path: Option<String>,
    needs_confirmation: bool,
    kind: ResultKind,
}

impl SystemEntry {
    fn new(action: &system::SystemAction) -> Self {
        Self {
            id: format!("system:{}", action.id),
            title: action.title.to_owned(),
            subtitle: "System",
            title_match: Prepared::new(action.title),
            keywords: action.keywords.iter().map(|k| Prepared::new(k)).collect(),
            symbol: action.symbol.to_owned(),
            path: None,
            needs_confirmation: action.needs_confirmation,
            kind: ResultKind::SystemAction,
        }
    }

    fn settings_pane(pane: &settings_panes::SettingsPane) -> Self {
        Self {
            id: format!("settings:{}", pane.id),
            title: pane.title.to_owned(),
            subtitle: pane.parent.unwrap_or("System Settings"),
            title_match: Prepared::new(pane.title),
            keywords: pane.keywords.iter().map(|k| Prepared::new(k)).collect(),
            symbol: "gearshape".to_owned(),
            path: Some(pane.icon_path()),
            needs_confirmation: false,
            kind: ResultKind::SettingsPane,
        }
    }

    fn window(&(_, id, title, keywords, symbol): &windows::ActionEntry) -> Self {
        Self {
            id: format!("window:{id}"),
            title: title.to_owned(),
            subtitle: "Window",
            title_match: Prepared::new(title),
            keywords: keywords.iter().map(|k| Prepared::new(k)).collect(),
            symbol: symbol.to_owned(),
            path: None,
            needs_confirmation: false,
            kind: ResultKind::WindowAction,
        }
    }

    fn mode(&(_, id, title, keywords, symbol): &ModeEntry) -> Self {
        Self {
            id: format!("mode:{id}"),
            title: title.to_owned(),
            subtitle: "Jaycast",
            title_match: Prepared::new(title),
            keywords: keywords.iter().map(|k| Prepared::new(k)).collect(),
            symbol: symbol.to_owned(),
            path: None,
            needs_confirmation: false,
            kind: ResultKind::SystemAction,
        }
    }

    fn pick_color() -> Self {
        Self {
            id: PICK_COLOR_ID.to_owned(),
            title: "Pick Colour".to_owned(),
            subtitle: "Jaycast",
            title_match: Prepared::new("Pick Colour"),
            keywords: ["pick color", "colour picker", "color picker", "eyedropper"]
                .iter()
                .map(|k| Prepared::new(k))
                .collect(),
            symbol: "eyedropper".to_owned(),
            path: None,
            needs_confirmation: false,
            kind: ResultKind::SystemAction,
        }
    }

    fn keep_awake(&(_, id, title, symbol): &(KeepAwakeCommand, &str, &str, &str)) -> Self {
        Self {
            id: format!("keep-awake:{id}"),
            title: title.to_owned(),
            subtitle: "Jaycast",
            title_match: Prepared::new(title),
            keywords: KEEP_AWAKE_KEYWORDS
                .iter()
                .map(|k| Prepared::new(k))
                .collect(),
            symbol: symbol.to_owned(),
            path: None,
            needs_confirmation: false,
            kind: ResultKind::SystemAction,
        }
    }

    fn result(&self) -> SearchResult {
        SearchResult {
            id: self.id.clone(),
            title: self.title.clone(),
            subtitle: self.subtitle.into(),
            kind: self.kind,
            path: self.path.clone(),
            symbol: Some(self.symbol.clone()),
            needs_confirmation: self.needs_confirmation,
            completion: None,
        }
    }
}

impl Launcher {
    /// Starts indexing on a background thread; `data_dir` holds the usage history (and the
    /// currency rate cache).
    pub fn new(
        data_dir: &Path,
        config: LauncherConfig,
        windows: Arc<WindowManager>,
        keep_awake: Arc<KeepAwake>,
    ) -> Arc<Self> {
        let store = UsageStore::new(data_dir);
        let launcher = Arc::new(Self {
            index: RwLock::new(Arc::new(Index::default())),
            system: MODES
                .iter()
                .map(SystemEntry::mode)
                .chain([SystemEntry::pick_color()])
                .chain(KEEP_AWAKE_COMMANDS.iter().map(SystemEntry::keep_awake))
                .chain(system::actions().iter().map(SystemEntry::new))
                .chain(
                    settings_panes::panes()
                        .iter()
                        .map(SystemEntry::settings_pane),
                )
                .chain(windows::ACTIONS.iter().map(SystemEntry::window))
                .collect(),
            usage: Mutex::new(store.load()),
            store,
            observer: Mutex::new(None),
            watcher: Mutex::new(None),
            prefs: RwLock::new(Arc::new(Prefs::new(&config))),
            config: RwLock::new(Arc::new(config)),
            calculator: Calculator::new(data_dir),
            windows,
            keep_awake,
        });

        let weak = Arc::downgrade(&launcher);
        thread::Builder::new()
            .name("jaycast-app-index".into())
            .spawn(move || {
                let Some(launcher) = weak.upgrade() else {
                    return;
                };
                launcher.reindex(true);
                // The watcher holds only a weak reference, so dropping the launcher drops
                // the watcher, which ends its thread.
                let weak = Arc::downgrade(&launcher);
                let watcher = apps::Watcher::start(move || {
                    if let Some(launcher) = weak.upgrade() {
                        launcher.reindex(false);
                    }
                });
                *launcher.watcher.lock().unwrap() = watcher;
            })
            .expect("failed to spawn the app index thread");
        launcher
    }

    pub fn set_observer(&self, observer: Option<Arc<dyn LauncherObserver>>) {
        *self.observer.lock().unwrap() = observer;
    }

    /// Quicklinks, commands, snippets and the calculator switch come from here.
    pub fn set_config(&self, config: LauncherConfig) {
        *self.prefs.write().unwrap() = Arc::new(Prefs::new(&config));
        *self.config.write().unwrap() = Arc::new(config);
    }

    /// Best matches for `query`, apps and system actions together. An empty query returns
    /// the most used. Fast enough to call on every keystroke (no I/O).
    pub fn search(&self, query: &str, limit: usize) -> Vec<SearchResult> {
        let index = Arc::clone(&self.index.read().unwrap());
        let config = Arc::clone(&self.config.read().unwrap());
        let mut extra = Vec::new();
        if !query.trim().is_empty() {
            if config.calculator
                && let Some(calculation) = self.calculator.evaluate(query)
            {
                extra.push((CALCULATOR_SCORE, calculation.result()));
            }
            extra.extend(extensions::results(query, &config));
            extra.extend(crate::snippets::results(query, &config.snippets));
        }
        let prefs = Arc::clone(&self.prefs.read().unwrap());
        let usage = self.usage.lock().unwrap();
        rank(
            &fuzzy::fold_query(query),
            &index,
            &self.system,
            &prefs,
            extra,
            &usage,
            now(),
            limit,
        )
    }

    /// Opens a result from `search` and records the choice for ranking.
    pub fn open(&self, id: &str, query: &str) -> Result<OpenOutcome, CoreError> {
        let mut outcome = OpenOutcome::Done;
        if let Some(mode) = id.strip_prefix("mode:").and_then(mode_for_id) {
            outcome = OpenOutcome::ShowMode { mode };
        } else if id == PICK_COLOR_ID {
            outcome = OpenOutcome::PickColor;
        } else if let Some(command) = id
            .strip_prefix("keep-awake:")
            .and_then(keep_awake_command_for_id)
        {
            self.keep_awake
                .set_active(command.resolve(self.keep_awake.is_active()))?;
            outcome = OpenOutcome::KeepAwake {
                active: self.keep_awake.is_active(),
            };
        } else if let Some(text) = id.strip_prefix("calc:") {
            crate::pasteboard::copy(text)?;
            // Calculations aren't worth remembering for ranking.
            return Ok(OpenOutcome::Copied {
                text: text.to_owned(),
            });
        } else if id.starts_with("quicklink:") || id.starts_with("command:") {
            let config = Arc::clone(&self.config.read().unwrap());
            let observer = self.observer.lock().unwrap().clone();
            outcome = extensions::open(id, query, &config, observer)?;
        } else if let Some(bundle_id) = id.strip_prefix("app:") {
            let index = Arc::clone(&self.index.read().unwrap());
            let app = index
                .by_id
                .get(id)
                .map(|&i| &index.apps[i])
                .ok_or_else(|| CoreError::Other {
                    message: format!("no app with bundle id {bundle_id}"),
                })?;
            launch(&app.info.path)?;
        } else if let Some(action) = id.strip_prefix("system:") {
            system::perform(action)?;
        } else if let Some(pane) = id.strip_prefix("settings:") {
            settings_panes::open(pane)?;
        } else if let Some(action) = id.strip_prefix("window:") {
            let action = windows::action_for_id(action).ok_or_else(|| CoreError::Other {
                message: format!("unknown window action {action}"),
            })?;
            self.windows.perform(action)?;
        } else if let Some(snippet_id) = id.strip_prefix("snippet:") {
            let config = Arc::clone(&self.config.read().unwrap());
            let snippet = config
                .snippets
                .snippets
                .iter()
                .find(|s| s.id == snippet_id)
                .ok_or_else(|| CoreError::Other {
                    message: format!("no snippet with id {snippet_id}"),
                })?;
            crate::snippets::insert(snippet)?;
        } else {
            return Err(CoreError::Other {
                message: format!("unknown result {id}"),
            });
        }
        self.record(id, query);
        Ok(outcome)
    }

    /// Forgets how often and for which queries `id` was opened.
    pub fn reset_ranking(&self, id: &str) -> Result<(), CoreError> {
        let now = now();
        let snapshot = {
            let mut usage = self.usage.lock().unwrap();
            usage.forget(id);
            usage.clone()
        };
        self.store.save(&snapshot, now)
    }

    /// Every indexed app, sorted by name (for choosing app hotkeys in settings).
    pub fn apps(&self) -> Vec<AppInfo> {
        let index = Arc::clone(&self.index.read().unwrap());
        index.apps.iter().map(|app| app.info.clone()).collect()
    }

    /// Every app and built-in command, for the launcher settings: apps by name, then the
    /// commands in palette order.
    pub fn items(&self) -> Vec<LauncherItem> {
        let index = Arc::clone(&self.index.read().unwrap());
        let apps = index.apps.iter().map(|app| LauncherItem {
            id: app_result_id(&app.info.bundle_id),
            title: app.info.name.clone(),
            category: "Application".into(),
            path: Some(app.info.path.clone()),
            symbol: None,
            window_action: None,
        });
        let commands = self.system.iter().map(|entry| LauncherItem {
            id: entry.id.clone(),
            title: entry.title.clone(),
            category: match entry.kind {
                ResultKind::SettingsPane => "System Settings".into(),
                _ => entry.subtitle.into(),
            },
            path: entry.path.clone(),
            symbol: entry.path.is_none().then(|| entry.symbol.clone()),
            window_action: entry
                .id
                .strip_prefix("window:")
                .and_then(windows::action_for_id),
        });
        apps.chain(commands).collect()
    }

    /// Saves synchronously: the file is a few KB and opens happen once per palette use, so
    /// a background writer isn't worth it.
    fn record(&self, id: &str, query: &str) {
        let now = now();
        let snapshot = {
            let mut usage = self.usage.lock().unwrap();
            usage.record(id, &fuzzy::fold_query(query), now);
            usage.clone()
        };
        // Losing a ranking hint isn't worth failing the open over.
        let _ = self.store.save(&snapshot, now);
    }

    /// Rebuilds the index; tells the observer after the first build or when anything changed.
    fn reindex(&self, first: bool) {
        let fresh = Index::new(apps::index());
        let changed = {
            let mut index = self.index.write().unwrap();
            let changed = !index.same_apps(&fresh);
            *index = Arc::new(fresh);
            changed
        };
        if first || changed {
            let observer = self.observer.lock().unwrap().clone();
            if let Some(observer) = observer {
                observer.apps_changed();
            }
        }
    }
}

/// Scores every app and system action against `query` (folded), merges in `extra`
/// (calculator, quicklinks, commands, snippets; already scored) and returns the best.
fn rank(
    query: &[char],
    index: &Index,
    system: &[SystemEntry],
    prefs: &Prefs,
    extra: Vec<(i32, SearchResult)>,
    usage: &Usage,
    now: f64,
    limit: usize,
) -> Vec<SearchResult> {
    if query.is_empty() {
        return most_used(index, system, prefs, usage, now, limit);
    }

    struct Scored {
        score: i32,
        title_len: usize,
        result: SearchResult,
    }
    let mut scored = Vec::new();

    for app in &index.apps {
        let id = app_result_id(&app.info.bundle_id);
        if !prefs.is_enabled(&id) {
            continue;
        }
        let score = best_score(query, &app.title, app.alias.iter());
        let Some(score) = score.max(prefs.alias_score(query, &id)) else {
            continue;
        };
        let result = app_result(app);
        scored.push(Scored {
            score: score + usage.boost(&result.id, query, now),
            title_len: app.info.name.chars().count(),
            result,
        });
    }
    for entry in system {
        if !prefs.is_enabled(&entry.id) {
            continue;
        }
        let score = best_score(query, &entry.title_match, entry.keywords.iter());
        let Some(score) = score.max(prefs.alias_score(query, &entry.id)) else {
            continue;
        };
        scored.push(Scored {
            score: score + usage.boost(&entry.id, query, now),
            title_len: entry.title.chars().count(),
            result: entry.result(),
        });
    }

    for (score, result) in extra {
        scored.push(Scored {
            score: score + usage.boost(&result.id, query, now),
            title_len: result.title.chars().count(),
            result,
        });
    }

    // Best score first; among equals, the shorter title is the closer match.
    scored.sort_by(|a, b| {
        b.score
            .cmp(&a.score)
            .then(a.title_len.cmp(&b.title_len))
            .then_with(|| a.result.title.cmp(&b.result.title))
    });
    scored.truncate(limit);
    scored.into_iter().map(|s| s.result).collect()
}

/// The title's score, or a discounted alias/keyword score if that's better.
pub(crate) fn best_score<'a>(
    query: &[char],
    title: &Prepared,
    secondary: impl Iterator<Item = &'a Prepared>,
) -> Option<i32> {
    let title = fuzzy::score(query, title).map(|m| m.score);
    let secondary = secondary
        .filter_map(|p| fuzzy::score(query, p))
        .map(|m| m.score * SECONDARY_MATCH_WEIGHT / SECONDARY_MATCH_DIVISOR)
        .max();
    title.max(secondary)
}

/// Most frecent results first, then the remaining apps alphabetically.
fn most_used(
    index: &Index,
    system: &[SystemEntry],
    prefs: &Prefs,
    usage: &Usage,
    now: f64,
    limit: usize,
) -> Vec<SearchResult> {
    let mut results: Vec<SearchResult> = usage
        .most_frecent(now)
        .into_iter()
        .filter(|(id, _)| prefs.is_enabled(id))
        .filter_map(|(id, _)| match index.by_id.get(&id) {
            Some(&i) => Some(app_result(&index.apps[i])),
            None => system.iter().find(|e| e.id == id).map(SystemEntry::result),
        })
        .take(limit)
        .collect();
    for app in &index.apps {
        if results.len() >= limit {
            break;
        }
        let id = app_result_id(&app.info.bundle_id);
        if prefs.is_enabled(&id) && !results.iter().any(|r| r.id == id) {
            results.push(app_result(app));
        }
    }
    results
}

fn app_result(app: &App) -> SearchResult {
    SearchResult {
        id: app_result_id(&app.info.bundle_id),
        title: app.info.name.clone(),
        subtitle: app.subtitle.clone(),
        kind: ResultKind::Application,
        path: Some(app.info.path.clone()),
        symbol: None,
        needs_confirmation: false,
        completion: None,
    }
}

fn app_result_id(bundle_id: &str) -> String {
    format!("app:{bundle_id}")
}

/// Launches the app, or brings it to the front if it's already running. Returns once the
/// request is made; the launch itself completes asynchronously.
fn launch(path: &str) -> Result<(), CoreError> {
    if !Path::new(path).exists() {
        return Err(CoreError::Other {
            message: format!("{path} no longer exists"),
        });
    }
    autoreleasepool(|_| {
        let url = NSURL::fileURLWithPath(&NSString::from_str(path));
        let configuration = NSWorkspaceOpenConfiguration::configuration();
        configuration.setActivates(true);
        NSWorkspace::sharedWorkspace().openApplicationAtURL_configuration_completionHandler(
            &url,
            &configuration,
            None,
        );
    });
    Ok(())
}

fn now() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0.0, |d| d.as_secs_f64())
}

/// Applies a new launcher config to the hotkeys.
pub(crate) fn apply_config(hotkeys: &HotkeyManager, config: &LauncherConfig) {
    hotkeys.apply(config);
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: f64 = 1_000_000_000.0;

    fn app(name: &str, bundle_id: &str) -> App {
        App {
            info: AppInfo {
                bundle_id: bundle_id.into(),
                name: name.into(),
                path: format!("/Applications/{name}.app"),
            },
            subtitle: "Application".into(),
            title: Prepared::new(name),
            alias: None,
        }
    }

    fn index(names: &[&str]) -> Index {
        let mut apps: Vec<App> = names
            .iter()
            .map(|n| app(n, &format!("test.{}", n.replace(' ', ""))))
            .collect();
        apps.sort_by_cached_key(|a| a.info.name.to_lowercase());
        Index::new(apps)
    }

    fn lock_screen() -> SystemEntry {
        SystemEntry::new(&system::SystemAction {
            id: "lock-screen",
            title: "Lock Screen",
            keywords: &["secure", "away"],
            symbol: "lock.fill",
            needs_confirmation: false,
        })
    }

    fn titles(results: &[SearchResult]) -> Vec<&str> {
        results.iter().map(|r| r.title.as_str()).collect()
    }

    fn search(index: &Index, usage: &Usage, query: &str) -> Vec<SearchResult> {
        rank(
            &fuzzy::fold_query(query),
            index,
            &[lock_screen()],
            &Prefs::default(),
            Vec::new(),
            usage,
            NOW,
            8,
        )
    }

    const NAMES: &[&str] = &[
        "Safari",
        "Software Update",
        "System Settings",
        "Terminal",
        "TextEdit",
        "Time Machine",
        "Notes",
        "Visual Studio Code",
    ];

    #[test]
    fn ranks_by_match_quality_without_history() {
        let index = index(NAMES);
        let usage = Usage::default();
        assert_eq!(titles(&search(&index, &usage, "saf"))[0], "Safari");
        assert_eq!(titles(&search(&index, &usage, "term"))[0], "Terminal");
        assert_eq!(
            titles(&search(&index, &usage, "vsc")),
            vec!["Visual Studio Code"]
        );
        assert!(search(&index, &usage, "zzz").is_empty());
    }

    #[test]
    fn strong_match_beats_weak_frequent_one() {
        let index = index(NAMES);
        let mut usage = Usage::default();
        // Terminal matches "tm" weakly (t…m) but is used constantly; Time Machine matches
        // on both word starts and has never been opened.
        for _ in 0..50 {
            usage.record("app:test.Terminal", &[], NOW);
        }
        assert_eq!(titles(&search(&index, &usage, "tm"))[0], "Time Machine");
    }

    #[test]
    fn learns_which_result_a_query_leads_to() {
        let index = index(NAMES);
        let mut usage = Usage::default();
        assert_ne!(titles(&search(&index, &usage, "s"))[0], "System Settings");
        usage.record("app:test.SystemSettings", &fuzzy::fold_query("sys"), NOW);
        assert_eq!(titles(&search(&index, &usage, "s"))[0], "System Settings");
    }

    #[test]
    fn system_actions_match_title_and_keywords() {
        let index = index(NAMES);
        let usage = Usage::default();
        let results = search(&index, &usage, "lock");
        assert_eq!(results[0].id, "system:lock-screen");
        assert_eq!(results[0].kind, ResultKind::SystemAction);
        assert_eq!(results[0].symbol.as_deref(), Some("lock.fill"));
        assert_eq!(
            titles(&search(&index, &usage, "secure")),
            vec!["Lock Screen"]
        );
    }

    #[test]
    fn colour_and_emoji_entries_are_typable() {
        let entries: Vec<SystemEntry> = MODES
            .iter()
            .map(SystemEntry::mode)
            .chain([SystemEntry::pick_color()])
            .collect();
        let find = |query: &str| {
            rank(
                &fuzzy::fold_query(query),
                &Index::default(),
                &entries,
                &Prefs::default(),
                Vec::new(),
                &Usage::default(),
                NOW,
                8,
            )
        };
        for query in ["pick colour", "pick color", "eyedropper"] {
            assert_eq!(find(query)[0].id, PICK_COLOR_ID, "{query}");
        }
        for query in ["list colours", "list colors", "colour history"] {
            assert_eq!(find(query)[0].id, "mode:colors", "{query}");
        }
        for query in ["emoji", "emoji picker", "smiley"] {
            assert_eq!(find(query)[0].id, "mode:emoji", "{query}");
        }
        assert_eq!(mode_for_id("colors"), Some(PaletteMode::Colors));
    }

    #[test]
    fn settings_panes_open_straight_to_the_pane() {
        let entries: Vec<SystemEntry> = system::actions()
            .iter()
            .map(SystemEntry::new)
            .chain(
                settings_panes::panes()
                    .iter()
                    .map(SystemEntry::settings_pane),
            )
            .collect();
        let find = |query: &str| {
            rank(
                &fuzzy::fold_query(query),
                &Index::default(),
                &entries,
                &Prefs::default(),
                Vec::new(),
                &Usage::default(),
                NOW,
                8,
            )
        };
        let privacy = &find("privacy")[0];
        assert_eq!(privacy.id, "settings:privacy-security");
        assert_eq!(privacy.kind, ResultKind::SettingsPane);
        assert_eq!(privacy.subtitle, "System Settings");
        assert!(privacy.path.as_deref().unwrap().ends_with(".appex"));
        for (query, id) in [
            ("sound", "settings:sound"),
            ("full disk", "settings:privacy-full-disk-access"),
            ("dock", "settings:desktop-dock"),
            ("airpods", "settings:bluetooth"),
        ] {
            assert_eq!(find(query)[0].id, id, "{query}");
        }
        let fda = &find("full disk access")[0];
        assert_eq!(fda.subtitle, "Privacy & Security");
    }

    #[test]
    fn keep_awake_is_typable_by_its_coffee_names() {
        let entries: Vec<SystemEntry> = KEEP_AWAKE_COMMANDS
            .iter()
            .map(SystemEntry::keep_awake)
            .collect();
        let find = |query: &str| {
            rank(
                &fuzzy::fold_query(query),
                &Index::default(),
                &entries,
                &Prefs::default(),
                Vec::new(),
                &Usage::default(),
                NOW,
                8,
            )
        };
        for query in ["keep awake", "caffeinate", "coffee", "amphetamine"] {
            assert!(
                find(query).iter().any(|r| r.id == "keep-awake:toggle"),
                "{query}"
            );
        }
        assert_eq!(find("turn on keep awake")[0].id, "keep-awake:on");
        assert_eq!(
            keep_awake_command_for_id("off"),
            Some(KeepAwakeCommand::Off)
        );
    }

    fn prefs(items: Vec<crate::config::ItemSettings>) -> Prefs {
        Prefs::new(&LauncherConfig {
            items,
            ..LauncherConfig::default()
        })
    }

    #[test]
    fn user_aliases_find_items_and_win_when_typed_exactly() {
        let index = index(NAMES);
        let prefs = prefs(vec![crate::config::ItemSettings {
            id: "app:test.VisualStudioCode".into(),
            aliases: vec![" ide ".into(), "".into(), "te".into()],
            ..Default::default()
        }]);
        let find = |query: &str| {
            rank(
                &fuzzy::fold_query(query),
                &index,
                &[lock_screen()],
                &prefs,
                Vec::new(),
                &Usage::default(),
                NOW,
                8,
            )
        };
        assert_eq!(titles(&find("ide")), vec!["Visual Studio Code"]);
        // "te" fuzzily matches Terminal and TextEdit by title, but is this item's alias.
        assert_eq!(titles(&find("te"))[0], "Visual Studio Code");
    }

    #[test]
    fn disabled_items_never_show() {
        let index = index(NAMES);
        let disable = |id: &str| crate::config::ItemSettings {
            id: id.into(),
            enabled: false,
            ..Default::default()
        };
        let prefs = prefs(vec![
            disable("app:test.Safari"),
            disable("system:lock-screen"),
        ]);
        let mut usage = Usage::default();
        usage.record("system:lock-screen", &[], NOW);
        let find = |query: &str| {
            rank(
                &fuzzy::fold_query(query),
                &index,
                &[lock_screen()],
                &prefs,
                Vec::new(),
                &usage,
                NOW,
                20,
            )
        };
        assert!(!titles(&find("saf")).contains(&"Safari"));
        assert!(find("lock").is_empty());
        let everything = find("");
        let all = titles(&everything);
        assert!(!all.contains(&"Safari") && !all.contains(&"Lock Screen"));
        assert_eq!(all.len(), NAMES.len() - 1);
    }

    #[test]
    fn aliases_match_at_a_discount() {
        let mut code = app("Visual Studio Code", "com.microsoft.VSCode");
        code.alias = Some(Prepared::new("Code"));
        let index = Index::new(vec![code, app("Codex", "test.codex")]);
        let results = rank(
            &fuzzy::fold_query("code"),
            &index,
            &[],
            &Prefs::default(),
            Vec::new(),
            &Usage::default(),
            NOW,
            8,
        );
        assert_eq!(titles(&results), vec!["Codex", "Visual Studio Code"]);
    }

    #[test]
    fn empty_query_lists_most_used_then_alphabetical() {
        let index = index(NAMES);
        let mut usage = Usage::default();
        usage.record("app:test.Terminal", &[], NOW);
        usage.record("app:test.Terminal", &[], NOW);
        usage.record("system:lock-screen", &[], NOW);
        let results = search(&index, &usage, "");
        assert_eq!(titles(&results)[..3], ["Terminal", "Lock Screen", "Notes"]);
        assert_eq!(results.len(), 8);
        assert_eq!(
            titles(&search(&index, &Usage::default(), ""))[..2],
            ["Notes", "Safari"]
        );
    }

    #[test]
    fn respects_the_limit() {
        let index = index(NAMES);
        let results = rank(
            &fuzzy::fold_query("e"),
            &index,
            &[],
            &Prefs::default(),
            Vec::new(),
            &Usage::default(),
            NOW,
            3,
        );
        assert_eq!(results.len(), 3);
    }

    /// Search must stay well under a frame on every keystroke.
    #[test]
    fn search_is_fast_for_hundreds_of_apps() {
        let words = [
            "Pro", "Studio", "Code", "Music", "Photo", "Editor", "Manager", "Monitor", "Book",
            "Mail", "Note", "Time", "Settings", "Update", "Terminal", "Viewer",
        ];
        let names: Vec<String> = (0..400)
            .map(|i| format!("{} {} {i}", words[i % 16], words[(i * 7 + 3) % 16]))
            .collect();
        let refs: Vec<&str> = names.iter().map(String::as_str).collect();
        let index = index(&refs);
        let usage = Usage::default();
        let queries = [
            "s", "st", "stu", "stud", "studi", "studio", "msc", "time set",
        ];
        let start = std::time::Instant::now();
        for query in queries {
            let _ = search(&index, &usage, query);
        }
        let per_query = start.elapsed() / queries.len() as u32;
        println!("400 apps: {per_query:?} per query");
        // A generous bound: debug builds run several times slower than release.
        assert!(
            per_query < std::time::Duration::from_millis(20),
            "{per_query:?}"
        );
    }

    /// Searches this machine's apps. Run with `cargo test -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn search_this_machine() {
        let index = Index::new(apps::index());
        let usage = Usage::default();
        for query in ["saf", "term", "sysset", "vsc", "fin", "act mon", "s"] {
            let start = std::time::Instant::now();
            let results = rank(
                &fuzzy::fold_query(query),
                &index,
                &[],
                &Prefs::default(),
                Vec::new(),
                &usage,
                NOW,
                3,
            );
            println!("{query:>8} ({:?}): {:?}", start.elapsed(), titles(&results));
        }
    }
}
