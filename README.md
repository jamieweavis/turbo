# Jaycast

One macOS app that combines a keyboard launcher, mouse fixes (smooth scrolling, button remapping)
and external monitor brightness control.

- `src/`: the Rust core. All logic and system integration, built as `libjaycast_core.a`.
- `shell/`: Swift/SwiftUI. Menu bar, palette, settings and onboarding. It calls the core
  through [UniFFI](https://mozilla.github.io/uniffi-rs/) bindings generated at build time.
- `tools/uniffi-bindgen/`: the bindings generator, pinned to the same UniFFI version as the core.

## Requirements

macOS 26+, Rust (stable), Xcode installed (its toolchain and SDK; the IDE is never opened) and [`just`](https://just.systems).

## Usage

```sh
just run        # build the core + shell, bundle build/Jaycast.app, launch it
just test       # Rust tests
just            # list all recipes
```

`CONFIG=release just app` builds optimised. The app is signed with your "Apple Development"
identity by default, so macOS keeps the Accessibility grant across rebuilds. Override with
`JAYCAST_SIGN_IDENTITY="-"` to sign ad hoc.
