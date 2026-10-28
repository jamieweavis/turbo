import os

/// Unified logging, under the same subsystem as the Rust core; `just logs` streams both.
/// Core error messages are safe to make public. Never log what the user typed or copied.
extension Logger {
    private static let subsystem = "com.jamieweavis.jaycast"

    static let app = Logger(subsystem: subsystem, category: "app")
    static let palette = Logger(subsystem: subsystem, category: "palette")
    static let settings = Logger(subsystem: subsystem, category: "settings")
    static let input = Logger(subsystem: subsystem, category: "input")
    static let display = Logger(subsystem: subsystem, category: "display")
    static let keepAwake = Logger(subsystem: subsystem, category: "keep_awake")
    static let clipboard = Logger(subsystem: subsystem, category: "clipboard")
}
