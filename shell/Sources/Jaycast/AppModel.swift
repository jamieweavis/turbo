import Foundation
import Observation

/// Owns the Rust core and the state the UI observes. Created once at launch.
@MainActor
@Observable
final class AppModel {
    let core: Core
    let accessibility = AccessibilityPermission()
    let mouse: MouseSettings
    let displays: DisplaySettings
    @ObservationIgnored private let brightnessIndicator: BrightnessIndicator
    private(set) var isInputRunning = false
    private(set) var inputError: String?

    init() throws {
        let dataDirectory = URL.applicationSupportDirectory.appending(path: "Jaycast", directoryHint: .isDirectory)
        try FileManager.default.createDirectory(at: dataDirectory, withIntermediateDirectories: true)
        core = try Core(dataDir: dataDirectory.path(percentEncoded: false))
        mouse = MouseSettings(core: core)
        displays = DisplaySettings(core: core)
        brightnessIndicator = BrightnessIndicator(settings: displays)
    }

    /// Starts input as soon as Accessibility is granted (now or later) and stops it if revoked.
    func start() {
        displays.onBrightnessFromKeys = { [brightnessIndicator] displayID, _ in
            brightnessIndicator.show(on: displayID)
        }
        core.setDisplayObserver(observer: DisplayObserverBridge(settings: displays))
        // Catch anything that changed between init and registering the observer.
        displays.displaysChanged(core.displays())

        accessibility.startWatching()
        Task {
            for await granted in Observations({ [accessibility] in accessibility.isGranted }) {
                updateInput(granted: granted)
            }
        }
    }

    func stopInput() {
        core.stopInput()
        isInputRunning = false
    }

    private func updateInput(granted: Bool) {
        if granted {
            do {
                try core.startInput()
                inputError = nil
            } catch {
                inputError = error.userMessage
            }
        } else {
            core.stopInput()
            inputError = nil
        }
        isInputRunning = core.isInputRunning()
    }
}

/// The mouse settings the UI edits. Every change is written through to the core, which
/// saves it and applies it to the running event tap.
@MainActor
@Observable
final class MouseSettings {
    var config: MouseConfig {
        didSet {
            if config != oldValue { save() }
        }
    }
    private(set) var saveError: String?
    @ObservationIgnored private let core: Core

    init(core: Core) {
        self.core = core
        config = core.mouseConfig()
    }

    /// Assigns a modifier to one scroll function, taking it away from any other function,
    /// since one modifier can't mean two things.
    func setModifier(_ modifier: Modifier?, for function: ScrollFunction) {
        var modifiers = config.scrollModifiers
        if let modifier {
            for other in ScrollFunction.allCases where other != function && modifiers[keyPath: other.keyPath] == modifier {
                modifiers[keyPath: other.keyPath] = nil
            }
        }
        modifiers[keyPath: function.keyPath] = modifier
        config.scrollModifiers = modifiers
    }

    func function(using modifier: Modifier) -> ScrollFunction? {
        ScrollFunction.allCases.first { config.scrollModifiers[keyPath: $0.keyPath] == modifier }
    }

    private func save() {
        do {
            try core.setMouseConfig(mouse: config)
            saveError = nil
        } catch {
            saveError = error.userMessage
        }
    }
}

extension Error {
    /// A message fit for the UI; the generated `CoreError.errorDescription` is debug output.
    var userMessage: String {
        guard let error = self as? CoreError else { return localizedDescription }
        switch error {
        case .PermissionDenied: return "Accessibility permission is required."
        case .Io(let message): return "Couldn't save settings: \(message)"
        case .InvalidConfig(let message): return "Invalid settings: \(message)"
        case .Other(let message): return message
        }
    }
}
