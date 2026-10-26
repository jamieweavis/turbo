// swift-tools-version: 6.2
import PackageDescription

let package = Package(
    name: "Jaycast",
    platforms: [.macOS(.v26)],
    targets: [
        // UniFFI's C header for the Rust core. `just bindings` generates the header,
        // and `just build` puts libjaycast_core.a on the linker search path.
        .target(
            name: "jaycast_coreFFI",
            path: "Sources/JaycastCoreFFI",
            linkerSettings: [.linkedLibrary("jaycast_core")]
        ),
        .executableTarget(
            name: "Jaycast",
            dependencies: ["jaycast_coreFFI"],
            path: "Sources/Jaycast"
        ),
    ]
)
