import SwiftUI

/// The menu bar popover: display brightness on top, then the app's toggles and actions.
struct StatusMenu: View {
    let model: AppModel
    let onShowOnboarding: () -> Void
    let onShowSettings: () -> Void

    @Environment(\.dismiss) private var dismiss
    private let core = coreInfo()

    var body: some View {
        @Bindable var mouse = model.mouse

        VStack(alignment: .leading, spacing: 12) {
            if model.displays.config.enabled, !model.displays.displays.isEmpty {
                displays
                Divider()
            }

            Toggle(isOn: $mouse.config.enabled) {
                Label("Mouse fixes", systemImage: "computermouse")
                    .frame(maxWidth: .infinity, alignment: .leading)
            }
            .toggleStyle(.switch)

            if !model.accessibility.isGranted {
                MenuRow(title: "Grant Accessibility…", systemImage: "hand.raised") {
                    dismiss()
                    onShowOnboarding()
                }
            }

            if let error = model.inputError {
                Label("Input unavailable: \(error)", systemImage: "exclamationmark.triangle.fill")
                    .font(.caption)
                    .foregroundStyle(.red)
            }

            Divider()

            HStack {
                Button("Settings…") {
                    dismiss()
                    onShowSettings()
                }
                .keyboardShortcut(",")

                Spacer()

                Text("Jaycast \(core.version)")
                    .font(.caption)
                    .foregroundStyle(.tertiary)

                Spacer()

                Button("Quit") { NSApp.terminate(nil) }
                    .keyboardShortcut("q")
            }
            .controlSize(.small)
        }
        .padding(14)
        .frame(width: 300)
    }

    private var displays: some View {
        VStack(alignment: .leading, spacing: 10) {
            Text("Displays")
                .font(.subheadline.weight(.semibold))
                .foregroundStyle(.secondary)

            ForEach(model.displays.displays, id: \.id) { display in
                DisplayBrightnessRow(display: display, settings: model.displays)
            }

            if let error = model.displays.brightnessError {
                Text(error)
                    .font(.caption)
                    .foregroundStyle(.red)
            }
        }
    }
}

/// A display's name with a brightness slider, or a note when it can't be controlled.
/// Shared by the menu bar popover and the Monitor settings pane.
struct DisplayBrightnessRow: View {
    let display: DisplayInfo
    let settings: DisplaySettings

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            Text(display.name)
                .font(.callout)

            if display.supportsDdc {
                HStack(spacing: 8) {
                    Image(systemName: "sun.min")
                        .foregroundStyle(.secondary)
                    Slider(
                        value: Binding(
                            get: { display.brightness },
                            set: { settings.setBrightness($0, for: display.id) }
                        ),
                        in: 0...1
                    )
                    .labelsHidden()
                    .frame(maxWidth: .infinity)
                    .accessibilityLabel("\(display.name) brightness")
                    Image(systemName: "sun.max")
                        .foregroundStyle(.secondary)
                }
            } else {
                Text("Brightness control not supported")
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }
        }
    }
}

/// A full-width, borderless button row in the style of Control Center's menu items.
private struct MenuRow: View {
    let title: String
    let systemImage: String
    let action: () -> Void

    var body: some View {
        Button(action: action) {
            Label(title, systemImage: systemImage)
                .frame(maxWidth: .infinity, alignment: .leading)
                .contentShape(.rect)
        }
        .buttonStyle(.plain)
    }
}
