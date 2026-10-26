import Observation
import SwiftUI

/// Whether the core is keeping the Mac awake. Hotkeys change it behind the UI's back, so
/// the core is the source of truth and this is refreshed when they fire.
@MainActor
@Observable
final class CaffeinateState {
    private(set) var isActive = false
    private(set) var error: String?
    @ObservationIgnored private let core: Core

    init(core: Core) {
        self.core = core
        refresh()
    }

    func setActive(_ active: Bool) {
        do {
            try core.setCaffeinateActive(active: active)
            error = nil
        } catch {
            self.error = error.userMessage
        }
        refresh()
    }

    func refresh() {
        isActive = core.caffeinateActive()
    }
}

struct CaffeinatePane: View {
    let state: CaffeinateState
    let settings: LauncherSettings

    var body: some View {
        @Bindable var settings = settings

        Form {
            if let error = state.error ?? settings.saveError {
                Section {
                    Label(error, systemImage: "exclamationmark.triangle.fill")
                        .foregroundStyle(.red)
                }
            }

            Section {
                Toggle("Keep Mac awake", isOn: Binding(get: { state.isActive }, set: { state.setActive($0) }))
                Toggle("Keep display on", isOn: $settings.config.caffeinate.keepDisplayOn)
            } footer: {
                Text(settings.config.caffeinate.keepDisplayOn
                    ? "Stops the Mac and its displays sleeping while idle. Turns off when Jaycast quits."
                    : "Stops the Mac sleeping while idle; displays can still sleep. Turns off when Jaycast quits.")
            }

            Section("Hotkeys") {
                row("Toggle", hotkey: $settings.config.caffeinate.toggleHotkey, slot: .toggle)
                row("Turn on", hotkey: $settings.config.caffeinate.onHotkey, slot: .on)
                row("Turn off", hotkey: $settings.config.caffeinate.offHotkey, slot: .off)
            }
        }
        .formStyle(.grouped)
    }

    private func row(_ title: String, hotkey: Binding<Hotkey?>, slot: CaffeinateHotkey) -> some View {
        LabeledContent(title) {
            HStack(spacing: 6) {
                if let current = hotkey.wrappedValue,
                   let problem = settings.problem(with: current, in: .caffeinate(slot)) {
                    Image(systemName: "exclamationmark.triangle.fill")
                        .foregroundStyle(.yellow)
                        .help(problem)
                }
                HotkeyField(hotkey: hotkey)
            }
        }
    }
}
