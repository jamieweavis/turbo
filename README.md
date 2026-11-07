<div align="center">

# Jaycast

### The all-in-one toolkit for macOS.

**A launcher, clipboard manager, snippet expander, window manager, mouse fixer and monitor controller, in one small native app.**
Written in Rust. Free and open source. No account, no telemetry, no subscription.

[![build](https://github.com/jamieweavis/jaycast/actions/workflows/build.yml/badge.svg)](https://github.com/jamieweavis/jaycast/actions/workflows/build.yml)
[![release](https://img.shields.io/github/v/release/jamieweavis/jaycast?label=release)](https://github.com/jamieweavis/jaycast/releases/latest)
[![Downloads](https://img.shields.io/github/downloads/jamieweavis/jaycast/total?color=f97316)](https://github.com/jamieweavis/jaycast/releases)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue)](LICENSE)
![Platforms](https://img.shields.io/badge/platform-macOS-lightgrey)
![Rust](https://img.shields.io/badge/rust-1.88%2B-orange)

[**Download for Mac**](https://github.com/jamieweavis/jaycast/releases/latest) · [Features](#features) · [Privacy](#privacy) · [Keyboard](#keyboard) · [Development](#development)

</div>

---

## Contents

- [Why Jaycast](#why-jaycast)
- [Features](#features)
  - [Launcher](#-launcher)
  - [Clipboard & text](#-clipboard--text)
  - [Windows](#-windows)
  - [Mouse & keyboard](#-mouse--keyboard)
  - [Displays & system](#-displays--system)
  - [Config file](#-config-file)
  - [Coming soon](#-coming-soon)
- [Privacy](#privacy)
- [Keyboard](#keyboard)
- [Yours to keep](#yours-to-keep)
- [Install](#install)
- [Development](#development)
- [Releasing](#releasing)

## Why Jaycast

Most Macs end up running a handful of menu bar apps to fill the gaps in macOS: one for launching, one for
the clipboard, one for windows, one to make a normal mouse scroll properly, one for external monitor
brightness. Each has its own updater, its own settings and its own slice of memory. Jaycast replaces them
with one app, one settings file and one process.

|  |  |
| --- | --- |
| **🧰 All in one** | A dozen tools that share one palette, one settings window and one hotkey system. Use the ones you want and switch the rest off. |
| **⚡ Fast** | The core is Rust, compiled to native code. The palette opens instantly and results update as you type. Nothing waits on the network. |
| **🪶 Small** | A ~4 MB download and a ~10 MB app. No Electron, no bundled browser, no runtime to install. |
| **🔋 Light** | Idles at close to zero CPU. Background work is event-driven, and features you switch off stop running entirely. |
| **🍎 Native** | A SwiftUI shell on top of the Rust core, so it looks and behaves like part of macOS. |
| **🔒 Local** | Your clipboard, snippets and settings never leave your Mac. |
| **🔓 Yours** | MIT licensed. Every feature is free, there is no Pro tier, and you can build it yourself. |

## Features

Every tool is reachable from the palette, from its own hotkey, or quietly in the background. Each one can
be switched off on its own.

### 🔍 Launcher

One palette for the things you do all day, opened with <kbd>⌥</kbd><kbd>Space</kbd>.

| | |
| --- | --- |
| **Apps** | Fuzzy-search apps, ranked by how often you use them, with per-app hotkeys and aliases. |
| **Calculator** | Type maths straight into the palette, with unit and currency conversion. |
| **Quicklinks** | Open websites and searches by keyword: `g rust traits`. |
| **Shell Commands** | Run shell commands from the palette. |
| **File Search** | Spotlight search, scoped to the folders you choose. |

### 📋 Clipboard & text

| | |
| --- | --- |
| **Clipboard History** | Search and paste everything you've copied: text, links, colours and images. Pin what you want to keep. |
| **Snippets** | Expand saved text by keyword, anywhere you type, with `{clipboard}`, `{date}` and `{cursor}` placeholders. |
| **Emoji** | Search and insert emoji from a grid. |
| **Colour Picker** | Pick a colour from anywhere on screen, copied and kept in a history. |

### 🪟 Windows

| | |
| --- | --- |
| **Window Management** | Snap to halves and quarters, maximise, centre, restore and move windows between displays with hotkeys. |
| **Window Transform** | Move and resize a window by dragging anywhere in it with <kbd>⌃</kbd><kbd>⌘</kbd> held. |

### 🖱️ Mouse & keyboard

| | |
| --- | --- |
| **Mouse** | Smooth scrolling, scroll modifiers and button remapping for ordinary mice. |
| **Hyper Key** | Turn Caps Lock or a modifier into <kbd>⌃</kbd><kbd>⌥</kbd><kbd>⇧</kbd><kbd>⌘</kbd> for shortcuts that never clash. |
| **Hotkeys** | Bind any app, system action or System Settings pane to its own global hotkey. |

### 🖥️ Displays & system

| | |
| --- | --- |
| **Brightness** | Control external monitors over DDC/CI with the brightness keys, dimming below their minimum to black. |
| **Media Keys** | Send play/pause, next and previous to Spotify, Music or VLC (whichever you used last, or always the one you pick), so they never launch Apple Music. Also in the palette as Play/Pause, Next Track and Previous Track. |
| **Keep Awake** | Stop your Mac (and optionally the display) sleeping while you need it. |
| **Camera Preview** | Check your webcam before a call, from the palette. Switch cameras (iPhone included) and mirroring from <kbd>⌘</kbd><kbd>K</kbd>. |
| **System Actions** | Lock Screen, Sleep, Empty Trash and more, plus every System Settings pane, from the palette. |

### 📄 Config file

Every setting, from hotkeys and snippets to quicklinks and mouse buttons, lives in one plain TOML file at
`~/.config/jaycast.toml`. Check it into your dotfiles and a new Mac is set up the moment Jaycast launches.
Symlink it from your dotfiles repo if you like: Jaycast writes through the link to the real file and leaves the link in place.

```toml
[launcher.hotkey]
key_code = 49            # Space
modifiers = ["Option"]

[clipboard]
max_items = 500
retention_days = 30

[[mouse.buttons]]
button = 3
trigger = "Click"
action = "Back"
```

- **Human-readable and diffable**, so changes show up cleanly in `git diff`.
- **The Settings window and the file stay in step:** anything you change in the app is written back to
  the file.
- **Missing keys fall back to their defaults**, and configs from older versions keep loading.
- **No database, no plist, no cloud sync** standing between you and your settings.

### 🚧 Coming soon

| | |
| --- | --- |
| **Icon Changer** | Swap macOS app icons, and reapply them automatically when apps update. *(Name TBC.)* |

## Privacy

**No account. No telemetry. No analytics. Nothing phones home.**

| | |
| --- | --- |
| Accounts | **0** |
| Telemetry | **0** |
| Cloud sync | **0** |

- **Clipboard history is stored on your Mac**, in a local SQLite database. It keeps 500 items for 30 days
  by default (pinned items stay), and both limits are yours to change. Turn it off and Jaycast stops
  watching the clipboard altogether.
- **Passwords are never recorded.** Copies from Keychain Access, Passwords and password managers, and
  anything an app marks as concealed or transient, are skipped.
- **The only network request** is for currency rates: the European Central Bank's daily reference rates
  from [frankfurter.dev](https://frankfurter.dev), fetched in the background and cached. No identifiers
  are sent.
- **The camera only runs while Camera Preview is showing**, and stops the moment you leave it. Nothing is
  recorded or saved.
- **Media keys only reach the music app they're for.** Jaycast sends it play/pause, next or previous
  and nothing else, after macOS asks you to allow it. Which app played last comes from the apps' own
  broadcasts, not from watching what you play.
- **Every feature can be switched off**, and a switched-off feature doesn't run.
- **Settings are a plain text file** at `~/.config/jaycast.toml`: readable, diffable, and easy to back up
  or keep in your dotfiles.

## Keyboard

Built for the keyboard. Every hotkey can be changed in Settings.

| Shortcut | Action |
| --- | --- |
| <kbd>⌥</kbd><kbd>Space</kbd> | Open the palette |
| <kbd>⌥</kbd><kbd>⌘</kbd><kbd>V</kbd> | Open clipboard history |
| <kbd>↩</kbd> | Run the main action |
| <kbd>⌘</kbd><kbd>↩</kbd> | Run the alternate action (copy instead of paste, reveal in Finder) |
| <kbd>⌘</kbd><kbd>K</kbd> | Open the actions menu |
| <kbd>⌘</kbd><kbd>P</kbd> | Pin or unpin a clipboard item |
| <kbd>⌘</kbd><kbd>Y</kbd> | Quick Look |
| <kbd>⌘</kbd><kbd>⌫</kbd> | Delete the selected item |
| <kbd>esc</kbd> | Go back, or close |
| <kbd>⌃</kbd><kbd>⌥</kbd><kbd>←</kbd><kbd>→</kbd><kbd>↑</kbd><kbd>↓</kbd> | Snap the focused window to a half |
| <kbd>⌃</kbd><kbd>⌘</kbd> + drag | Move or resize the window under the cursor |
| <kbd>⏯</kbd> <kbd>⏭</kbd> <kbd>⏮</kbd> | Play/pause, next and previous in your music app (with Media Keys on) |

Give any app, system action or setting its own hotkey, and pair it with the Hyper Key so it never clashes
with another app's shortcuts.

## Yours to keep

Jaycast is free and open source under the [MIT licence](LICENSE), and it stays that way.

- **No subscription, no Pro tier, no trial.** Every feature is included.
- **No account to sign in to**, and no server that can switch your app off.
- **Your data stays in files you own**, on your Mac.
- **Read the source, change it, build it yourself.** It's a `just run` away.

## Install

Download the `.dmg` from the [latest release](https://github.com/jamieweavis/jaycast/releases/latest), drag
Jaycast to Applications and grant Accessibility access when asked. Media Keys also asks, once per music
app, to control it. Requires macOS 26 or later.

## Development

### Requirements

- macOS 26+
- Rust (stable)
- Xcode, for its toolchain and SDK (the IDE is never opened)
- [`just`](https://just.systems)

### Commands

```sh
just run        # build the core + shell, bundle build/Jaycast.app, launch it
just test       # Rust tests
just check      # everything CI runs: fmt, clippy, tests, app build
just logs       # stream the app's logs
just            # list all recipes
```

`CONFIG=release just app` builds optimised. The app is signed with your "Apple Development" identity by
default, so macOS keeps the Accessibility grant across rebuilds. Set `JAYCAST_SIGN_IDENTITY="-"` to sign
ad hoc.

### Project layout

| Path | What's there |
| --- | --- |
| `src/` | The Rust core: all logic and system integration, built as `libjaycast_core.a`. |
| `shell/` | Swift/SwiftUI: menu bar, palette, settings and onboarding. Calls the core through [UniFFI](https://mozilla.github.io/uniffi-rs/) bindings generated at build time. |
| `tools/uniffi-bindgen/` | The bindings generator, pinned to the core's UniFFI version. |

## Releasing

Commits follow [Conventional Commits](https://www.conventionalcommits.org) (`feat:`, `fix:`, …). On every
push to `main`, [release-plz](https://release-plz.dev) opens or updates a release PR that bumps the version
and writes `CHANGELOG.md`. Merging it tags `vX.Y.Z`, and CI attaches the `.dmg` (`just dmg`) to the GitHub
Release.

CI needs a `RELEASE_PLZ_TOKEN` secret: a token with contents and pull-requests write access, so its PRs
trigger CI.
