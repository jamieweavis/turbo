import Foundation
import Observation

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

    /// Called when the brightness keys change a display, to show the on-screen indicator.
    @ObservationIgnored var onBrightnessFromKeys: ((_ displayID: UInt32, _ brightness: Double) -> Void)?

    @ObservationIgnored private let core: Core
    /// Latest slider value not yet sent to the core, per display.
    @ObservationIgnored private var pending: [UInt32: Double] = [:]
    /// Displays with a write loop in flight; their core echoes are ignored meanwhile so a
    /// dragged slider doesn't jump back to a stale value.
    @ObservationIgnored private var writing: Set<UInt32> = []

    private static let writeInterval = Duration.milliseconds(16)

    init(core: Core) {
        self.core = core
        config = core.monitorConfig()
        displays = core.displays()
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

    // MARK: Observer callbacks

    func displaysChanged(_ displays: [DisplayInfo]) {
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
