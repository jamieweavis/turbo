import AppKit
import SwiftUI

/// A plain AppKit window rather than a SwiftUI `Settings` scene, which is unreliable for
/// menu-bar-only (accessory) apps.
@MainActor
final class SettingsWindowController {
    private let model: AppModel
    private let onShowOnboarding: () -> Void
    private var window: NSWindow?

    init(model: AppModel, onShowOnboarding: @escaping () -> Void) {
        self.model = model
        self.onShowOnboarding = onShowOnboarding
    }

    func show() {
        model.accessibility.refresh()
        let window = self.window ?? makeWindow()
        self.window = window
        NSApp.activate()
        window.makeKeyAndOrderFront(nil)
    }

    private func makeWindow() -> NSWindow {
        let view = SettingsView(model: model, onShowOnboarding: onShowOnboarding)
        let window = NSWindow(contentViewController: NSHostingController(rootView: view))
        window.title = "Jaycast Settings"
        window.styleMask = [.titled, .closable, .miniaturizable, .resizable, .fullSizeContentView]
        window.toolbarStyle = .unified
        window.isReleasedWhenClosed = false
        window.setContentSize(NSSize(width: 720, height: 560))
        window.center()
        window.setFrameAutosaveName("Settings")
        return window
    }
}

enum SettingsPane: CaseIterable, Identifiable {
    case general, launcher, mouse, monitor

    var id: Self { self }

    var title: String {
        switch self {
        case .general: "General"
        case .launcher: "Launcher"
        case .mouse: "Mouse"
        case .monitor: "Monitor"
        }
    }

    var systemImage: String {
        switch self {
        case .general: "gearshape"
        case .launcher: "magnifyingglass"
        case .mouse: "computermouse"
        case .monitor: "display"
        }
    }
}

struct SettingsView: View {
    let model: AppModel
    let onShowOnboarding: () -> Void

    @State private var selection: SettingsPane? = .general

    var body: some View {
        NavigationSplitView {
            List(SettingsPane.allCases, selection: $selection) { pane in
                Label(pane.title, systemImage: pane.systemImage)
            }
            .navigationSplitViewColumnWidth(180)
            .toolbar(removing: .sidebarToggle)
        } detail: {
            let pane = selection ?? .general
            Group {
                switch pane {
                case .general: GeneralPane(model: model, onShowOnboarding: onShowOnboarding)
                case .launcher: LauncherPane(settings: model.launcher)
                case .mouse: MousePane(settings: model.mouse)
                case .monitor: MonitorPane(settings: model.displays)
                }
            }
            .navigationTitle(pane.title)
        }
        .frame(minWidth: 680, minHeight: 460)
    }
}

private struct GeneralPane: View {
    let model: AppModel
    let onShowOnboarding: () -> Void

    var body: some View {
        Form {
            Section("Permissions") {
                LabeledContent("Accessibility") {
                    if model.accessibility.isGranted {
                        Label("Granted", systemImage: "checkmark.circle.fill")
                            .foregroundStyle(.green)
                    } else {
                        Button("Grant Access…", action: onShowOnboarding)
                    }
                }
                LabeledContent("Input") {
                    if let error = model.inputError {
                        Text(error).foregroundStyle(.red)
                    } else {
                        Text(model.isInputRunning ? "Active" : "Inactive")
                    }
                }
            }

            Section("About") {
                LabeledContent("Version", value: coreInfo().version)
            }
        }
        .formStyle(.grouped)
    }
}
