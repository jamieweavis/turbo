# Jaycast plan

One macOS app combining a keyboard launcher (in the spirit of Tinycast/Raycast), mouse fixes (in
the spirit of Mac Mouse Fix) and external monitor brightness (in the spirit of MonitorControl).

## Principles

- **Rust core, thin Swift shell.** Anything that decides or does something lives in `core/`. The
  shell draws UI and forwards intent over UniFFI.
- **Clean room.** Upstream projects (Tinycast AGPL-3.0, Mac Mouse Fix MMF License, MonitorControl
  MIT) are references for *what* is possible only. No code is copied or ported. The license is
  chosen at publish time.
- **Terminal-driven.** `just` + cargo + SwiftPM; Xcode is installed but never opened.
- **One step at a time.** Each step ends with a commit and a pause for review.

## Steps

| # | Step | Status |
|---|---|---|
| 0 | Skeleton: Rust core ↔ Swift shell, `just`, menu bar app, Accessibility onboarding | ✅ Done (`27275b5`) |
| 1 | Mouse: event tap, button remapping, scroll modifiers, smooth scrolling, settings UI | ✅ Done |
| 2 | Monitor: DDC brightness for external displays, menu bar sliders, brightness keys + OSD | ✅ Done |
| 3 | Launcher core: hotkey palette, app search with fuzzy ranking, system actions, per-app hotkeys | ✅ Done |
| 4 | Calculator & conversions, quicklinks & shell commands | ✅ Done |
| 5 | Clipboard history, file search | ✅ Done |
| 6 | Window management, snippets / text expansion | ✅ Done |
| 7 | Window move/resize by modifier + drag (after easy-move-resize) | ✅ Done |

### Step 1: Mouse

- [x] Shared core API: `Core` object, persisted JSON config, `MouseConfig` types, error type (`a599e9c`)
- [x] Event tap on its own thread; re-enables itself if macOS disables it; ignores our own synthetic events
- [x] Button remapping: middle/back/forward buttons; click, double-click and hold triggers;
      actions for back/forward, Mission Control, App Exposé, Show Desktop, Launchpad, Spaces,
      custom keyboard shortcut
- [x] Scroll modifiers: reverse wheel direction (independent of the trackpad), modifier for
      horizontal, zoom and precise scrolling
- [x] Smooth scrolling: pure, unit-tested physics (acceleration, momentum) + emitter that posts
      continuous scroll events with proper phases at display rate; only for wheel mice (`61fb54d`)
- [x] Shell: Settings window with a Mouse pane; input starts once Accessibility is granted;
      menu toggle to turn mouse fixes on/off

### Step 2: Monitor

- [x] Shared core API: `MonitorConfig`, `DisplayInfo`, `DisplayObserver` callback, `Core`
      display methods; brightness keys start/stop with the other event taps
- [x] Enumerate external displays; read/write brightness over DDC/CI (`ddc-macos`), coalesced
      background writes, hotplug
- [x] Brightness keys control the display under the cursor (pass through on the built-in display
      or displays without DDC)
- [x] Shell: menu bar popover with per-display sliders, on-screen indicator for key changes,
      Monitor settings pane
- [x] Minimum brightness is black: the bottom 25% of the range holds DDC at 0 and dims gamma
      to black

### Step 3: Launcher core

- [x] Shared core API: `LauncherConfig` (palette hotkey, default ⌥Space; app hotkeys),
      `SearchResult`, `LauncherObserver`, `Core` search/open/hotkey methods
- [x] App index (with change watching) and fuzzy ranking that learns from usage
- [x] System actions (lock, sleep, dark mode, empty trash, …)
- [x] System Settings panes and privacy sections (opened directly, with their own icons)
- [x] Global hotkeys: palette + per-app focus-or-hide
- [x] Shell: floating palette panel with keyboard navigation, confirmations, Launcher settings pane
- [x] ⌘K actions menu (added on request): a searchable list of everything that can be done
      with the selection in every palette mode, with its shortcuts; ⌘↩ runs a result's second
      action (Show in Finder for apps); apps can also be hidden, quit or force quit, and any
      result's ranking reset

### Step 4: Calculator, quicklinks, shell commands

- [x] Shared core API: `LauncherConfig` calculator switch, quicklinks (defaults: Google `g`,
      YouTube `yt`, GitHub `gh`) and commands; `SearchResult.completion`; `OpenOutcome`;
      `command_finished` callback; search merges calculator/extension results; clipboard helper
- [x] Calculator: inline maths, unit and currency conversion (cached ECB rates)
- [x] Quicklinks with `{query}` / `{clipboard}` placeholders
- [x] Shell commands run from the palette (discard / show / copy output, optional confirmation)
- [x] Shell: calculator row, argument completion, copied/output feedback, Quicklinks and
      Commands settings

