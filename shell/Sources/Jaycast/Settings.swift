import AppKit
import ServiceManagement
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
        if window.isMiniaturized { window.deminiaturize(nil) }
        // Activation is only a request: when another app is frontmost (as it is behind the
        // non-activating palette) the system may refuse it, so raise the window regardless.
        NSApp.activate()
        window.orderFrontRegardless()
        window.makeKeyAndOrderFront(nil)
    }

    private func makeWindow() -> NSWindow {
        let view = SettingsView(model: model, onShowOnboarding: onShowOnboarding)
        let window = NSWindow(contentViewController: NSHostingController(rootView: view))
        // Clear window so the SwiftUI glass background shows the desktop through it.
        window.isOpaque = false
        window.backgroundColor = .clear
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
    case general, launcher, quicklinks, commands, snippets, clipboard, files, windows, keyboard, mouse, monitor, keepAwake, colorPicker

    var id: Self { self }

    var title: String {
        switch self {
        case .general: "General"
        case .launcher: "Launcher"
        case .quicklinks: "Quicklinks"
        case .commands: "Commands"
        case .snippets: "Snippets"
        case .clipboard: "Clipboard"
        case .files: "File Search"
        case .windows: "Windows"
        case .keyboard: "Hyper Key"
        case .mouse: "Mouse"
        case .monitor: "Brightness"
        case .keepAwake: "Keep Awake"
        case .colorPicker: "Colour Picker"
        }
    }

    var systemImage: String {
        switch self {
        case .general: "gearshape"
        case .launcher: "magnifyingglass"
        case .quicklinks: "link"
        case .commands: "terminal"
        case .snippets: "text.quote"
        case .clipboard: "doc.on.clipboard"
        case .files: "doc.text.magnifyingglass"
        case .windows: "macwindow"
        case .keyboard: "command.square"
        case .mouse: "computermouse"
        case .monitor: "sun.max"
        case .keepAwake: "eye"
        case .colorPicker: "eyedropper"
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
            // The column width alone is ignored here; the frame is what actually widens the sidebar,
            // so the selected (bold) "Colour Picker" fits.
            .frame(minWidth: 165)
            .navigationSplitViewColumnWidth(min: 165, ideal: 165, max: 165)
            .toolbar(removing: .sidebarToggle)
        } detail: {
            let pane = selection ?? .general
            Group {
                switch pane {
                case .general: GeneralPane(model: model, onShowOnboarding: onShowOnboarding)
                case .launcher: LauncherPane(settings: model.launcher)
                case .quicklinks: QuicklinksPane(settings: model.launcher)
                case .commands: CommandsPane(settings: model.launcher)
                case .snippets: SnippetsPane(settings: model.launcher)
                case .clipboard: ClipboardPane(settings: model.launcher)
                case .files: FileSearchPane(settings: model.launcher)
                case .windows: WindowsPane(settings: model.launcher)
                case .keyboard: KeyboardPane(settings: model.hyperKey, isInputRunning: model.isInputRunning)
                case .mouse: MousePane(settings: model.mouse)
                case .monitor: MonitorPane(settings: model.displays)
                case .keepAwake: KeepAwakePane(state: model.keepAwake, settings: model.launcher)
                case .colorPicker: ColorPickerPane(picker: model.colorPicker, settings: model.launcher)
                }
            }
            .navigationTitle("Jaycast Settings")
            .scrollContentBackground(.hidden)
        }
        .frame(minWidth: 680, minHeight: 460)
        .containerBackground(.clear, for: .window)
        // A background layer, so the split view keeps its normal title bar layout. Tinted with
        // the window colour to tone down how much the desktop shows through.
        .background {
            Rectangle()
                .fill(.clear)
                .glassEffect(.regular.tint(Color(nsColor: .windowBackgroundColor).opacity(0.5)), in: .rect)
                .ignoresSafeArea()
        }
    }
}

private struct GeneralPane: View {
    let model: AppModel
    let onShowOnboarding: () -> Void

    // The login item registration itself is the source of truth, so nothing is persisted here.
    @State private var launchAtLogin = SMAppService.mainApp.status == .enabled
    @State private var launchAtLoginError: String?

    var body: some View {
        Form {
            Section("Startup") {
                Toggle("Launch at login", isOn: $launchAtLogin)
                    .onChange(of: launchAtLogin) { _, enabled in setLaunchAtLogin(enabled) }
                if let launchAtLoginError {
                    Text(launchAtLoginError).foregroundStyle(.red)
                }
            }

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

    private func setLaunchAtLogin(_ enabled: Bool) {
        let service = SMAppService.mainApp
        guard enabled != (service.status == .enabled) else { return }
        do {
            if enabled {
                try service.register()
            } else {
                try service.unregister()
            }
            launchAtLoginError = nil
        } catch {
            launchAtLoginError = error.localizedDescription
            launchAtLogin = service.status == .enabled
        }
    }
}
