# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

Jaycast is a macOS menu bar app (launcher, clipboard history, snippets, window management, mouse fixes,
DDC monitor brightness, hyper key, keep awake). A Rust core does all the work; a thin SwiftUI shell
draws the UI. Requires macOS 26+, Rust stable, Xcode's toolchain (the IDE is never opened) and `just`.

## Commands

```sh
just run              # build core + bindings + shell, bundle build/Jaycast.app, (re)launch it
just test             # cargo test --workspace (Rust only; the shell has no tests)
just lint             # cargo fmt --check + clippy with -D warnings
just check            # everything CI runs: lint, test, app build
just logs [info]      # stream the app's os_log output (subsystem com.jamieweavis.jaycast)
just logs-recent 1h   # past logs (debug level isn't kept)
just reset-permissions  # forget the Accessibility grant to re-test onboarding
CONFIG=release just app # optimised build; `just dmg` packages it
```

- Single test: `cargo test <name_substring>` (e.g. `cargo test calc::parser`). Some tests are
  `#[ignore]` because they touch the real machine (app index, currency fetch, keystroke insertion);
  run them with `cargo test <name> -- --ignored --nocapture`.
- The app is signed with the "Apple Development" identity by default so macOS keeps the Accessibility
  grant across rebuilds. `JAYCAST_SIGN_IDENTITY="-"` signs ad hoc (what CI does).
- `.cargo/config.toml` sets `MACOSX_DEPLOYMENT_TARGET=26.0` so bundled C code (SQLite) matches the app.

## Architecture

**Rust core, thin Swift shell.** Anything that decides or does something belongs in `src/`. The shell
(`shell/Sources/Jaycast/`) draws UI and forwards intent. When adding a feature, put logic, state,
persistence and system integration in Rust and expose it over FFI; don't implement behaviour in Swift.

**Build pipeline** (see `justfile`): `cargo build -p jaycast` produces `libjaycast_core.a` (crate name
`jaycast`, lib name `jaycast_core`) → `tools/uniffi-bindgen` generates Swift bindings from the static
library → the header/modulemap go to `shell/Sources/JaycastCoreFFI/include/` and `jaycast_core.swift`
to `shell/Sources/Jaycast/Generated/` (both gitignored, regenerated on every build) → SwiftPM builds the
shell linking the `.a` → the bundle is assembled from `Resources/Info.plist` and codesigned. Any change
to an exported Rust type or method requires rebuilding through `just build`/`just run` for Swift to see it.
Frameworks the Rust crates need but Swift doesn't pull in must be added to `linkerSettings` in
`shell/Package.swift` (a static lib can't carry link flags). The app is signed with the hardened runtime
and `Resources/Jaycast.entitlements`: a protected device (the camera, today) needs both an entitlement there
and a usage string in `Info.plist`, or macOS silently refuses it (missing the string kills the app on the
prompt).

**FFI surface:** `src/lib.rs` defines the single `Core` `uniffi::Object` the shell creates at launch.
It owns one subsystem per feature module (`input`, `display`, `launcher`, `clipboard`, `files`,
`windows`, `snippets`, `hyper`, `keep_awake`, `color_picker`, `emoji`, `camera`) and exposes paired
`<area>_config()` / `set_<area>_config()` methods plus feature actions. Callbacks to Swift go through
observer traits (`DisplayObserver`, `LauncherObserver`) registered via `set_*_observer`.

**Shell side:** `AppModel` owns the `Core` and one `@Observable` settings/state class per area
(`LauncherSettings`, `DisplaySettings`, …). Observer bridges (e.g. `LauncherObserverBridge`) receive
Rust callbacks on arbitrary threads and must hop to `DispatchQueue.main.async` before
`MainActor.assumeIsolated`. Slow calls (Spotlight file search) run in `Task.detached` with debouncing.

**Config:** one `Config` struct (`src/config.rs`) persisted as TOML at `~/.config/jaycast.toml`.
Every struct is `#[serde(default)]` so older/partial configs keep loading — keep it that way when adding
fields, and use `#[serde(alias = ...)]` when renaming. `ConfigSaver` debounces writes (500 ms) and
writes atomically (temp + rename). A legacy `config.json` in the data dir is migrated on first load.
Other state (clipboard SQLite DB, usage ranking, colour/emoji history, chosen camera) lives in
`~/Library/Application Support/Jaycast`, passed to `Core::new` as `data_dir`.

**Event taps** (`input/tap.rs`, `hyper/tap.rs`, `snippets/tap.rs`, `windows/drag/tap.rs`,
`display/keys.rs`): each runs on its own thread/run loop, wraps the callback in `catch_unwind` and passes
the event through on panic, reads config via a generation counter rather than locking per event,
re-enables itself after macOS disables it, and ignores its own synthetic events. Follow this pattern
for new taps. Taps need Accessibility; global hotkeys don't (`start_hotkeys` runs before the grant).

**Conventions:** use `parking_lot` locks (no poisoning, so a panicking background thread can't crash
every FFI call). `unsafe` blocks carry `SAFETY` comments. Don't hold locks across I/O on paths the
palette calls per keystroke (`search`, `clipboard_items`). Disabled features must stop running
entirely, not just hide.

## Commits and releases

Use Conventional Commits (`feat:`, `fix:`, `docs:` …). release-plz builds `CHANGELOG.md` and version
bumps from them on every push to `main`; merging its release PR tags `vX.Y.Z` and CI attaches the
`.dmg`. The version lives in `[workspace.package]` in `Cargo.toml` — don't bump it by hand.
