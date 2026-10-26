import SwiftUI

struct StatusMenu: View {
    let permission: AccessibilityPermission
    let onShowOnboarding: () -> Void

    private let core = coreInfo()

    var body: some View {
        Text("Jaycast \(core.version)")

        if permission.isGranted {
            Text("Accessibility: granted")
        } else {
            Button("Grant Accessibility…", action: onShowOnboarding)
        }

        Divider()

        Button("Quit Jaycast") { NSApp.terminate(nil) }
            .keyboardShortcut("q")
    }
}
