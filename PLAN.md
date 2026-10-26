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
| 1 | Mouse: event tap, button remapping, scroll modifiers, smooth scrolling, settings UI | 🚧 In progress |
| 2 | Monitor: DDC brightness for external displays, menu bar sliders, brightness keys + OSD | ⏳ |
| 3 | Launcher core: hotkey palette, app search with fuzzy ranking, system actions, per-app hotkeys | ⏳ |
| 4 | Calculator & conversions, quicklinks & shell commands | ⏳ |
| 5 | Clipboard history, file search | ⏳ |
| 6 | Window management, snippets / text expansion | ⏳ |

### Step 1: Mouse

- [x] Shared core API: `Core` object, persisted JSON config, `MouseConfig` types, error type (`a599e9c`)
- [ ] Event tap on its own thread; re-enables itself if macOS disables it; ignores our own synthetic events
- [ ] Button remapping: middle/back/forward buttons; click, double-click and hold triggers;
      actions for back/forward, Mission Control, App Exposé, Show Desktop, Launchpad, Spaces,
      custom keyboard shortcut
- [ ] Scroll modifiers: reverse wheel direction (independent of the trackpad), modifier for
      horizontal, zoom and precise scrolling
- [x] Smooth scrolling: pure, unit-tested physics (acceleration, momentum) + emitter that posts
      continuous scroll events with proper phases at display rate; only for wheel mice (`61fb54d`)
- [ ] Shell: Settings window with a Mouse pane; input starts once Accessibility is granted;
      menu toggle to turn mouse fixes on/off

### Step 2: Monitor

- [ ] Enumerate external displays; read/write brightness over DDC/CI (`ddc-macos`)
- [ ] Menu bar sliders per display
- [ ] Brightness keys control the display under the cursor (pass through on the built-in display),
      with the native on-screen indicator

### Step 3: Launcher core

- [ ] Global hotkey + floating palette panel
- [ ] App index (with change watching) and fuzzy ranking that learns from usage
- [ ] System actions (lock, sleep, dark mode, empty trash, …)
- [ ] Per-app hotkeys to focus or hide an app

### Step 4: Calculator, quicklinks, shell commands

- [ ] Inline maths, unit and currency conversion
- [ ] Quicklinks with `{query}` placeholders
- [ ] Custom shell commands run from the palette

### Step 5: Clipboard history, file search

- [ ] Clipboard history for text and images (SQLite), searchable, paste from the palette
- [ ] File search through Spotlight, scoped to chosen folders

### Step 6: Window management, snippets

- [ ] Snap halves/thirds/quarters, maximise, move between displays
- [ ] Snippets with placeholders and keyword expansion while typing

## Not in v1

Click-and-drag mouse gestures, built-in / Apple display brightness, DDC volume, software dimming,
Raycast extension support.

## Log

- **Step 0** (`27275b5`): Cargo workspace + UniFFI bindings generated at build time, SwiftPM menu
  bar app, Accessibility onboarding, justfile. Signed with the Apple Development identity so the
  Accessibility grant survives rebuilds.
