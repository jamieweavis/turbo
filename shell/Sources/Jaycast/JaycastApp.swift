import AppKit
import SwiftUI

@main
struct JaycastApp: App {
    @NSApplicationDelegateAdaptor(AppDelegate.self) private var appDelegate

    var body: some Scene {
        MenuBarExtra {
            if let model = appDelegate.model {
                StatusMenu(
                    model: model,
                    onShowOnboarding: { appDelegate.showOnboarding() },
                    onShowSettings: { appDelegate.showSettings() }
                )
            } else {
                VStack(alignment: .leading) {
                    Text("Jaycast couldn't start")
                    Button("Quit Jaycast") { NSApp.terminate(nil) }
                }
                .padding(14)
            }
        } label: {
            Image(nsImage: MenuBarIcon.multitool)
                .accessibilityLabel("Jaycast")
        }
        .menuBarExtraStyle(.window)
    }
}

@MainActor
final class AppDelegate: NSObject, NSApplicationDelegate {
    /// Nil only if the core failed to start, in which case the app explains and quits.
    let model: AppModel?
    private let startupError: (any Error)?
    private var onboarding: OnboardingWindowController?
    private var settings: SettingsWindowController?

    override init() {
        do {
            model = try AppModel()
            startupError = nil
        } catch {
            model = nil
            startupError = error
        }
        super.init()
    }

    func applicationDidFinishLaunching(_ notification: Notification) {
        guard let model else {
            let alert = NSAlert()
            alert.messageText = "Jaycast couldn't start"
            alert.informativeText = startupError?.userMessage ?? ""
            alert.alertStyle = .critical
            NSApp.activate()
            alert.runModal()
            NSApp.terminate(nil)
            return
        }

        model.palette.onShowSettings = { [weak self] in self?.showSettings() }
        model.start()
        if !model.accessibility.isGranted {
            showOnboarding()
        }
    }

    func applicationWillTerminate(_ notification: Notification) {
        model?.stopInput()
    }

    func showOnboarding() {
        guard let model else { return }
        let onboarding = self.onboarding ?? OnboardingWindowController(permission: model.accessibility)
        self.onboarding = onboarding
        onboarding.show()
    }

    func showSettings() {
        guard let model else { return }
        let settings = self.settings ?? SettingsWindowController(model: model) { [weak self] in
            self?.showOnboarding()
        }
        self.settings = settings
        settings.show()
    }
}
