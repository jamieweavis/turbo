import AppKit
import SwiftUI

/// A plain AppKit window rather than a SwiftUI `Window` scene: scenes are unreliable
/// for menu-bar-only (accessory) apps, which have no Dock icon to reopen them from.
@MainActor
final class OnboardingWindowController: NSObject {
    private let permission: AccessibilityPermission
    private var window: NSWindow?

    init(permission: AccessibilityPermission) {
        self.permission = permission
    }

    func show() {
        let window = self.window ?? makeWindow()
        self.window = window
        RegularAppWindows.show(window)
    }

    private func makeWindow() -> NSWindow {
        let view = OnboardingView(permission: permission) { [weak self] in
            self?.window?.close()
        }
        let window = NSWindow(contentViewController: NSHostingController(rootView: view))
        window.title = "Welcome to Backpack"
        window.styleMask = [.titled, .closable]
        window.isReleasedWhenClosed = false
        window.center()
        return window
    }
}

private struct OnboardingView: View {
    let permission: AccessibilityPermission
    let onDone: () -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            Label("Accessibility access", systemImage: "hand.raised")
                .font(.title2.bold())

            Text("Backpack needs Accessibility access to remap mouse buttons, smooth scrolling, handle brightness keys, manage windows and expand snippets.")
                .fixedSize(horizontal: false, vertical: true)

            HStack {
                Image(systemName: permission.isGranted ? "checkmark.circle.fill" : "xmark.circle")
                    .foregroundStyle(permission.isGranted ? .green : .secondary)
                Text(permission.isGranted ? "Access granted" : "Not granted yet")
            }

            HStack {
                if permission.isGranted {
                    Spacer()
                    Button("Done", action: onDone)
                        .keyboardShortcut(.defaultAction)
                } else {
                    Button("Open System Settings") { permission.openSystemSettings() }
                    Spacer()
                    Button("Grant Access") { permission.request() }
                        .keyboardShortcut(.defaultAction)
                }
            }
        }
        .padding(24)
        .frame(width: 420)
    }
}
