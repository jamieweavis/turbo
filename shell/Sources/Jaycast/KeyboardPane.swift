import Observation
import SwiftUI

/// The Hyper key settings the UI edits. Every change is written through to the core, which
/// saves it and remaps the key at once.
@MainActor
@Observable
final class HyperKeySettings {
    /// The app's one instance, so shortcuts anywhere can show ⬥ for the modifiers it presses.
    static private(set) var current: HyperKeySettings?

    var config: HyperKeyConfig {
        didSet {
            if config != oldValue { save() }
        }
    }
    private(set) var saveError: String?
    @ObservationIgnored private let core: Core

    init(core: Core) {
        self.core = core
        config = core.hyperKeyConfig()
        Self.current = self
    }

    private func save() {
        do {
            try core.setHyperKeyConfig(hyperKey: config)
            saveError = nil
        } catch {
            saveError = error.userMessage
        }
    }
}

struct KeyboardPane: View {
    @Bindable var settings: HyperKeySettings
    let isInputRunning: Bool

    var body: some View {
        Form {
            PaneHeaderSection(pane: .keyboard)

            if let error = settings.saveError {
                Section {
                    Label(error, systemImage: "exclamationmark.triangle.fill")
                        .foregroundStyle(.red)
                }
            }

            Section {
                Picker("Key", selection: $settings.config.key) {
                    ForEach(HyperKeySource.allCases) { Text($0.title).tag($0) }
                }

                Toggle(isOn: $settings.config.includeShift) {
                    Text("Include Shift")
                    Text(settings.config.includeShift
                        ? "Hyper is ⌃⌥⇧⌘."
                        : "Hyper is ⌃⌥⌘, and Shift can be added on top.")
                }

                Picker(selection: $settings.config.quickPress) {
                    ForEach(QuickPress.allCases) { Text($0.title).tag($0) }
                } label: {
                    Text("Quick press")
                    Text("Pressing and releasing the key on its own.")
                }
            } footer: {
                if settings.config.enabled && !isInputRunning {
                    Text("The Hyper key works once Accessibility is granted.")
                        .foregroundStyle(.red)
                } else {
                    Text("Hold the Hyper key to press ⌃⌥⇧⌘ at once, then use it in any shortcut, including Jaycast's own hotkeys. The key loses its usual behaviour while it's the Hyper key.")
                }
            }
        }
        .formStyle(.grouped)
    }
}

extension HyperKeySource: CaseIterable, Identifiable {
    public static let allCases: [HyperKeySource] = [
        .capsLock,
        .leftControl, .leftOption, .leftCommand, .leftShift,
        .rightControl, .rightOption, .rightCommand, .rightShift,
    ]
    public var id: Self { self }

    var title: String {
        switch self {
        case .capsLock: "⇪ Caps Lock"
        case .leftControl: "Left ⌃ Control"
        case .leftShift: "Left ⇧ Shift"
        case .leftOption: "Left ⌥ Option"
        case .leftCommand: "Left ⌘ Command"
        case .rightControl: "Right ⌃ Control"
        case .rightShift: "Right ⇧ Shift"
        case .rightOption: "Right ⌥ Option"
        case .rightCommand: "Right ⌘ Command"
        }
    }
}

extension QuickPress: CaseIterable, Identifiable {
    public static let allCases: [QuickPress] = [.nothing, .escape, .toggleCapsLock]
    public var id: Self { self }

    var title: String {
        switch self {
        case .nothing: "Does nothing"
        case .escape: "Escape"
        case .toggleCapsLock: "Toggles Caps Lock"
        }
    }
}
