import SwiftUI

struct StatusMenu: View {
    let model: AppModel
    let onShowOnboarding: () -> Void
    let onShowSettings: () -> Void

    private let core = coreInfo()

    var body: some View {
        @Bindable var mouse = model.mouse

        Text("Jaycast \(core.version)")

        if model.accessibility.isGranted {
            Text("Accessibility: granted")
        } else {
            Button("Grant Accessibility…", action: onShowOnboarding)
        }

        if let error = model.inputError {
            Text("Mouse input unavailable: \(error)")
        }

        Divider()

        Toggle("Mouse fixes", isOn: $mouse.config.enabled)

        Divider()

        Button("Settings…", action: onShowSettings)
            .keyboardShortcut(",")

        Button("Quit Jaycast") { NSApp.terminate(nil) }
            .keyboardShortcut("q")
    }
}
