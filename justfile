# Jaycast — run `just` to list recipes.

config := env("CONFIG", "debug")
# A stable identity keeps the Accessibility grant across rebuilds; "-" signs ad hoc.
sign_identity := env("JAYCAST_SIGN_IDENTITY", "Apple Development")
bundle_id := "com.jamieweavis.jaycast"

rust_flag := if config == "release" { "--release" } else { "" }
rust_out := justfile_directory() / "target" / config
ffi_dir := "shell/Sources/JaycastCoreFFI/include"
generated_dir := "shell/Sources/Jaycast/Generated"
app := "build/Jaycast.app"

default:
    @just --list

# Build the Rust core static library
core:
    cargo build -p jaycast {{rust_flag}}

# Regenerate the Swift bindings for the Rust core
bindings: core
    mkdir -p {{ffi_dir}} {{generated_dir}}
    cargo run -q -p uniffi-bindgen -- generate --library {{rust_out}}/libjaycast_core.a \
        --language swift --out-dir build/bindings
    mv build/bindings/jaycast_coreFFI.h {{ffi_dir}}/
    mv build/bindings/jaycast_coreFFI.modulemap {{ffi_dir}}/module.modulemap
    mv build/bindings/jaycast_core.swift {{generated_dir}}/

# Build the Swift shell, linked against the Rust core
build: bindings
    swift build --package-path shell -c {{config}} -Xlinker -L{{rust_out}}

# Assemble and sign build/Jaycast.app
app: build
    rm -rf {{app}}
    mkdir -p {{app}}/Contents/MacOS {{app}}/Contents/Resources
    cp Resources/Info.plist {{app}}/Contents/
    /usr/libexec/PlistBuddy -c "Set :CFBundleShortVersionString $({{just_executable()}} version)" \
        -c "Set :CFBundleVersion $({{just_executable()}} version)" {{app}}/Contents/Info.plist
    cp "$(swift build --package-path shell -c {{config}} --show-bin-path)/Jaycast" {{app}}/Contents/MacOS/
    codesign --force --options runtime --entitlements Resources/Jaycast.entitlements \
        --sign "{{sign_identity}}" {{app}}

# Package build/Jaycast.app as build/Jaycast-<version>-macos-arm64.dmg (CONFIG=release to ship)
dmg: app
    #!/usr/bin/env bash
    set -euo pipefail
    dmg="build/Jaycast-$({{just_executable()}} version)-macos-arm64.dmg"
    rm -rf build/dmg
    mkdir -p build/dmg
    cp -R {{app}} build/dmg/
    ln -s /Applications build/dmg/Applications
    hdiutil create -volname Jaycast -srcfolder build/dmg -format UDZO -ov "$dmg"
    (cd build && shasum -a 256 "$(basename "$dmg")" > "$(basename "$dmg").sha256")

# Print the app's version (the workspace version, which release-plz bumps)
version:
    @cargo pkgid -p jaycast | sed 's/.*[#@]//'

# Build, then (re)launch the app
run: app
    -pkill -x Jaycast
    # Wait for LaunchServices, not just the kernel, to see the old instance gone, or open fails with -600.
    @while [ -n "$(lsappinfo find bundleid={{bundle_id}})" ]; do sleep 0.1; done
    open {{app}}

# Capture every settings pane (dark, demo config) to .github/screenshots; needs Screen Recording
screenshots: app
    #!/usr/bin/env bash
    set -euo pipefail
    out="{{justfile_directory()}}/.github/screenshots"
    scratch="$(mktemp -d)"
    trap 'rm -rf "$scratch"' EXIT
    # A copy, so anything the app saves while running doesn't touch the checked-in demo config.
    cp "$out/demo-config.toml" "$scratch/config.toml"
    mkdir "$scratch/data"
    # Only one Jaycast can own the hotkeys; relaunch whichever copy was running afterwards.
    running="$(pgrep -x Jaycast | head -1 || true)"
    previous=""
    if [ -n "$running" ]; then
        previous="$(ps -o comm= -p "$running" | sed 's#/Contents/MacOS/.*##')"
        pkill -x Jaycast
        while [ -n "$(lsappinfo find bundleid={{bundle_id}})" ]; do sleep 0.1; done
    fi
    rm -f "$out"/*.png
    echo "Capturing settings panes, don't touch the mouse…"
    # Through LaunchServices rather than running the binary, so macOS treats it as Jaycast
    # itself: its Accessibility grant applies and the Screen Recording prompt names it.
    open -W \
        --env JAYCAST_SCREENSHOTS="$out" \
        --env JAYCAST_CONFIG="$scratch/config.toml" \
        --env JAYCAST_DATA_DIR="$scratch/data" \
        {{app}}
    if [ -n "$previous" ]; then open "$previous"; fi
    count="$(find "$out" -name '*.png' | wc -l | tr -d ' ')"
    expected="$(grep -o 'case general.*' shell/Sources/Jaycast/Settings.swift | tr ',' '\n' | wc -l | tr -d ' ')"
    if [ "$count" -ne "$expected" ]; then
        echo "Saved $count of $expected screenshots. Jaycast needs Screen Recording access (System Settings ›"
        echo "Privacy & Security); \`just logs-recent 5m\` shows what failed."
        exit 1
    fi
    if command -v oxipng >/dev/null; then oxipng -q -o 4 --strip safe "$out"/*.png; fi
    echo "Saved $count screenshots to .github/screenshots"

# Stream the app's logs; `just logs info` hides the debug ones
logs level="debug":
    log stream --style compact --level {{level}} --predicate 'subsystem == "{{bundle_id}}"'

# Show the last `period` of the app's logs (debug ones aren't kept)
logs-recent period="1h":
    log show --style compact --info --last {{period}} --predicate 'subsystem == "{{bundle_id}}"'

# Run all tests
test:
    cargo test --workspace

# Format Rust code
fmt:
    cargo fmt --all

# Check formatting and run clippy
lint:
    cargo fmt --all --check
    cargo clippy --workspace --all-targets -- -D warnings

# Run everything CI runs
check: lint test app

# Forget the Accessibility grant, to test onboarding again
reset-permissions:
    tccutil reset Accessibility {{bundle_id}}

clean:
    cargo clean
    rm -rf build shell/.build {{generated_dir}} {{ffi_dir}}
