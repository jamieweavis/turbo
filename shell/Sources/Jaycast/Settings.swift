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
        case .keyboard: "diamond.fill"
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

    @State private var history = PaneHistory()
    @State private var titles = PaneTitleTracker()

    var body: some View {
        // A nil selection (a ⌘-click on the selected row) keeps the current pane.
        let selection = Binding<SettingsPane?>(
            get: { history.current },
            set: { if let pane = $0 { history.visit(pane) } }
        )
        NavigationSplitView {
            List(selection: selection) {
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
            .frame(minWidth: 230)
            .navigationSplitViewColumnWidth(min: 230, ideal: 230, max: 230)
            .toolbar(removing: .sidebarToggle)
            .background(SidebarLimits(width: 230))
        } detail: {
            let pane = history.current
            Group {
                switch pane {
                case .general:
                    GeneralPane(model: model, onShowOnboarding: onShowOnboarding) { history.visit($0) }
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
            .environment(titles)
            .toolbar {
                ToolbarItemGroup(placement: .navigation) {
                    Button("Back", systemImage: "chevron.left") { history.goBack() }
                        .disabled(history.previous == nil)
                        .help(history.previous.map { "Back to \($0.title)" } ?? "Back")
                        .keyboardShortcut("[", modifiers: .command)
                    Button("Forward", systemImage: "chevron.right") { history.goForward() }
                        .disabled(history.next == nil)
                        .help(history.next.map { "Forward to \($0.title)" } ?? "Forward")
                        .keyboardShortcut("]", modifiers: .command)
                }
                // Not on General: with no switch on the right, the title sits off-centre.
                if pane != .general {
                    ToolbarItem(placement: .principal) { ToolbarTitle(pane: pane, titles: titles) }
                        .sharedBackgroundVisibility(.hidden)
                }
                ToolbarSpacer(.flexible)
                if isEnabled(pane) != nil {
                    ToolbarItem { FeatureSwitch(pane: pane, model: model) }
                        .sharedBackgroundVisibility(.hidden)
                }
            }
        }
        .frame(minWidth: 680, minHeight: 460)
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

/// The panes visited, for the toolbar's back and forward buttons, like a browser's history.
struct PaneHistory {
    private var back: [SettingsPane] = []
    private(set) var current: SettingsPane = .general
    private var forward: [SettingsPane] = []

    var previous: SettingsPane? { back.last }
    var next: SettingsPane? { forward.last }

    /// Opens `pane`, dropping the forward history.
    mutating func visit(_ pane: SettingsPane) {
        guard pane != current else { return }
        back.append(current)
        current = pane
        forward.removeAll()
    }

    mutating func goBack() {
        guard let pane = back.popLast() else { return }
        forward.append(current)
        current = pane
    }

    mutating func goForward() {
        guard let pane = forward.popLast() else { return }
        back.append(current)
        current = pane
    }
}

/// Where each pane's header title is, so the toolbar can take over the title once it scrolls
/// away. Its own object so only the toolbar title redraws as the pane scrolls.
@Observable
final class PaneTitleTracker {
    var titleFrames: [SettingsPane: CGRect] = [:]

    /// Whether `pane`'s header title has scrolled off the top of the window. Not just under the
    /// toolbar, which it still shows through.
    func isTitleHidden(for pane: SettingsPane) -> Bool {
        guard let frame = titleFrames[pane] else { return false }
        return frame.maxY <= 0
    }
}

/// The pane's title in the middle of the toolbar, sliding up into place once its header's title
/// has scrolled out of view, so the two never show at once.
private struct ToolbarTitle: View {
    let pane: SettingsPane
    let titles: PaneTitleTracker

    private let height: CGFloat = 22

    var body: some View {
        let shown = titles.isTitleHidden(for: pane)
        Text(pane.title)
            .font(.headline)
            .offset(y: shown ? 0 : height)
            .opacity(shown ? 1 : 0)
            .animation(.easeOut(duration: 0.2), value: shown)
            .frame(height: height)
            .clipped()
            .allowsHitTesting(false)
            .accessibilityHidden(!shown)
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
                // The diamond fills more of its box than other symbols, which shows at sidebar size.
                let scale = pane == .keyboard && size <= 20 ? 0.35 : 0.5
                Image(systemName: pane.systemImage)
                    .font(.system(size: size * scale, weight: .medium))
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

    @Environment(PaneTitleTracker.self) private var titles: PaneTitleTracker?

    var body: some View {
        VStack(spacing: 8) {
            PaneIcon(pane: pane, size: 56)
                .padding(.bottom, 4)
            Text(pane.title)
                .font(.largeTitle.bold())
                .onGeometryChange(for: CGRect.self) { $0.frame(in: .global) } action: {
                    titles?.titleFrames[pane] = $0
                }
                // A form drops rows scrolled well out of view, by which point the title is under
                // the toolbar.
                .onDisappear { titles?.titleFrames[pane] = CGRect(x: 0, y: -.infinity, width: 0, height: 0) }
            Text(pane.subtitle)
                .font(.title3)
                .foregroundStyle(.secondary)
                .multilineTextAlignment(.center)
        }
        .frame(maxWidth: .infinity)
        // Pulled up into the grouped form's top margin, which sits below the toolbar's safe area,
        // but not so far that the toolbar's edge clips the icon.
        .padding(.top, -12)
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
    let onSelect: (SettingsPane) -> Void

    // The login item registration itself is the source of truth, so nothing is persisted here.
    @State private var launchAtLogin = SMAppService.mainApp.status == .enabled
    @State private var launchAtLoginError: String?

    var body: some View {
        Form {
            PaneHeaderSection(pane: .general)

            Section {
                Toggle("Launch at login", isOn: $launchAtLogin)
                    .onChange(of: launchAtLogin) { _, enabled in setLaunchAtLogin(enabled) }
                if let launchAtLoginError {
                    Text(launchAtLoginError).foregroundStyle(.red)
                }
            } header: {
                SectionHeading("Startup")
            }

            Section {
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
            } header: {
                SectionHeading("Permissions")
            }

            // In the header rather than the rows, so the cards sit on the window background
            // instead of inside a grouped section's box. Grouped as in the sidebar.
            ForEach(SettingsSection.allCases.filter { $0.title != nil }) { section in
                Section {} header: {
                    VStack(alignment: .leading, spacing: 10) {
                        SectionHeading(section.title ?? "")
                        LazyVGrid(columns: [GridItem(.flexible(), spacing: 12), GridItem(.flexible())], spacing: 12) {
                            ForEach(section.panes) { pane in
                                FeatureCard(pane: pane, model: model) { onSelect(pane) }
                            }
                        }
                    }
                    // The form spaces header-only sections tightly, which crowds the next heading.
                    .padding(.bottom, 16)
                }
            }

            Section {
                LabeledContent("Version", value: coreInfo().version)
            } header: {
                SectionHeading("About")
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

/// A General pane section's heading. Set explicitly, as the form styles plain-text headers
/// smaller and dimmer than ones holding other views, like the feature card groups.
private struct SectionHeading: View {
    let title: String

    init(_ title: String) { self.title = title }

    var body: some View {
        Text(title)
            .font(.headline)
            .foregroundStyle(.primary)
    }
}

/// A feature's glass card on the General pane, laid out like the Shortcuts gallery: clicking it opens
/// the feature's pane, and its switch in the corner turns the feature on and off.
private struct FeatureCard: View {
    let pane: SettingsPane
    let model: AppModel
    let onSelect: () -> Void

    @State private var isHovered = false

    var body: some View {
        let isOn = SettingsView.isEnabled(pane, model: model)
        Button(action: onSelect) {
            VStack(alignment: .leading, spacing: 0) {
                PaneIcon(pane: pane, size: 28)
                Spacer(minLength: 12)
                Text(pane.title)
                    .font(.system(size: 13, weight: .semibold))
                    .foregroundStyle(.primary)
                Text(pane.subtitle)
                    .font(.system(size: 11))
                    .foregroundStyle(.secondary)
                    .lineLimit(2, reservesSpace: true)
                    .padding(.top, 2)
            }
            .multilineTextAlignment(.leading)
            .frame(maxWidth: .infinity, alignment: .leading)
            .padding(12)
            .contentShape(.rect(cornerRadius: 10))
            .glassEffect(
                .regular.tint(.primary.opacity(isHovered ? 0.06 : 0)).interactive(),
                in: .rect(cornerRadius: 10)
            )
        }
        .buttonStyle(FeatureCardButtonStyle())
        .onHover { isHovered = $0 }
        .animation(.easeOut(duration: 0.15), value: isHovered)
        .help("Open \(pane.title) settings")
        .overlay(alignment: .topTrailing) {
            if let isOn {
                Toggle("Enable \(pane.title)", isOn: isOn)
                    .toggleStyle(.switch)
                    .controlSize(.mini)
                    .labelsHidden()
                    .help(isOn.wrappedValue ? "Turn \(pane.title) off" : "Turn \(pane.title) on")
                    .padding(12)
            }
        }
    }
}

/// Shrinks a feature card slightly while it's pressed.
private struct FeatureCardButtonStyle: ButtonStyle {
    func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .scaleEffect(configuration.isPressed ? 0.97 : 1)
            .animation(.easeOut(duration: 0.1), value: configuration.isPressed)
    }
}

/// Fixes the sidebar at `width` so the divider can't be dragged to collapse or resize it, which
/// `NavigationSplitView` doesn't enforce: it finds the AppKit split view item behind the sidebar and
/// pins its limits directly.
private struct SidebarLimits: NSViewRepresentable {
    let width: CGFloat

    func makeNSView(context: Context) -> NSView { Probe(width: width) }
    func updateNSView(_ nsView: NSView, context: Context) {}

    private final class Probe: NSView {
        let width: CGFloat
        private var observations: [NSKeyValueObservation] = []

        init(width: CGFloat) {
            self.width = width
            super.init(frame: .zero)
        }

        required init?(coder: NSCoder) { fatalError() }

        // Not `viewDidMoveToWindow`: the probe isn't inside the split view yet at that point.
        override func layout() {
            super.layout()
            guard observations.isEmpty else { return }
            var view = superview
            while let current = view, !(current is NSSplitView) { view = current.superview }
            guard let controller = (view as? NSSplitView)?.delegate as? NSSplitViewController,
                  let sidebar = controller.splitViewItems.first(where: { $0.behavior == .sidebar })
            else { return }
            // SwiftUI resets these after this runs, so keep setting them back.
            let width = width
            observations = [
                sidebar.observe(\.canCollapse, options: [.initial]) { item, _ in
                    if item.canCollapse { item.canCollapse = false }
                },
                sidebar.observe(\.minimumThickness, options: [.initial]) { item, _ in
                    if item.minimumThickness != width { item.minimumThickness = width }
                },
                sidebar.observe(\.maximumThickness, options: [.initial]) { item, _ in
                    if item.maximumThickness != width { item.maximumThickness = width }
                },
            ]
        }
    }
}
