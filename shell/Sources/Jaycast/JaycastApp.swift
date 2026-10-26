import AppKit
import SwiftUI

@main
struct JaycastApp: App {
    @NSApplicationDelegateAdaptor(AppDelegate.self) private var appDelegate

    var body: some Scene {
        MenuBarExtra("Jaycast", systemImage: "sparkle.magnifyingglass") {
            StatusMenu(
                permission: appDelegate.accessibility,
                onShowOnboarding: { appDelegate.showOnboarding() }
            )
        }
    }
}

@MainActor
final class AppDelegate: NSObject, NSApplicationDelegate {
    let accessibility = AccessibilityPermission()
    private lazy var onboarding = OnboardingWindowController(permission: accessibility)

    func applicationDidFinishLaunching(_ notification: Notification) {
        accessibility.refresh()
        if !accessibility.isGranted {
            showOnboarding()
        }
    }

    func showOnboarding() {
        onboarding.show()
    }
}
