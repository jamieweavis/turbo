import AppKit
import os
import ScreenCaptureKit
import UniformTypeIdentifiers

/// `just screenshots`: launched with `TURBO_SCREENSHOTS=<dir>`, the app hides every other app,
/// captures the launcher palette over the desktop to `<dir>/palette.png` and the open menu bar
/// popover under Turbo's icon (alone in the menu bar) to `<dir>/menubar.png`, then opens Settings and
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
        screencapture(region: region, name: "palette", in: directory)
        palette.hide()

        await captureMenuBar(in: directory)

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

    /// Opens the menu bar popover and saves `menubar.png` (see `composeMenuBar`).
    private static func captureMenuBar(in directory: URL) async {
        // The status item can take a moment to reach the menu bar after launch.
        var button = statusItemButton()
        for _ in 0..<30 where button == nil {
            try? await Task.sleep(for: .milliseconds(100))
            button = statusItemButton()
        }
        guard let button else {
            let windows = NSApp.windows.filter { $0.className.contains("StatusBarWindow") }
                .map { NSStringFromRect($0.frame) }.joined(separator: ", ")
            Logger.app.error("screenshots: couldn't find the menu bar icon (status windows: \(windows, privacy: .public))")
            return
        }
        // SwiftUI's status item ignores performClick, so it gets a real click; another if it didn't
        // take (the icon can still be settling into place).
        var opened: (popover: NSWindow, icon: NSRect)?
        for _ in 0..<3 where opened == nil {
            guard let icon = button.window?.frame else { break }
            click(at: icon)
            for _ in 0..<10 {
                try? await Task.sleep(for: .milliseconds(100))
                if let popover = NSApp.windows.first(where: { $0.isVisible && $0.className.contains("MenuBarExtra") }) {
                    opened = (popover, icon)
                    break
                }
            }
        }
        guard let (popover, icon) = opened else {
            let windows = NSApp.windows.filter(\.isVisible).map(\.className).joined(separator: ", ")
            Logger.app.error("screenshots: the menu bar popover didn't open (visible windows: \(windows, privacy: .public))")
            return
        }
        // Off to the side, so it doesn't highlight a control in the popover; then time for it to settle.
        if let screen = NSScreen.screens.first {
            CGWarpMouseCursorPosition(CGPoint(x: screen.frame.width * 0.1, y: screen.frame.height * 0.5))
        }
        try? await Task.sleep(for: .seconds(1))
        do {
            try await composeMenuBar(popover: popover.frame, icon: icon, to: directory.appending(path: "menubar.png"))
            Logger.app.info("screenshots: saved menubar")
        } catch {
            Logger.app.error("screenshots: menubar failed: \(error, privacy: .public)")
        }
        popover.orderOut(nil)
        try? await Task.sleep(for: .milliseconds(500))
    }

    /// Draws the menu bar shot with only Turbo's icon in the menu bar, whatever else is there: the
    /// wallpaper and the popover, captured without the menu bar, then Turbo's icon (with its open
    /// highlight) cut from an ordinary capture where it is. The popover sits in the middle.
    private static func composeMenuBar(popover: NSRect, icon: NSRect, to file: URL) async throws {
        guard let screen = NSScreen.screens.first,
              let displayID = screen.deviceDescription[NSDeviceDescriptionKey("NSScreenNumber")] as? CGDirectDisplayID
        else { throw ScreenshotError("no main display") }
        let scale = screen.backingScaleFactor
        let content = try await SCShareableContent.excludingDesktopWindows(false, onScreenWindowsOnly: true)
        guard let display = content.displays.first(where: { $0.displayID == displayID }) else {
            throw ScreenshotError("ScreenCaptureKit doesn't list the main display")
        }
        // The background keeps the wallpaper (WindowManager's), the shading macOS puts behind the
        // menu bar ("underbelly") and Turbo's own windows, leaving out the menu bar and its icons.
        let me = ProcessInfo.processInfo.processIdentifier
        let onMain = { (window: SCWindow) in window.frame.minX == 0 && window.frame.minY == 0 }
        let backgroundWindows = content.windows.filter {
            ($0.owningApplication?.processID == me && $0.isOnScreen)
                || (onMain($0) && ($0.title == "underbelly"
                    || ($0.title == "Wallpaper" && $0.owningApplication?.bundleIdentifier == "com.apple.WindowManager")))
        }

        // In points from the main display's top left, which ScreenCaptureKit and the
        // accessibility API use; x is the same as AppKit's.
        let left = popover.minX - menuBarMargin
        let right = popover.maxX + menuBarMargin
        let height = screen.frame.maxY - popover.minY + menuBarMarginBottom
        let region = CGRect(x: left, y: 0, width: right - left, height: height)

        func capture(_ filter: SCContentFilter, _ source: CGRect) async throws -> CGImage {
            let config = SCStreamConfiguration()
            config.sourceRect = source
            config.width = Int((source.width * scale).rounded())
            config.height = Int((source.height * scale).rounded())
            config.showsCursor = false
            config.captureResolution = .best
            return try await SCScreenshotManager.captureImage(contentFilter: filter, configuration: config)
        }
        let background = try await capture(SCContentFilter(display: display, including: backgroundWindows), region)
        let everything = try await capture(SCContentFilter(display: display, excludingWindows: []), region)

        let width = background.width, imageHeight = background.height
        guard let context = CGContext(
            data: nil, width: width, height: imageHeight, bitsPerComponent: 8, bytesPerRow: 0,
            space: CGColorSpace(name: CGColorSpace.sRGB)!, bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue
        ) else { throw ScreenshotError("couldn't make a drawing context") }
        // Top-left origin in points, like the rects above.
        context.translateBy(x: 0, y: CGFloat(imageHeight))
        context.scaleBy(x: scale, y: -scale)
        /// Draws `rect` of `image` (in points from its top left) at `origin` in the region.
        func draw(_ image: CGImage, _ rect: CGRect, at origin: CGPoint) {
            let pixels = CGRect(x: rect.minX * scale, y: rect.minY * scale, width: rect.width * scale, height: rect.height * scale)
            guard let piece = image.cropping(to: pixels.integral) else { return }
            context.saveGState()
            // CGContext draws images upright in its own (bottom-left) space; flip back for this one.
            context.translateBy(x: origin.x, y: origin.y + rect.height)
            context.scaleBy(x: 1, y: -1)
            context.draw(piece, in: CGRect(origin: .zero, size: rect.size))
            context.restoreGState()
        }

        draw(background, CGRect(origin: .zero, size: region.size), at: .zero)
        let barHeight = screen.frame.maxY - screen.visibleFrame.maxY
        // The open highlight is a capsule a little wider than the button; the margin either side is
        // still within the button padding before a neighbouring icon, and matches the background.
        let turbo = CGRect(x: icon.minX - left, y: 0, width: icon.width, height: barHeight).insetBy(dx: -6, dy: 0)
        draw(everything, turbo, at: turbo.origin)

        guard let image = context.makeImage(),
              let destination = CGImageDestinationCreateWithURL(file as CFURL, UTType.png.identifier as CFString, 1, nil)
        else { throw ScreenshotError("couldn't write \(file.lastPathComponent)") }
        CGImageDestinationAddImage(destination, image, nil)
        guard CGImageDestinationFinalize(destination) else { throw ScreenshotError("couldn't write \(file.lastPathComponent)") }
    }

    private struct ScreenshotError: Error, CustomStringConvertible {
        let description: String
        init(_ description: String) { self.description = description }
    }

    /// Clicks the middle of `frame`, in AppKit's coordinates (origin at the main display's bottom left).
    private static func click(at frame: NSRect) {
        // CGEvent takes points from the top-left of the main display.
        let mainHeight = NSScreen.screens.first?.frame.maxY ?? 0
        let point = CGPoint(x: frame.midX, y: mainHeight - frame.midY)
        for type in [CGEventType.leftMouseDown, .leftMouseUp] {
            CGEvent(mouseEventSource: nil, mouseType: type, mouseCursorPosition: point, mouseButton: .left)?
                .post(tap: .cghidEventTap)
        }
    }

    /// Desktop showing beside and below the menu bar popover, in points.
    private static let menuBarMargin: CGFloat = 22
    private static let menuBarMarginBottom: CGFloat = 27

    /// The button of the status item SwiftUI's `MenuBarExtra` made in the main display's menu bar
    /// (each display's menu bar has its own copy).
    private static func statusItemButton() -> NSStatusBarButton? {
        func find(in view: NSView) -> NSStatusBarButton? {
            if let button = view as? NSStatusBarButton { return button }
            return view.subviews.lazy.compactMap(find).first
        }
        let main = NSScreen.screens.first?.frame ?? .zero
        return NSApp.windows
            .filter { $0.className.contains("StatusBarWindow") && main.intersects($0.frame) }
            .lazy.compactMap { $0.contentView.flatMap(find) }.first
    }

    /// Captures `region`, in AppKit's coordinates (origin at the main display's bottom left).
    private static func screencapture(region: NSRect, name: String, in directory: URL) {
        // screencapture -R takes points from the top-left of the main display.
        let mainHeight = NSScreen.screens.first?.frame.maxY ?? 0
        let rect = [region.minX, mainHeight - region.maxY, region.width, region.height]
            .map { String(Int($0.rounded())) }
            .joined(separator: ",")
        screencapture(["-R", rect], name: name, in: directory)
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
