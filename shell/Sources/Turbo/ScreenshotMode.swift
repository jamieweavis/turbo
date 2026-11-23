import AppKit
import os

/// `just screenshots`: launched with `TURBO_SCREENSHOTS=<dir>`, the app hides every other app,
/// captures the launcher palette over the desktop to `<dir>/palette.png`, then opens Settings and
/// saves each pane's window to `<dir>/<pane>.png` with `screencapture` (frame and shadow
/// included), unhides the apps it hid and quits. Capturing needs Screen Recording access.
@MainActor
enum ScreenshotMode {
    static let directory: URL? = ProcessInfo.processInfo.environment["TURBO_SCREENSHOTS"]
        .flatMap { $0.isEmpty ? nil : URL(filePath: $0, directoryHint: .isDirectory) }

    /// Desktop showing around the palette, in points.
    private static let palettePadding: CGFloat = 48

    static func capture(palette: PaletteWindowController, settings: SettingsWindowController, to directory: URL) async {
        do {
            try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        } catch {
            Logger.app.error("screenshots: couldn't create \(directory.path, privacy: .public): \(error, privacy: .public)")
            NSApp.terminate(nil)
            return
        }
        // Frontmost first: an app can't hide the one that's active.
        NSApp.activate()
        try? await Task.sleep(for: .milliseconds(500))
        let hidden = hideOtherApps()

        // Let the other apps' windows go and the launcher's app list load.
        try? await Task.sleep(for: .seconds(2))
        palette.show()
        try? await Task.sleep(for: .seconds(1))
        let region = palette.panelFrame.insetBy(dx: -palettePadding, dy: -palettePadding)
        // screencapture -R takes points from the top-left of the main display; AppKit's origin is
        // its bottom-left.
        let mainHeight = NSScreen.screens.first?.frame.maxY ?? 0
        let rect = [region.minX, mainHeight - region.maxY, region.width, region.height]
            .map { String(Int($0.rounded())) }
            .joined(separator: ",")
        screencapture(["-R", rect], name: "palette", in: directory)
        palette.hide()

        // General is left out: the palette shot replaces it at the top of the README.
        for pane in SettingsPane.allCases where pane != .general {
            settings.show(pane)
            try? await Task.sleep(for: .seconds(1))
            guard let windowNumber = settings.windowNumber else { break }
            // -l: just this window, with its shadow.
            screencapture(["-l", String(windowNumber)], name: pane.slug, in: directory)
        }
        // Before quitting: terminate doesn't return.
        hidden.forEach { $0.unhide() }
        NSApp.terminate(nil)
    }

    /// Hides every visible regular app but this one, returning them to unhide afterwards.
    private static func hideOtherApps() -> [NSRunningApplication] {
        let me = NSRunningApplication.current
        let visible = NSWorkspace.shared.runningApplications.filter {
            $0 != me && $0.activationPolicy == .regular && !$0.isHidden
        }
        NSApp.hideOtherApplications(nil)
        return visible
    }

    private static func screencapture(_ arguments: [String], name: String, in directory: URL) {
        let file = directory.appending(path: "\(name).png")
        let process = Process()
        process.executableURL = URL(filePath: "/usr/sbin/screencapture")
        // -x: no shutter sound.
        process.arguments = ["-x"] + arguments + [file.path(percentEncoded: false)]
        let errors = Pipe()
        process.standardError = errors
        do {
            try process.run()
            process.waitUntilExit()
            if process.terminationStatus == 0 {
                Logger.app.info("screenshots: saved \(name, privacy: .public)")
            } else {
                let message = String(decoding: errors.fileHandleForReading.readDataToEndOfFile(), as: UTF8.self)
                Logger.app.error("screenshots: \(name, privacy: .public) failed (\(process.terminationStatus)): \(message, privacy: .public)")
            }
        } catch {
            Logger.app.error("screenshots: \(name, privacy: .public) failed: \(error, privacy: .public)")
        }
    }
}

extension SettingsPane {
    /// The screenshot's file name: "Window Management" → `window-management`.
    var slug: String {
        title.lowercased().replacing(" ", with: "-")
    }
}
