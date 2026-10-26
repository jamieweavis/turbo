import AppKit
import Observation
import SwiftUI

/// The on-screen indicator shown when the brightness keys change an external display.
///
/// Jaycast draws its own HUD by default. The private `OSDManager` still exists on macOS 27,
/// but calling it showed no window when tested, so it is opt-in for experimenting:
/// `defaults write com.jamieweavis.jaycast UseSystemOSD -bool YES`.
@MainActor
final class BrightnessIndicator {
    private let systemOSD = UserDefaults.standard.bool(forKey: "UseSystemOSD") ? SystemOSD() : nil
    private var hud: BrightnessHUD?

    func show(brightness: Double, on displayID: UInt32) {
        if let systemOSD, systemOSD.show(brightness: brightness, on: displayID) {
            return
        }
        let hud = self.hud ?? BrightnessHUD()
        self.hud = hud
        hud.show(brightness: brightness, on: displayID)
    }
}

/// A borderless, click-through panel near the bottom of the target screen. Reused across
/// key presses; each press restarts the fade-out.
@MainActor
private final class BrightnessHUD {
    private let state = HUDState()
    private let panel: NSPanel
    private var hideTask: Task<Void, Never>?

    private static let size = NSSize(width: 220, height: 48)
    private static let bottomMargin: CGFloat = 96
    private static let visibleFor = Duration.milliseconds(1500)

    init() {
        panel = NSPanel(
            contentRect: NSRect(origin: .zero, size: Self.size),
            styleMask: [.borderless, .nonactivatingPanel],
            backing: .buffered,
            defer: true
        )
        panel.level = NSWindow.Level(rawValue: Int(CGWindowLevelForKey(.overlayWindow)))
        panel.collectionBehavior = [.canJoinAllSpaces, .stationary, .ignoresCycle, .fullScreenAuxiliary]
        panel.ignoresMouseEvents = true
        panel.isOpaque = false
        panel.backgroundColor = .clear
        panel.hasShadow = false
        panel.contentView = NSHostingView(rootView: BrightnessHUDView(state: state))
    }

    func show(brightness: Double, on displayID: UInt32) {
        state.brightness = brightness
        let screen = NSScreen.screens.first { $0.displayID == displayID } ?? NSScreen.main
        if let frame = screen?.frame {
            panel.setFrameOrigin(NSPoint(x: frame.midX - Self.size.width / 2, y: frame.minY + Self.bottomMargin))
        }

        panel.animator().alphaValue = 1
        if !panel.isVisible {
            panel.alphaValue = 1
            panel.orderFrontRegardless()
        }

        hideTask?.cancel()
        hideTask = Task { [weak self] in
            try? await Task.sleep(for: Self.visibleFor)
            guard !Task.isCancelled else { return }
            self?.fadeOut()
        }
    }

    private func fadeOut() {
        NSAnimationContext.runAnimationGroup { context in
            context.duration = 0.3
            panel.animator().alphaValue = 0
        } completionHandler: { [weak self] in
            MainActor.assumeIsolated {
                guard let self, self.panel.alphaValue == 0 else { return }
                self.panel.orderOut(nil)
            }
        }
    }
}

@MainActor
@Observable
private final class HUDState {
    var brightness = 0.0
}

private struct BrightnessHUDView: View {
    let state: HUDState

    var body: some View {
        HStack(spacing: 12) {
            Image(systemName: "sun.max.fill")
                .font(.title3)
                .foregroundStyle(.secondary)
            LevelBar(level: state.brightness)
                .frame(height: 6)
                .animation(.easeOut(duration: 0.12), value: state.brightness)
        }
        .padding(.horizontal, 18)
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .glassEffect(.regular, in: .capsule)
    }
}

private struct LevelBar: View {
    let level: Double

    var body: some View {
        GeometryReader { proxy in
            ZStack(alignment: .leading) {
                Capsule().fill(Color.secondary.opacity(0.3))
                Capsule()
                    .fill(Color.primary)
                    .frame(width: proxy.size.width * min(max(level, 0), 1))
            }
        }
    }
}

/// The private OSD.framework's `OSDManager`, looked up at runtime so a missing class or
/// changed selector just means falling back to Jaycast's own HUD.
@MainActor
private final class SystemOSD {
    private typealias ShowImage = @convention(c) (
        NSObject, Selector, Int64, UInt32, UInt32, UInt32, UInt32, UInt32, Bool
    ) -> Void

    private let manager: NSObject?
    private let selector = NSSelectorFromString(
        "showImage:onDisplayID:priority:msecUntilFade:filledChiclets:totalChiclets:locked:"
    )

    private static let brightnessImage: Int64 = 1
    private static let priority: UInt32 = 0x1F4
    private static let fadeAfterMs: UInt32 = 1000
    private static let chiclets: UInt32 = 16

    init() {
        dlopen("/System/Library/PrivateFrameworks/OSD.framework/OSD", RTLD_LAZY)
        let shared = NSSelectorFromString("sharedManager")
        guard let cls = NSClassFromString("OSDManager") as? NSObject.Type, cls.responds(to: shared) else {
            manager = nil
            return
        }
        manager = cls.perform(shared)?.takeUnretainedValue() as? NSObject
    }

    /// Returns false if the OSD isn't available, so the caller can fall back.
    func show(brightness: Double, on displayID: UInt32) -> Bool {
        guard let manager, manager.responds(to: selector), let imp = manager.method(for: selector) else {
            return false
        }
        let filled = UInt32((brightness * Double(Self.chiclets)).rounded())
        let showImage = unsafeBitCast(imp, to: ShowImage.self)
        showImage(manager, selector, Self.brightnessImage, displayID, Self.priority, Self.fadeAfterMs, filled, Self.chiclets, false)
        return true
    }
}

extension NSScreen {
    var displayID: UInt32? {
        (deviceDescription[NSDeviceDescriptionKey("NSScreenNumber")] as? NSNumber)?.uint32Value
    }
}