### Step 5: Clipboard history, file search

- [x] Shared core API: palette modes (search / clipboard / files) with entries in search and
      their own hotkeys (clipboard ⌥⌘V by default), `ClipboardConfig`, `FileSearchConfig`,
      `ClipboardItem`, `FileResult`, `Core` clipboard and file methods
- [x] Clipboard history: monitor, SQLite store, text/links/images/files, ignore concealed copies
      and password managers, retention, pinning, paste into the frontmost app
- [x] File search through Spotlight, scoped to chosen folders
- [x] Shell: clipboard and file modes in the palette (preview, actions, Quick Look), Clipboard
      and File Search settings

### Step 6: Window management, snippets

- [x] Shared core API: `WindowConfig` (hotkeys, default ⌃⌥ arrows etc.; gap), `WindowAction`,
      `SnippetsConfig`/`Snippet`, window and snippet results in search, window hotkeys,
      snippet tap started with the other event taps
- [x] Window management through Accessibility: halves (repeat cycles 1/2 → 2/3 → 1/3),
      quarters, thirds, sixths (2 rows × 3 columns), maximise, almost maximise, centre, restore,
      next/previous display
- [x] Snippets with placeholders (`{clipboard}`, `{date}`, `{time}`, `{cursor}`, …) from the
      palette, and keyword expansion while typing
- [x] Shell: window and snippet results in the palette, Windows and Snippets settings
- [x] Hyper key (added on request): Caps Lock or a left/right modifier acts as ⌃⌥⇧⌘ (Shift
      optional) while held; a quick press can send Escape or toggle Caps Lock; Keyboard pane

## Not in v1

Click-and-drag mouse gestures, built-in / Apple display brightness, DDC volume, software dimming
for displays without DDC,
Raycast extension support.

## Log

- **Window move/resize**: ⌃⌘ (configurable, exact match, Caps Lock/Fn ignored) + left drag
  moves the window under the cursor, + right drag resizes it; "Swap buttons" reverses them.
  Own `[window_drag]` config and settings pane, own session-level tap that only runs while on;
  it swallows the press, drags and release (even over the desktop, where nothing moves) and
  hands positions to a worker thread that coalesces them to the newest, so a slow app never
  stalls the cursor. Grabbing raises and focuses the window. Resize always keeps the top-left
  corner and moves the bottom-right, wherever grabbed; min 100×60. Full-screen and unmovable windows are skipped; `AXEnhancedUserInterface` is off
  for the drag; Restore puts a dragged window back.
- **Actions menu**: shell-side `PaletteAction` lists built per mode from the existing model
  methods, so the menu and the ⌘ shortcuts stay in step; the menu floats bottom-right over the
  palette with its own field (filter by words in the title), and the palette grows to fit it
  unfiltered. Destructive actions that can't be undone (Force Quit, clearing clipboard or
  colour history) ask for a second ↩. Core: `is_app_running`, `hide_app`, `quit_app` (via
  `NSRunningApplication`) and `reset_result_ranking`. Not yet tried in the running app.
- **Hyper key**: the chosen key is remapped to F18 through the HID event system's
  `UserKeyMapping` (what `hidutil` sets; other mappings are kept), and an active HID-level tap
  swallows F18 and adds ⌃⌥⇧⌘ to key and mouse button events while it's held, so global
  hotkeys and other apps' taps see plain modifiers. The mapping only exists while the tap runs:
  removed on stop/quit, when the setting is off, and at launch in case of a crash. A quick press
  is one with nothing else pressed, released within 1 s. Off by default. Known limits: apps
  that read the live modifier state instead of the event's flags won't see Hyper, and a key
  already bound to F18 elsewhere would fire. Not yet tried on the real keyboard.
