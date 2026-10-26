import AppKit
import SwiftUI

struct LauncherPane: View {
    let settings: LauncherSettings

    /// An app or action picked in the sheet but not yet given a hotkey; it's added once one
    /// is recorded.
    @State private var pending: HotkeyTarget?
    @State private var isPicking = false

    var body: some View {
        @Bindable var settings = settings

        Form {
            if let error = settings.saveError {
                Section {
                    Label(error, systemImage: "exclamationmark.triangle.fill")
                        .foregroundStyle(.red)
                }
            }

            Section {
                LabeledContent("Open launcher") {
                    HStack(spacing: 6) {
                        if let hotkey = settings.config.hotkey, let problem = settings.problem(with: hotkey, in: .palette) {
                            Image(systemName: "exclamationmark.triangle.fill")
                                .foregroundStyle(.yellow)
                                .help(problem)
                        }
                        HotkeyField(hotkey: $settings.config.hotkey)
                    }
                }
            } footer: {
                Text("Opens the palette from anywhere. The menu bar can always open it too.")
            }

            Section {
                Toggle("Show calculator and conversion results", isOn: $settings.config.calculator)
            } footer: {
                Text("Type maths, units or currencies, like “12% of 80” or “5 km in miles”. Press ↩ to copy the answer.")
            }

            Section {
                ForEach(Array(settings.config.appHotkeys.enumerated()), id: \.offset) { index, appHotkey in
                    let path = settings.path(forBundleID: appHotkey.bundleId)
                    TargetHotkeyRow(
                        name: appHotkey.name,
                        note: path == nil ? "Not installed" : nil,
                        hotkey: hotkeyBinding(at: index),
                        problem: settings.problem(with: appHotkey.hotkey, in: .app(index)),
                        onRemove: { settings.config.appHotkeys.remove(at: index) }
                    ) {
                        AppIconView(path: path)
                    }
                }

                ForEach(Array(settings.config.actionHotkeys.enumerated()), id: \.offset) { index, actionHotkey in
                    let action = settings.systemAction(withID: actionHotkey.action)
                    TargetHotkeyRow(
                        name: action?.title ?? actionHotkey.action,
                        note: action == nil ? "Unknown action" : nil,
                        hotkey: actionHotkeyBinding(at: index),
                        problem: settings.problem(with: actionHotkey.hotkey, in: .action(index)),
                        onRemove: { settings.config.actionHotkeys.remove(at: index) }
                    ) {
                        ActionIconView(symbol: action?.symbol)
                    }
                }

                if let pending {
                    PendingHotkeyRow(
                        target: pending,
                        onRecord: { hotkey in
                            settings.addHotkey(hotkey, for: pending)
                            self.pending = nil
                        },
                        onCancel: { self.pending = nil }
                    )
                }

                Button("Add…", systemImage: "plus") {
                    settings.refreshApps()
                    isPicking = true
                }
            } header: {
                Text("Hotkeys")
            } footer: {
                Text("Press an app's hotkey to bring it to the front, launching it if needed. Press it again to hide the app. Other hotkeys run their action straight away.")
            }
        }
        .formStyle(.grouped)
        .sheet(isPresented: $isPicking) {
            AppPicker(apps: settings.apps, actions: settings.systemActions) { app in
                isPicking = false
                pending = .app(app)
            } onCancel: {
                isPicking = false
            } onPickAction: { action in
                isPicking = false
                pending = .action(action)
            }
        }
    }

    private func hotkeyBinding(at index: Int) -> Binding<Hotkey> {
        Binding(
            get: { settings.config.appHotkeys[index].hotkey },
            set: { settings.config.appHotkeys[index].hotkey = $0 }
        )
    }

    private func actionHotkeyBinding(at index: Int) -> Binding<Hotkey> {
        Binding(
            get: { settings.config.actionHotkeys[index].hotkey },
            set: { settings.config.actionHotkeys[index].hotkey = $0 }
        )
    }
}

/// What a hotkey in the Hotkeys list opens or runs.
enum HotkeyTarget {
    case app(AppInfo)
    case action(SystemActionInfo)
}

/// A recorder for an optional global hotkey, with a button to clear it.
struct HotkeyField: View {
    @Binding var hotkey: Hotkey?

    var body: some View {
        HStack(spacing: 6) {
            ShortcutRecorder(
                shortcut: Binding(
                    get: { hotkey.map(Shortcut.init) },
                    set: { hotkey = $0?.hotkey }
                ),
                requiresModifier: true
            )
            if hotkey != nil {
                Button("Clear", systemImage: "xmark.circle.fill") { hotkey = nil }
                    .labelStyle(.iconOnly)
                    .buttonStyle(.borderless)
                    .foregroundStyle(.secondary)
                    .help("Remove the hotkey")
            }
        }
    }
}

private struct TargetHotkeyRow<Icon: View>: View {
    let name: String
    /// Why the target can't be opened, shown next to its name.
    let note: String?
    @Binding var hotkey: Hotkey
    let problem: String?
    let onRemove: () -> Void
    @ViewBuilder let icon: () -> Icon

