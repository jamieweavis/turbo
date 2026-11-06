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