- **Step 6**: built by three parallel agents (window management; snippets + expansion; Swift
  panes), integrated on main. 281 Rust tests. Windows: pure layout module (gap shared between
  neighbours, whole points), AX focused window with a 0.5 s messaging timeout, size → position →
  size, `AXEnhancedUserInterface` switched off during the move; per-window memory keyed by
  pid + CGWindowID (`_AXUIElementGetWindow`) for Restore and half cycling; next/previous
  display ordered left to right and keeps a snapped layout. The read-only probe saw both
  displays and the focused window; nothing has moved a real window yet. Snippets: search by
  name/keyword, placeholders with locale date formats, insertion pastes a transient pasteboard
  item with ⌘V and restores the previous clipboard after 600 ms; expansion is a listen-only key
  tap that only runs when a snippet has a keyword, never reads during secure input, and pauses
  while the palette is open (it's a non-activating panel, so the shell reports it). Known
  limits: dead keys/IME composition, a copy made <400 ms before an expansion can miss history,
  the restored clipboard keeps the transient marker. Settings: Windows pane (hotkeys, gap,
  Restore Defaults) and Snippets pane (editor with placeholder menu, keyword clash warnings).
- **Step 5**: built by three parallel agents (clipboard history; Spotlight file search; Swift
  palette modes + settings), integrated on main. 217 Rust tests. Clipboard: polls the change
  count every 400 ms, SQLite (WAL) + PNG files in `clipboard/` (0700/0600), dedupes by SHA-256,
  skips concealed/transient copies and ignored apps, prunes by count/age (pinned kept), pastes
  with ⌘V 100 ms after writing. File search: MDQuery on file names (`*word*`cd) in the chosen
  folders, ~0.25 s per query, ranked by match quality, recency and depth; unindexed folders
  return nothing. Palette: clipboard (list + preview) and files modes with their own hotkeys.
- **Step 4**: built by three parallel agents (calculator; quicklinks + commands; Swift UI),
  integrated on main. 183 Rust tests. Calculator: own lexer/parser, units, currency (ECB rates
  from api.frankfurter.dev via Foundation, cached in `currency-rates.json`, refreshed every 12 h;
  the first fetch from the app worked), about 2 µs per evaluation, answers only "interesting"
  queries so app searches aren't hijacked. Quicklinks/commands: `<keyword> <text>`, completion
  to `<keyword> `, `{query}`/`{clipboard}` URL-encoded or shell-quoted, commands under
  `zsh -lc` with a 60 s process-group timeout and discard/show/copy output.
- **Step 3**: built by three parallel agents (app index + fuzzy + ranking; Carbon hotkeys, app
  toggle + system actions; Swift palette + Launcher pane), integrated on main. 118 Rust tests.
  Indexes ~99 apps in ~60 ms; search takes 20–65 µs per query. Names are Finder's localized names,
  with folder/bundle names as aliases. Ranking: fuzzy score + capped frecency (one-week half-life)
  + per-query memory, in `launcher-usage.json`. Hotkeys pause while a shortcut recorder is
  listening, and the Launcher pane flags hotkeys another app owns. System actions: lock
  (private `SACLockScreenImmediate`), sleep/displays (`pmset`), restart/shut down/log out
  (loginwindow Apple Events, may prompt for Automation once), dark mode (private SkyLight),
  empty trash (Finder, prompts for Automation once), screen saver, mute. Not yet checked by hand:
  the palette's look, the hotkey, opening apps and actions, FSEvents re-indexing.
- **Step 2**: built by three parallel agents (DDC display manager; brightness-key tap; Swift
  popover, HUD and Monitor pane), integrated on main. 72 Rust tests; both event taps confirmed
  live. DDC verified once on the DELL U2723QE (read 100/100 in 48 ms), after which the monitor
  started answering every request with a DDC null message — probably from concurrent access
  (two test managers, and MonitorControl was running at the time). Workers keep re-reading with
  backoff, so it recovers on its own once the monitor answers again. Notes: Jaycast draws its own
  brightness HUD by default (the private `OSDManager` exists on macOS 27 but showed nothing;
  opt in with `defaults write com.jamieweavis.jaycast UseSystemOSD -bool YES`). Brightness
  changed with the monitor's own buttons isn't re-read until restart or hotplug. F14/F15 keys
  aren't handled. After hands-on approval, the bottom 25% of the range became gamma dimming so
  minimum brightness is fully black (macOS restores gamma when the app exits). The brightness
  indicator was then restyled after macOS 26's own (Liquid Glass card under the menu bar, top
  right, display name, draggable slider with step ticks); the private `OSDManager` path was removed.
- **Step 1**: built by three parallel agents (event tap + buttons + scroll routing; smooth-scroll
  physics + emitter; Swift settings UI), integrated on main. 50 Rust tests. The live tap is confirmed
  running (`CGGetEventTapList`: enabled). Not yet checked by hand: scroll feel, every action, the
  binding editor and shortcut recorder. Notes: Launchpad opens Apps.app (Launchpad is gone in macOS
  26); Dock actions use the private `CoreDockSendNotification`; smoothed events carry no modifier
  flags so AppKit doesn't re-apply Shift-to-horizontal. Tested by hand and approved; the momentum
  setting was removed afterwards (momentum is always on).
- **Step 0** (`27275b5`): Cargo workspace + UniFFI bindings generated at build time, SwiftPM menu
  bar app, Accessibility onboarding, justfile. Signed with the Apple Development identity so the
  Accessibility grant survives rebuilds.