    var body: some View {
        HStack(spacing: 10) {
            icon()
            Text(name)
            if let note {
                Text(note)
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }
            Spacer()
            if let problem {
                Image(systemName: "exclamationmark.triangle.fill")
                    .foregroundStyle(.yellow)
                    .help(problem)
            }
            ShortcutRecorder(
                shortcut: Binding(
                    get: { Shortcut(hotkey) },
                    set: { if let shortcut = $0 { hotkey = shortcut.hotkey } }
                ),
                requiresModifier: true
            )
            Button("Remove", systemImage: "minus.circle", action: onRemove)
                .labelStyle(.iconOnly)
                .buttonStyle(.borderless)
        }
    }
}

private struct PendingHotkeyRow: View {
    let target: HotkeyTarget
    let onRecord: (Hotkey) -> Void
    let onCancel: () -> Void

    var body: some View {
        HStack(spacing: 10) {
            switch target {
            case .app(let app):
                AppIconView(path: app.path)
                Text(app.name)
            case .action(let action):
                ActionIconView(symbol: action.symbol)
                Text(action.title)
            }
            Spacer()
            ShortcutRecorder(
                shortcut: Binding(
                    get: { nil },
                    set: { if let shortcut = $0 { onRecord(shortcut.hotkey) } }
                ),
                recordsOnAppear: true,
                requiresModifier: true
            )
            Button("Cancel", systemImage: "xmark.circle", action: onCancel)
                .labelStyle(.iconOnly)
                .buttonStyle(.borderless)
        }
    }
}

/// A system action's SF Symbol, sized to sit in a list next to app icons.
struct ActionIconView: View {
    let symbol: String?

    var body: some View {
        Image(systemName: symbol ?? "questionmark.square.dashed")
            .foregroundStyle(.secondary)
            .frame(width: 20, height: 20)
    }
}

struct AppIconView: View {
    let path: String?

    var body: some View {
        Group {
            if let path {
                Image(nsImage: AppIcons.icon(forPath: path))
                    .resizable()
            } else {
                Image(systemName: "app.dashed")
                    .resizable()
                    .foregroundStyle(.secondary)
            }
        }
        .aspectRatio(contentMode: .fit)
        .frame(width: 20, height: 20)
    }
}

/// A searchable list of installed apps, and optionally system actions above them.
struct AppPicker: View {
    let apps: [AppInfo]
    var actions: [SystemActionInfo] = []
    let onPick: (AppInfo) -> Void
    let onCancel: () -> Void
    var onPickAction: (SystemActionInfo) -> Void = { _ in }

    @State private var filter = ""
    /// `app:<bundle id>` or `action:<action id>`.
    @State private var selection: String?

    private var trimmedFilter: String {
        filter.trimmingCharacters(in: .whitespaces)
    }

    private var filteredApps: [AppInfo] {
        guard !trimmedFilter.isEmpty else { return apps }
        return apps.filter { $0.name.localizedCaseInsensitiveContains(trimmedFilter) }
    }

    private var filteredActions: [SystemActionInfo] {
        guard !trimmedFilter.isEmpty else { return actions }
        return actions.filter { $0.title.localizedCaseInsensitiveContains(trimmedFilter) }
    }

    var body: some View {
        VStack(spacing: 0) {
            TextField(actions.isEmpty ? "Search apps" : "Search apps and actions", text: $filter)
                .textFieldStyle(.roundedBorder)
                .padding(12)

            List(selection: $selection) {
                if !filteredActions.isEmpty {
                    Section("System") {
                        ForEach(filteredActions, id: \.id) { action in
                            HStack(spacing: 8) {
                                ActionIconView(symbol: action.symbol)
                                Text(action.title)
                            }
                            .tag("action:\(action.id)")
                        }
                    }
                }
                if actions.isEmpty {
                    appRows
                } else if !filteredApps.isEmpty {
                    Section("Apps") { appRows }
                }
            }
            .overlay {
                if filteredApps.isEmpty && filteredActions.isEmpty {
                    Text(apps.isEmpty && actions.isEmpty ? "No apps indexed yet" : "No matches")
                        .foregroundStyle(.secondary)
                }
            }
            .contextMenu(forSelectionType: String.self) { _ in } primaryAction: { ids in
                if let id = ids.first { pick(id) }
            }

            HStack {
                Spacer()
                Button("Cancel", action: onCancel)
                    .keyboardShortcut(.cancelAction)
                Button("Choose") {
                    if let selection { pick(selection) }
                }
                .keyboardShortcut(.defaultAction)
                .disabled(selection == nil)
            }
            .padding(12)
        }
        .frame(width: 380, height: 460)
    }

    private var appRows: some View {
        ForEach(filteredApps, id: \.bundleId) { app in
            HStack(spacing: 8) {
                AppIconView(path: app.path)
                Text(app.name)
            }
            .tag("app:\(app.bundleId)")
        }
    }

    private func pick(_ id: String) {
        if let bundleID = id.stripping(prefix: "app:"), let app = apps.first(where: { $0.bundleId == bundleID }) {
            onPick(app)
        } else if let actionID = id.stripping(prefix: "action:"), let action = actions.first(where: { $0.id == actionID }) {
            onPickAction(action)
        }
    }
}

private extension String {
    func stripping(prefix: String) -> String? {
        hasPrefix(prefix) ? String(dropFirst(prefix.count)) : nil
    }
}
