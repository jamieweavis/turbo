import AppKit
import os

/// `just screenshots`: launched with `JAYCAST_SCREENSHOTS=<dir>`, the app opens Settings, saves
/// each pane's window to `<dir>/<pane>.png` with `screencapture` (frame and shadow included) and
/// quits. Capturing needs Screen Recording access.
@MainActor
enum ScreenshotMode {
    static let directory: URL? = ProcessInfo.processInfo.environment["JAYCAST_SCREENSHOTS"]
        .flatMap { $0.isEmpty ? nil : URL(filePath: $0, directoryHint: .isDirectory) }

    static func capture(_ settings: SettingsWindowController, to directory: URL) async {
        do {
            try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        } catch {
            Logger.app.error("screenshots: couldn't create \(directory.path, privacy: .public): \(error, privacy: .public)")
            NSApp.terminate(nil)
            return
        }
        // Let the window finish appearing and the launcher's app list load.
        try? await Task.sleep(for: .seconds(2))
        for pane in SettingsPane.allCases {
            settings.show(pane)
            try? await Task.sleep(for: .seconds(1))
            guard let windowNumber = settings.windowNumber else { break }
            let file = directory.appending(path: "\(pane.slug).png")
            let process = Process()
            process.executableURL = URL(filePath: "/usr/sbin/screencapture")
            // -l: just this window, with its shadow; -x: no shutter sound.
            process.arguments = ["-x", "-l", String(windowNumber), file.path(percentEncoded: false)]
            let errors = Pipe()
            process.standardError = errors
            do {
                try process.run()
                process.waitUntilExit()
                if process.terminationStatus == 0 {
                    Logger.app.info("screenshots: saved \(pane.slug, privacy: .public)")
                } else {
                    let message = String(decoding: errors.fileHandleForReading.readDataToEndOfFile(), as: UTF8.self)
                    Logger.app.error("screenshots: \(pane.slug, privacy: .public) failed (\(process.terminationStatus)): \(message, privacy: .public)")
                }
            } catch {
                Logger.app.error("screenshots: \(pane.slug, privacy: .public) failed: \(error, privacy: .public)")
            }
        }
        NSApp.terminate(nil)
    }
}

extension SettingsPane {
    /// The screenshot's file name: "Window Management" → `window-management`.
    var slug: String {
        title.lowercased().replacing(" ", with: "-")
    }
}
