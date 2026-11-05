import Foundation
import Observation
import os

/// External displays and the monitor settings the UI edits. Settings changes are written
/// through to the core; brightness changes are throttled to one write per frame per display.
@MainActor
@Observable
final class DisplaySettings {
    private(set) var displays: [DisplayInfo] = []
    var config: MonitorConfig {
        didSet {
            if config != oldValue { save() }
        }
    }
    private(set) var saveError: String?
    private(set) var brightnessError: String?
    /// The built-in display's brightness, or nil when it isn't active (e.g. the lid is closed).
    private(set) var builtinBrightness: Double?

    /// Called when the brightness keys change a display, to show the on-screen indicator.
    @ObservationIgnored var onBrightnessFromKeys: ((_ displayID: UInt32, _ brightness: Double) -> Void)?

    @ObservationIgnored private let core: Core
    /// Latest slider value not yet sent to the core, per display.
    @ObservationIgnored private var pending: [UInt32: Double] = [:]
    /// Displays with a write loop in flight; their core echoes are ignored meanwhile so a
    /// dragged slider doesn't jump back to a stale value.
    @ObservationIgnored private var writing: Set<UInt32> = []

    /// When the built-in slider last wrote, so a refresh doesn't fight a drag.
    @ObservationIgnored private var lastBuiltinWrite = ContinuousClock.now

    private static let writeInterval = Duration.milliseconds(16)

    init(core: Core) {
        self.core = core
        config = core.monitorConfig()
        displays = core.displays()
        builtinBrightness = core.builtinBrightness()
    }

    /// Displays whose brightness can be controlled.
    var controllable: [DisplayInfo] {
        displays.filter(\.supportsDdc)
    }

    func setBrightness(_ brightness: Double, for displayID: UInt32) {
        updateLocal(displayID, brightness)
        pending[displayID] = brightness
        guard writing.insert(displayID).inserted else { return }
        Task { await writeLoop(displayID) }
    }

    /// Re-reads the built-in display's brightness, which macOS changes itself (the brightness
    /// keys, auto-brightness, opening and closing the lid).
    func refreshBuiltin() {
        guard ContinuousClock.now - lastBuiltinWrite > .milliseconds(500) else { return }
        builtinBrightness = core.builtinBrightness()
    }

    func setBuiltinBrightness(_ brightness: Double) {
        builtinBrightness = brightness
        lastBuiltinWrite = .now
        do {
            try core.setBuiltinBrightness(brightness: brightness)
            brightnessError = nil
        } catch {
            Logger.display.error("couldn't set built-in brightness: \(error, privacy: .public)")
            brightnessError = error.userMessage
        }
    }

    // MARK: Observer callbacks

    func displaysChanged(_ displays: [DisplayInfo]) {
        // Closing or opening the lid reconfigures displays; pick that up now rather than on the
        // menu's next poll, so the menu doesn't change height while it's open.
        refreshBuiltin()
        // Keep the slider's value for a display mid-drag rather than the core's older one.
        self.displays = displays.map { display in
            guard writing.contains(display.id), let current = self.displays.first(where: { $0.id == display.id }) else {
                return display
            }
            var display = display
            display.brightness = current.brightness
            return display
        }
    }

    func brightnessChanged(_ displayID: UInt32, _ brightness: Double, fromKeys: Bool) {
        if fromKeys {
            updateLocal(displayID, brightness)
            onBrightnessFromKeys?(displayID, brightness)
        } else if !writing.contains(displayID) {
            updateLocal(displayID, brightness)
        }
    }

    // MARK: Private

    private func updateLocal(_ displayID: UInt32, _ brightness: Double) {
        guard let index = displays.firstIndex(where: { $0.id == displayID }) else { return }
        displays[index].brightness = brightness
    }

    private func writeLoop(_ displayID: UInt32) async {
        while let brightness = pending.removeValue(forKey: displayID) {
            do {
                try core.setBrightness(displayId: displayID, brightness: brightness)
                brightnessError = nil
            } catch {
                Logger.display.error("couldn't set brightness of display \(displayID): \(error, privacy: .public)")
                brightnessError = error.userMessage
            }
            try? await Task.sleep(for: Self.writeInterval)
        }
        writing.remove(displayID)
    }

    private func save() {
        do {
            try core.setMonitorConfig(monitor: config)
            saveError = nil
        } catch {
            Logger.settings.error("couldn't save monitor settings: \(error, privacy: .public)")
            saveError = error.userMessage
        }
    }
}

/// Receives the core's display callbacks on background threads and forwards them, in order,
/// to the main actor.
/// Both live for the app's lifetime, so holding `settings` strongly is fine.
final class DisplayObserverBridge: DisplayObserver {
    private let settings: DisplaySettings

    init(settings: DisplaySettings) {
        self.settings = settings
    }

    func displaysChanged(displays: [DisplayInfo]) {
        DispatchQueue.main.async { [settings] in
            MainActor.assumeIsolated { settings.displaysChanged(displays) }
        }
    }

    func brightnessChanged(displayId: UInt32, brightness: Double, fromKeys: Bool) {
        DispatchQueue.main.async { [settings] in
            MainActor.assumeIsolated { settings.brightnessChanged(displayId, brightness, fromKeys: fromKeys) }
        }
    }
}
