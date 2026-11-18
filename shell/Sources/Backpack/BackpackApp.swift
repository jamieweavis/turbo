import AppKit
import SwiftUI
import os

@main
struct BackpackApp: App {
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
                    Text("Backpack couldn't start")
                    Button("Quit Backpack") { NSApp.terminate(nil) }
                }
                .padding(14)
            }
        } label: {
            Image(nsImage: MenuBarIcon.backpack)
                .accessibilityLabel("Backpack")
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
            Logger.app.fault("couldn't start the core: \(error, privacy: .public)")
            model = nil
            startupError = error
        }
        super.init()
    }

    func applicationDidFinishLaunching(_ notification: Notification) {
        guard let model else {
            let alert = NSAlert()
            alert.messageText = "Backpack couldn't start"
            alert.informativeText = startupError?.userMessage ?? ""
            alert.alertStyle = .critical
            NSApp.activate()
            alert.runModal()
            NSApp.terminate(nil)
            return
        }

        Logger.app.info("launched; Accessibility \(model.accessibility.isGranted ? "granted" : "not granted", privacy: .public)")
        model.palette.onShowSettings = { [weak self] in self?.showSettings() }
        model.start()
        if let directory = ScreenshotMode.directory {
            NSApp.appearance = NSAppearance(named: .darkAqua)
            showSettings()
            guard let settings else { return }
            Task { await ScreenshotMode.capture(settings, to: directory) }
        } else if !model.accessibility.isGranted {
            showOnboarding()
        }
    }

    func applicationWillTerminate(_ notification: Notification) {
        model?.stopInput()
        // Settings are written once changes settle; write any still waiting. There's no
        // one left to tell if it fails, so it's only logged.
        do {
            try model?.core.flushConfig()
        } catch {
            Logger.app.error("couldn't write settings on quit: \(error, privacy: .public)")
        }
        Logger.app.info("quitting")
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
