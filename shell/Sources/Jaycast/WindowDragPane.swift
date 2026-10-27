import Observation
import SwiftUI

/// The window drag settings the UI edits, written through to the core, which saves them and
/// applies them to the running tap.
@MainActor
@Observable
final class WindowDragSettings {
    var config: WindowDragConfig {
        didSet {
            if config != oldValue { save() }
        }
    }
    private(set) var saveError: String?
    @ObservationIgnored private let core: Core

    init(core: Core) {
        self.core = core
        config = core.windowDragConfig()
    }

    /// Kept in the order macOS displays modifiers, so the summary reads ⌃⌥⇧⌘.
    func setModifier(_ modifier: Modifier, held: Bool) {
        var modifiers = Set(config.modifiers)
        if held { modifiers.insert(modifier) } else { modifiers.remove(modifier) }
        config.modifiers = Modifier.allCases.filter(modifiers.contains)
    }

    private func save() {
        do {
            try core.setWindowDragConfig(windowDrag: config)
            saveError = nil
        } catch {
            saveError = error.userMessage
        }
    }
}

struct WindowDragPane: View {
    @Bindable var settings: WindowDragSettings
    let isInputRunning: Bool

    private var modifiers: String {
        settings.config.modifiers.map(\.symbol).joined()
    }

    var body: some View {
        Form {
            PaneHeaderSection(pane: .windowDrag)

            if let error = settings.saveError {
                Section {
                    Label(error, systemImage: "exclamationmark.triangle.fill")
                        .foregroundStyle(.red)
                }
            }

            Section {
                LabeledContent {
                    HStack(spacing: 6) {
                        ForEach(Modifier.allCases) { modifier in
                            Toggle(isOn: Binding(
                                get: { settings.config.modifiers.contains(modifier) },
                                set: { settings.setModifier(modifier, held: $0) }
                            )) {
                                Text(modifier.symbol)
                                    .frame(minWidth: 18)
                            }
                            .toggleStyle(.button)
                            .help(modifier.title)
                        }
                    }
                } label: {
                    Text("Modifiers")
                    Text("Hold exactly these while pressing a mouse button.")
                }

                Toggle(isOn: $settings.config.swapButtons) {
                    Text("Swap buttons")
                    Text("Drag with the left button to resize and the right button to move.")
                }
            } footer: {
                footer
            }
        }
        .formStyle(.grouped)
    }

    @ViewBuilder
    private var footer: some View {
        if settings.config.modifiers.isEmpty {
            Label("Choose at least one modifier.", systemImage: "exclamationmark.triangle.fill")
                .foregroundStyle(.red)
        } else if settings.config.enabled && !isInputRunning {
            Text("Window move and resize works once Accessibility is granted.")
                .foregroundStyle(.red)
        } else {
            VStack(alignment: .leading, spacing: 6) {
                let (move, resize) = settings.config.swapButtons ? ("right", "left") : ("left", "right")
                Text("\(modifiers) + \(move) drag moves the window under the cursor; \(modifiers) + \(resize) drag resizes it from its bottom-right corner.")
                if settings.config.modifiers.count == 1 {
                    Label("Apps use \(modifiers)-click too, so those clicks will move or resize windows instead. Two or more modifiers are safer.",
                          systemImage: "exclamationmark.triangle.fill")
                        .foregroundStyle(.yellow)
                }
            }
        }
    }
}
