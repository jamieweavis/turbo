<div align="center">

# Jaycast

**A keyboard launcher, mouse fixes and monitor brightness control for macOS, all in one app.**

[![build](https://github.com/jamieweavis/jaycast/actions/workflows/build.yml/badge.svg)](https://github.com/jamieweavis/jaycast/actions/workflows/build.yml)
[![release](https://img.shields.io/github/v/release/jamieweavis/jaycast?label=release)](https://github.com/jamieweavis/jaycast/releases/latest)
[![Downloads](https://img.shields.io/github/downloads/jamieweavis/jaycast/total?color=f97316)](https://github.com/jamieweavis/jaycast/releases)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue)](LICENSE)
![Platforms](https://img.shields.io/badge/platform-macOS-lightgrey)
![Rust](https://img.shields.io/badge/rust-1.88%2B-orange)

[**Download**](https://github.com/jamieweavis/jaycast/releases/latest) · [Features](#features) · [Development](#development)

</div>

---

## Contents

- [Features](#features)
  - [Launcher](#-launcher)
  - [Utilities](#-utilities)
  - [Coming soon](#-coming-soon)
  - [Config file](#-config-file)
- [Install](#install)
- [Development](#development)
  - [Requirements](#requirements)
  - [Commands](#commands)
  - [Project layout](#project-layout)
- [Releasing](#releasing)

## Features

### 🔍 Launcher

One palette for everything, opened with a global hotkey.

| | |
| --- | --- |
| **Launcher** | Fuzzy-search apps (ranked by use), system actions, System Settings panes and emoji, with per-app hotkeys and aliases. Doubles as a calculator with unit and currency conversion. |
| **Quicklinks** | Open websites and searches by keyword: `g rust traits`. |
| **Shell Commands** | Run shell commands from the launcher. |
| **File Search** | Spotlight search, scoped to the folders you choose. |
| **Snippets** | Insert saved text by keyword, anywhere you type, with `{clipboard}`, `{date}` and `{cursor}` placeholders. |
| **Clipboard** | Search and paste everything you've copied, text and images. Password managers are skipped. |
| **Window Management** | Snap, maximise, centre, restore and move windows between displays with hotkeys. |
| **Keep Awake** | Stop your Mac (and optionally the display) sleeping while you need it. |
| **Colour Picker** | Pick a colour from anywhere on screen, copied and kept in a history. |

### 🧰 Utilities

Work in the background, no palette needed.

| | |
| --- | --- |
| **Window Transform** | Move and resize a window by dragging anywhere in it with <kbd>⌃</kbd><kbd>⌘</kbd> held. |
| **Hyper Key** | Turn Caps Lock or a modifier into <kbd>⌃</kbd><kbd>⌥</kbd><kbd>⇧</kbd><kbd>⌘</kbd> for shortcuts that never clash. |
| **Mouse** | Smooth scrolling, scroll modifiers and button remapping for ordinary mice. |
| **Brightness** | Control external monitors over DDC/CI with the brightness keys, dimming below their minimum to black. |

### 🚧 Coming soon

| | |
| --- | --- |
| **Screen Drawing** | Draw and annotate over the screen. |
| **Camera Preview** | Preview your webcam from the launcher, with <kbd>⌘</kbd><kbd>K</kbd> to switch camera. |
| **Icon Changer** | Swap macOS app icons, and reapply them automatically when apps update. *(Name TBC.)* |
| **Media Keys** | Send the media keys to the app you choose, such as Spotify, so they never launch Apple Music. |

### 📄 Config file

Settings live in `~/.config/jaycast.toml`.

## Install

Download the `.dmg` from the [latest release](https://github.com/jamieweavis/jaycast/releases/latest), drag
Jaycast to Applications and grant Accessibility access when asked.

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
