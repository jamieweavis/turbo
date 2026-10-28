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
        RegularAppWindows.show(window)
    }

    private func makeWindow() -> NSWindow {
        let view = SettingsView(model: model, onShowOnboarding: onShowOnboarding)
        let hosting = NSHostingController(rootView: view)
        // Only the minimum size: following the panes' ideal sizes would resize the window
        // whenever one is long (the Launcher pane's table), pushing content under the title bar.
        hosting.sizingOptions = [.minSize]
        let window = NSWindow(contentViewController: hosting)
        // Clear window so the SwiftUI glass background shows the desktop through it.
        window.isOpaque = false
        window.backgroundColor = .clear
        // Still the window's name in the Window menu and Mission Control; each pane's header
        // shows its own title instead.
        window.title = "Jaycast Settings"
        window.titleVisibility = .hidden
        // No toolbar band behind the pane switch: it floats at the top right over the pane.
        window.titlebarAppearsTransparent = true
        window.titlebarSeparatorStyle = .none
        window.styleMask = [.titled, .closable, .miniaturizable, .resizable, .fullSizeContentView]
        window.toolbarStyle = .unified
        window.isReleasedWhenClosed = false
        window.setContentSize(NSSize(width: 720, height: 560))
        window.center()
        window.setFrameAutosaveName("Settings")
        return window
    }
}

/// A sidebar group of panes, in sidebar order.
enum SettingsSection: CaseIterable, Identifiable {
    case main, launcher, utilities, input

    var id: Self { self }

    /// The sidebar subheading; `nil` for the untitled group at the top.
    var title: String? {
        switch self {
        case .main: nil
        case .launcher: "Launcher"
        case .utilities: "Utilities"
        case .input: "Input"
        }
    }

    var panes: [SettingsPane] {
        SettingsPane.allCases.filter { $0.section == self }
    }
}

enum SettingsPane: CaseIterable, Identifiable {
    case general, launcher, quicklinks, commands, files, snippets, clipboard, windows, windowDrag, keyboard, mouse, monitor, keepAwake, colorPicker

    var id: Self { self }

    var section: SettingsSection {
        switch self {
        case .general: .main
        case .launcher, .quicklinks, .commands, .files, .snippets, .clipboard, .windows: .launcher
        case .keyboard, .mouse: .input
        case .windowDrag, .monitor, .keepAwake, .colorPicker: .utilities
        }
    }

    var title: String {
        switch self {
        case .general: "General"
        case .launcher: "Launcher"
        case .quicklinks: "Quicklinks"
        case .commands: "Shell Commands"
        case .snippets: "Snippets"
        case .clipboard: "Clipboard"
        case .files: "File Search"
        case .windows: "Window Management"
        case .windowDrag: "Window Transform"
        case .keyboard: "Hyper Key"
        case .mouse: "Mouse"
        case .monitor: "Brightness"
        case .keepAwake: "Keep Awake"
        case .colorPicker: "Colour Picker"
        }
    }

    /// Under the title in the pane's header.
    var subtitle: String {
        switch self {
        case .general: "Startup, permissions and version."
        case .launcher: "Search apps, commands and System Settings, with hotkeys and aliases."
        case .quicklinks: "Open websites and searches by keyword."
        case .commands: "Run shell commands from the launcher."
        case .snippets: "Insert saved text by keyword, anywhere you type."
        case .clipboard: "Search and paste everything you've copied."
        case .files: "Search for files and folders on your computer."
        case .windows: "Move and resize windows with hotkeys."
        case .windowDrag: "Move and resize a window by dragging anywhere in it with modifiers held."
        case .keyboard: "Turn one key into ⌃⌥⇧⌘ for shortcuts that never clash."
        case .mouse: "Smooth scrolling, scroll modifiers and button remapping."
        case .monitor: "Adjust your displays' brightness, external monitors included."
        case .keepAwake: "Stop your Mac sleeping while you need it."
        case .colorPicker: "Pick a colour from anywhere on screen."
        }
    }

