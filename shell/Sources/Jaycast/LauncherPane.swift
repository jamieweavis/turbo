import AppKit
import SwiftUI

struct LauncherPane: View {
    let settings: LauncherSettings

    /// An app picked in the sheet but not yet given a hotkey; it's added once one is recorded.
    @State private var pendingApp: AppInfo?
    @State private var isPickingApp = false

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
                        if let hotkey = settings.config.hotkey, let problem = settings.problem(with: hotkey) {
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
                ForEach(Array(settings.config.appHotkeys.enumerated()), id: \.offset) { index, appHotkey in
                    AppHotkeyRow(
                        name: appHotkey.name,
                        path: settings.path(forBundleID: appHotkey.bundleId),
                        hotkey: hotkeyBinding(at: index),
                        problem: settings.problem(with: appHotkey.hotkey, ignoringAppAt: index),
                        onRemove: { settings.config.appHotkeys.remove(at: index) }
                    )
                }

                if let pendingApp {
                    PendingAppHotkeyRow(
                        app: pendingApp,
                        onRecord: { hotkey in
                            settings.addAppHotkey(pendingApp, hotkey: hotkey)
                            self.pendingApp = nil
                        },
                        onCancel: { self.pendingApp = nil }
                    )
                }

                Button("Add App…", systemImage: "plus") {
                    settings.refreshApps()
                    isPickingApp = true
                }
            } header: {
                Text("App Hotkeys")
            } footer: {
                Text("Press an app's hotkey to bring it to the front, launching it if needed. Press it again to hide the app.")
            }
        }
        .formStyle(.grouped)
        .sheet(isPresented: $isPickingApp) {
            AppPicker(apps: settings.apps) { app in
                isPickingApp = false
                pendingApp = app
            } onCancel: {
                isPickingApp = false
            }
        }
    }

    private func hotkeyBinding(at index: Int) -> Binding<Hotkey> {
        Binding(
            get: { settings.config.appHotkeys[index].hotkey },
            set: { settings.config.appHotkeys[index].hotkey = $0 }
        )
    }
}

/// A recorder for an optional global hotkey, with a button to clear it.
private struct HotkeyField: View {
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

private struct AppHotkeyRow: View {
    let name: String
    let path: String?
    @Binding var hotkey: Hotkey
    let problem: String?
    let onRemove: () -> Void

    var body: some View {
        HStack(spacing: 10) {
            AppIconView(path: path)
            Text(name)
            if path == nil {
                Text("Not installed")
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

private struct PendingAppHotkeyRow: View {
    let app: AppInfo
    let onRecord: (Hotkey) -> Void
    let onCancel: () -> Void

    var body: some View {
        HStack(spacing: 10) {
            AppIconView(path: app.path)
            Text(app.name)
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

private struct AppIconView: View {
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

/// A searchable list of installed apps.
private struct AppPicker: View {
    let apps: [AppInfo]
    let onPick: (AppInfo) -> Void
    let onCancel: () -> Void

    @State private var filter = ""
    @State private var selection: String?

    private var filtered: [AppInfo] {
        let filter = filter.trimmingCharacters(in: .whitespaces)
        guard !filter.isEmpty else { return apps }
        return apps.filter { $0.name.localizedCaseInsensitiveContains(filter) }
    }

    var body: some View {
        VStack(spacing: 0) {
            TextField("Search apps", text: $filter)
                .textFieldStyle(.roundedBorder)
                .padding(12)

            List(filtered, id: \.bundleId, selection: $selection) { app in
                HStack(spacing: 8) {
                    AppIconView(path: app.path)
                    Text(app.name)
                }
                .tag(app.bundleId)
            }
            .overlay {
                if filtered.isEmpty {
                    Text(apps.isEmpty ? "No apps indexed yet" : "No matching apps")
                        .foregroundStyle(.secondary)
                }
            }
            .contextMenu(forSelectionType: String.self) { _ in } primaryAction: { ids in
                if let id = ids.first, let app = apps.first(where: { $0.bundleId == id }) {
                    onPick(app)
                }
            }

            HStack {
                Spacer()
                Button("Cancel", action: onCancel)
                    .keyboardShortcut(.cancelAction)
                Button("Choose") {
                    if let app = apps.first(where: { $0.bundleId == selection }) {
                        onPick(app)
                    }
                }
                .keyboardShortcut(.defaultAction)
                .disabled(selection == nil)
            }
            .padding(12)
        }
        .frame(width: 380, height: 460)
    }
}
