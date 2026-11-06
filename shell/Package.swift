// swift-tools-version: 6.2
import PackageDescription

let package = Package(
    name: "Jaycast",
    platforms: [.macOS(.v26)],
    targets: [
        // UniFFI's C header for the Rust core. `just bindings` generates the header,
        // and `just build` puts libjaycast_core.a on the linker search path.
        // A static library can't carry its own link flags, so frameworks the Rust crates
        // need but Swift doesn't already pull in are listed here.
        .target(
            name: "jaycast_coreFFI",
            path: "Sources/JaycastCoreFFI",
            linkerSettings: [
                .linkedLibrary("jaycast_core"),
                .linkedFramework("CoreDisplay"),  // ddc-macos
                .linkedFramework("AVFoundation"),  // camera
            ]
        ),
        .executableTarget(
            name: "Jaycast",
            dependencies: ["jaycast_coreFFI"],
            path: "Sources/Jaycast"
        ),
    ]
)