    /// The header icon's background.
    var tint: Color {
        switch self {
        case .general: .gray
        case .launcher: .blue
        case .quicklinks: .teal
        case .commands: .green
        case .snippets: .orange
        case .clipboard: .indigo
        case .files: .blue
        case .windows: .cyan
        case .windowDrag: .mint
        case .keyboard: .purple
        case .mouse: .pink
        case .monitor: .yellow
        case .keepAwake: .brown
        case .colorPicker: .red
        }
    }

    /// The launcher feature the pane's switch turns on and off, if it has one.
    var feature: LauncherFeature? {
        switch self {
        case .quicklinks: .quicklinks
        case .commands: .commands
        case .snippets: .snippets
        case .clipboard: .clipboard
        case .files: .files
        case .windows: .windows
        case .keepAwake: .keepAwake
        case .colorPicker: .colorPicker
        case .general, .launcher, .windowDrag, .keyboard, .mouse, .monitor: nil
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
        case .windows: "rectangle.lefthalf.filled"
        case .windowDrag: "arrow.up.and.down.and.arrow.left.and.right"
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
    @State private var columnVisibility = NavigationSplitViewVisibility.all

    var body: some View {
        // Dragging the divider or View > Hide Sidebar still collapse the sidebar without the
        // toggle button, so snap it back whenever that happens.
        NavigationSplitView(columnVisibility: $columnVisibility) {
            List(selection: $selection) {
                ForEach(SettingsSection.allCases) { section in
                    if let title = section.title {
                        Section(title) { rows(section) }
                    } else {
                        Section { rows(section) }
                    }
                }
            }
            // The column width alone is ignored here; the frame is what actually widens the sidebar,
            // so the selected (bold) "Window Management" fits.
            .frame(minWidth: 220)
            .navigationSplitViewColumnWidth(min: 220, ideal: 220, max: 220)
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
                case .windowDrag: WindowDragPane(settings: model.windowDrag, isInputRunning: model.isInputRunning)
                case .keyboard: KeyboardPane(settings: model.hyperKey, isInputRunning: model.isInputRunning)
                case .mouse: MousePane(settings: model.mouse)
                case .monitor: MonitorPane(settings: model.displays)
                case .keepAwake: KeepAwakePane(state: model.keepAwake, settings: model.launcher)
                case .colorPicker: ColorPickerPane(picker: model.colorPicker, settings: model.launcher)
                }
            }
            // Monitor keeps the built-in display's slider usable, and disables the rest itself.
            .disabled(pane != .monitor && isEnabled(pane)?.wrappedValue == false)
            .scrollContentBackground(.hidden)
            // Every pane has a toolbar item, even an invisible one: without one the toolbar
            // collapses and the traffic lights lose their inset position.
            .toolbar {
                ToolbarSpacer(.flexible)
                if isEnabled(pane) != nil {
                    ToolbarItem { FeatureSwitch(pane: pane, model: model) }
                        .sharedBackgroundVisibility(.hidden)
                } else {
                    ToolbarItem {
                        Color.clear.frame(width: 1, height: 1)
                    }
                    .sharedBackgroundVisibility(.hidden)
                }
            }
        }
        .frame(minWidth: 680, minHeight: 460)
        .onChange(of: columnVisibility) {
            if columnVisibility != .all { columnVisibility = .all }
        }
        // Turning a feature off from its own pane, or ticking it back on in the Launcher pane's
        // table, can change whether the Mac is kept awake.
        .onChange(of: model.launcher.config.keepAwake.enabled) { model.keepAwake.refresh() }
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

extension SettingsView {
    private func rows(_ section: SettingsSection) -> some View {
        ForEach(section.panes) { pane in
            Label {
                HStack {
                    Text(pane.title)
                    Spacer()
                    if isEnabled(pane)?.wrappedValue == false {
                        Text("Off")
                            .foregroundStyle(.secondary)
                    }
                }
            } icon: {
                PaneIcon(pane: pane, size: 20)
            }
        }
    }

    /// The pane's on/off switch: its launcher feature's, or the window drag, Hyper key, mouse
    /// or display brightness master switch. Nil for panes that can't be turned off.
    private func isEnabled(_ pane: SettingsPane) -> Binding<Bool>? {
        Self.isEnabled(pane, model: model)
    }

    fileprivate static func isEnabled(_ pane: SettingsPane, model: AppModel) -> Binding<Bool>? {
        if let feature = pane.feature {
            let launcher = model.launcher
            return Binding(get: { launcher.isEnabled(feature) }, set: { launcher.setEnabled($0, for: feature) })
        }
        switch pane {
        case .windowDrag:
            let windowDrag = model.windowDrag
            return Binding(get: { windowDrag.config.enabled }, set: { windowDrag.config.enabled = $0 })
        case .keyboard:
            let hyperKey = model.hyperKey
            return Binding(get: { hyperKey.config.enabled }, set: { hyperKey.config.enabled = $0 })
        case .mouse:
            let mouse = model.mouse
            return Binding(get: { mouse.config.enabled }, set: { mouse.config.enabled = $0 })
        case .monitor:
            let displays = model.displays
            return Binding(get: { displays.config.enabled }, set: { displays.config.enabled = $0 })
        default:
            return nil
        }
    }
}

/// The pane's on/off switch in the toolbar.
private struct FeatureSwitch: View {
    let pane: SettingsPane
    let model: AppModel

    /// Whether the switch has had its first update.
    @State private var updated = false

    var body: some View {
        if let isOn = SettingsView.isEnabled(pane, model: model) {
            Toggle("Enable \(pane.title)", isOn: isOn)
                .toggleStyle(.switch)
                .controlSize(.mini)
                .labelsHidden()
                // The toolbar pads the item 9pt more each side when it first lays it out than
                // when it updates it, so the switch drifted right on its first flip. Updating
                // straight away with that padding added keeps it in the same place for good.
                .padding(.horizontal, updated ? 9 : 0)
                .help(isOn.wrappedValue ? "Turn \(pane.title) off" : "Turn \(pane.title) on")
                .task { updated = true }
        }
    }
}

/// The pane's white symbol on a rounded square of its colour, in the header and the sidebar.
struct PaneIcon: View {
    let pane: SettingsPane
    let size: CGFloat

    var body: some View {
        Group {
            if pane == .windows {
                // The same drawn screen as the Left Half action, rather than a symbol.
                WindowActionIcon(action: .leftHalf, width: size * 0.6)
            } else {
                Image(systemName: pane.systemImage)
                    .font(.system(size: size * 0.5, weight: .medium))
            }
        }
        .foregroundStyle(.white)
        .frame(width: size, height: size)
        .background(pane.tint.gradient, in: .rect(cornerRadius: size * 0.23))
    }
}

/// The pane's icon, title and description, at the top of its content.
struct PaneHeader: View {
    let pane: SettingsPane

    var body: some View {
        VStack(spacing: 8) {
            PaneIcon(pane: pane, size: 56)
                .padding(.bottom, 4)
            Text(pane.title)
                .font(.largeTitle.bold())
            Text(pane.subtitle)
                .font(.title3)
                .foregroundStyle(.secondary)
                .multilineTextAlignment(.center)
        }
        .frame(maxWidth: .infinity)
        // Pulled up into the grouped form's top margin, which sits below the toolbar's safe area.
        .padding(.top, -24)
        .padding(.bottom, 12)
    }
}

/// The header as a Form's first section, so it scrolls with the pane.
struct PaneHeaderSection: View {
    let pane: SettingsPane

    var body: some View {
        Section {} header: { PaneHeader(pane: pane) }
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
            PaneHeaderSection(pane: .general)

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
