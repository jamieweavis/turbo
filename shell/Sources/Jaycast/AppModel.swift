import Foundation
import Observation
import os

/// Owns the Rust core and the state the UI observes. Created once at launch.
@MainActor
@Observable
final class AppModel {
    let core: Core
    let accessibility = AccessibilityPermission()
    let mouse: MouseSettings
    let displays: DisplaySettings
    let launcher: LauncherSettings
    let hyperKey: HyperKeySettings
    let windowDrag: WindowDragSettings
    let mediaKeys: MediaKeysSettings
    let keepAwake: KeepAwakeState
    let colorPicker: ColorPickerState
    let emoji: EmojiState
    @ObservationIgnored let palette: PaletteWindowController
    @ObservationIgnored private let brightnessIndicator: BrightnessIndicator
    @ObservationIgnored private let commandOutput = CommandOutputWindows()
    @ObservationIgnored private let pill = Pill()
    private(set) var isInputRunning = false
    private(set) var inputError: String?

    init() throws {
        // `just screenshots` runs with a throwaway data directory, leaving the real history alone.
        let dataDirectory = ProcessInfo.processInfo.environment["JAYCAST_DATA_DIR"]
            .flatMap { $0.isEmpty ? nil : URL(filePath: $0, directoryHint: .isDirectory) }
            ?? URL.applicationSupportDirectory.appending(path: "Jaycast", directoryHint: .isDirectory)
        try FileManager.default.createDirectory(at: dataDirectory, withIntermediateDirectories: true)
        core = try Core(dataDir: dataDirectory.path(percentEncoded: false))
        mouse = MouseSettings(core: core)
        displays = DisplaySettings(core: core)
        brightnessIndicator = BrightnessIndicator(settings: displays)
        launcher = LauncherSettings(core: core)
        hyperKey = HyperKeySettings(core: core)
        windowDrag = WindowDragSettings(core: core)
        mediaKeys = MediaKeysSettings(core: core)
        keepAwake = KeepAwakeState(core: core)
        colorPicker = ColorPickerState(core: core)
        emoji = EmojiState(core: core)
        palette = PaletteWindowController(core: core, settings: launcher)
        palette.model.onPickColor = { [colorPicker] in colorPicker.pick() }
        palette.model.onColorsChanged = { [colorPicker] in colorPicker.refresh() }
        palette.model.onEmojiUsed = { [emoji] in emoji.refresh() }
        palette.model.onKeepAwakeChanged = { [keepAwake, pill] active in
            keepAwake.refresh()
            pill.show(.keepAwake(active))
        }
        palette.model.onStatus = { [pill] status in pill.show(.status(status)) }
        colorPicker.onPicked = { [pill] color, text in pill.show(.color(color, text: text)) }
    }

    /// Starts input as soon as Accessibility is granted (now or later) and stops it if revoked.
    func start() {
        displays.onBrightnessFromKeys = { [brightnessIndicator] displayID, _ in
            brightnessIndicator.show(on: displayID)
        }
        core.setDisplayObserver(observer: DisplayObserverBridge(settings: displays))
        // Catch anything that changed between init and registering the observer.
        displays.displaysChanged(core.displays())

        core.setLauncherObserver(observer: LauncherObserverBridge(
            onTogglePalette: { [palette] in palette.toggle() },
            onAppsChanged: { [launcher, palette] in
                launcher.refreshApps()
                if palette.isVisible { palette.model.refresh(ifShowing: .search) }
            },
            onCommandFinished: { [commandOutput] name, output, success in
                commandOutput.show(name: name, output: output, success: success)
            },
            onShowMode: { [palette] mode in palette.toggle(mode: mode) },
            onClipboardChanged: { [palette] in
                if palette.isVisible { palette.model.refresh(ifShowing: .clipboard) }
            },
            onKeepAwakeChanged: { [keepAwake, pill] active in
                keepAwake.refresh()
                pill.show(.keepAwake(active))
            },
            onPickColor: { [colorPicker] in colorPicker.pick() },
            onShowStatus: { [pill] status in pill.show(.status(status)) }
        ))
        // Hotkeys don't need Accessibility, so they work before it's granted.
        core.startHotkeys()
        launcher.refreshFailed()
        launcher.refreshApps()
        ShortcutRecorder.onRecordingChanged = { [core] recording in
            core.setHotkeysPaused(paused: recording)
        }

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
                Logger.input.info("input started")
            } catch {
                Logger.input.error("couldn't start input: \(error, privacy: .public)")
                inputError = error.userMessage
            }
        } else {
            Logger.input.info("input stopped")
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
            Logger.settings.error("couldn't save mouse settings: \(error, privacy: .public)")
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
