import SwiftUI

struct MonitorPane: View {
    @Bindable var settings: DisplaySettings

    var body: some View {
        Form {
            PaneHeaderSection(pane: .monitor)

            if let error = settings.saveError {
                Section {
                    Label(error, systemImage: "exclamationmark.triangle.fill")
                        .foregroundStyle(.red)
                }
            }

            Section {
                Toggle(isOn: $settings.config.brightnessKeys) {
                    Text("Brightness keys control the display under the cursor")
                    Text("The built-in display, and displays without DDC/CI, keep the keys' normal behaviour.")
                }
                .switchedOff(!settings.config.enabled)
            } footer: {
                Text("Brightness is set over DDC/CI, which most external monitors support.")
            }

            Section("Displays") {
                // macOS controls the built-in display itself, so it doesn't depend on the toggle.
                if let brightness = settings.builtinBrightness {
                    BrightnessSliderRow(
                        name: "Built-in Display",
                        brightness: brightness,
                        onChange: { settings.setBuiltinBrightness($0) }
                    )
                }

                Group {
                    if settings.displays.isEmpty {
                        Text("No external displays")
                            .foregroundStyle(.secondary)
                    } else {
                        ForEach(settings.displays, id: \.id) { display in
                            DisplayBrightnessRow(display: display, settings: settings)
                        }
                    }
                }
                .switchedOff(!settings.config.enabled)

                if let error = settings.brightnessError {
                    Label(error, systemImage: "exclamationmark.triangle.fill")
                        .foregroundStyle(.red)
                }
            }
        }
        .formStyle(.grouped)
        .task {
            // Follow changes macOS makes to the built-in display while the pane is open.
            while !Task.isCancelled {
                settings.refreshBuiltin()
                try? await Task.sleep(for: .seconds(1))
            }
        }
    }
}
