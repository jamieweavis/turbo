// swift-tools-version: 6.2
import PackageDescription

let package = Package(
    name: "Backpack",
    platforms: [.macOS(.v26)],
    targets: [
        // UniFFI's C header for the Rust core. `just bindings` generates the header,
        // and `just build` puts libbackpack_core.a on the linker search path.
        // A static library can't carry its own link flags, so frameworks the Rust crates
        // need but Swift doesn't already pull in are listed here.
        .target(
            name: "backpack_coreFFI",
            path: "Sources/BackpackCoreFFI",
            linkerSettings: [
                .linkedLibrary("backpack_core"),
                .linkedFramework("CoreDisplay"),  // ddc-macos
                .linkedFramework("AVFoundation"),  // camera
                .linkedFramework("CoreServices"),  // media keys' Apple Event permission check
                .linkedFramework("CoreAudio"),  // sound devices and volume
                .linkedFramework("CoreWLAN"),  // Toggle Wi-Fi
            ]
        ),
        .executableTarget(
            name: "Backpack",
            dependencies: ["backpack_coreFFI"],
            path: "Sources/Backpack"
        ),
    ]
)
