import SwiftUI

struct MonitorPane: View {
    @Bindable var settings: DisplaySettings

    var body: some View {
        Form {
            if let error = settings.saveError {
                Section {
                    Label(error, systemImage: "exclamationmark.triangle.fill")
                        .foregroundStyle(.red)
                }
            }

            Section {
                Toggle("Enable external display brightness", isOn: $settings.config.enabled)
                Toggle(isOn: $settings.config.brightnessKeys) {
                    Text("Brightness keys control the display under the cursor")
                    Text("The built-in display, and displays without DDC/CI, keep the keys' normal behaviour.")
                }
                .disabled(!settings.config.enabled)
            } footer: {
                Text("Brightness is set over DDC/CI, which most external monitors support.")
            }

            Section("Displays") {
                if settings.displays.isEmpty {
                    Text("No external displays")
                        .foregroundStyle(.secondary)
                } else {
                    ForEach(settings.displays, id: \.id) { display in
                        DisplayBrightnessRow(display: display, settings: settings)
                    }
                }

                if let error = settings.brightnessError {
                    Label(error, systemImage: "exclamationmark.triangle.fill")
                        .foregroundStyle(.red)
                }
            }
            .disabled(!settings.config.enabled)
        }
        .formStyle(.grouped)
    }
}
