// swift-tools-version: 6.2
import PackageDescription

let package = Package(
    name: "Turbo",
    platforms: [.macOS(.v26)],
    targets: [
        // UniFFI's C header for the Rust core. `just bindings` generates the header,
        // and `just build` puts libturbo_core.a on the linker search path.
        // A static library can't carry its own link flags, so frameworks the Rust crates
        // need but Swift doesn't already pull in are listed here.
        .target(
            name: "turbo_coreFFI",
            path: "Sources/TurboCoreFFI",
            linkerSettings: [
                .linkedLibrary("turbo_core"),
                .linkedFramework("CoreDisplay"),  // ddc-macos
                .linkedFramework("AVFoundation"),  // camera
                .linkedFramework("CoreServices"),  // media keys' Apple Event permission check
                .linkedFramework("CoreAudio"),  // sound devices and volume
                .linkedFramework("CoreWLAN"),  // Toggle Wi-Fi
            ]
        ),
        .executableTarget(
            name: "Turbo",
            dependencies: ["turbo_coreFFI"],
            path: "Sources/Turbo"
        ),
    ]
)
