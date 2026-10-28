import Observation
import SwiftUI
import os

/// Whether the core is keeping the Mac awake. Hotkeys change it behind the UI's back, so
/// the core is the source of truth and this is refreshed when they fire.
@MainActor
@Observable
final class KeepAwakeState {
    private(set) var isActive = false
    private(set) var error: String?
    @ObservationIgnored private let core: Core

    init(core: Core) {
        self.core = core
        refresh()
    }

    func setActive(_ active: Bool) {
        do {
            try core.setKeepAwakeActive(active: active)
            error = nil
        } catch {
            Logger.keepAwake.error("couldn't turn Keep Awake \(active ? "on" : "off"): \(error, privacy: .public)")
            self.error = error.userMessage
        }
        refresh()
    }

    func refresh() {
        isActive = core.keepAwakeActive()
    }
}

struct KeepAwakePane: View {
    let state: KeepAwakeState
    let settings: LauncherSettings

    var body: some View {
        @Bindable var settings = settings

        Form {
            PaneHeaderSection(pane: .keepAwake)

            if let error = state.error ?? settings.saveError {
                Section {
                    Label(error, systemImage: "exclamationmark.triangle.fill")
                        .foregroundStyle(.red)
                }
            }

            Section {
                Toggle(isOn: Binding(get: { state.isActive }, set: { state.setActive($0) })) {
                    Text("Keep Mac awake")
                    Text("Stops the Mac sleeping while idle. Stays on when Jaycast relaunches.")
                }
                Toggle(isOn: $settings.config.keepAwake.keepDisplayOn) {
                    Text("Keep display on")
                    Text("Stops the displays sleeping too while the Mac is kept awake.")
                }
            }

            Section("Hotkeys") {
                row("Toggle", hotkey: $settings.config.keepAwake.toggleHotkey, slot: .toggle)
                row("Turn on", hotkey: $settings.config.keepAwake.onHotkey, slot: .on)
                row("Turn off", hotkey: $settings.config.keepAwake.offHotkey, slot: .off)
            }
        }
        .formStyle(.grouped)
    }

    private func row(_ title: String, hotkey: Binding<Hotkey?>, slot: KeepAwakeHotkey) -> some View {
        LabeledContent(title) {
            HStack(spacing: 6) {
                if let current = hotkey.wrappedValue,
                   let problem = settings.problem(with: current, in: .keepAwake(slot)) {
                    Image(systemName: "exclamationmark.triangle.fill")
                        .foregroundStyle(.yellow)
                        .help(problem)
                }
                HotkeyField(hotkey: hotkey)
            }
        }
    }
}
