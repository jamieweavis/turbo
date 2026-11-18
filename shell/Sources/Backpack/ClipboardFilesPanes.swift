import AppKit
import SwiftUI
import os

struct ClipboardPane: View {
    let settings: LauncherSettings

    @State private var isPickingApp = false
    @State private var isConfirmingClear = false
    @State private var clearError: String?

    private static let itemLimits: [UInt32] = [100, 250, 500, 1000, 5000]
    private static let retentionDays: [UInt32] = [1, 7, 30, 90, 365]

    var body: some View {
        @Bindable var settings = settings

        Form {
            PaneHeaderSection(pane: .clipboard)

            if let error = settings.saveError ?? clearError {
                Section {
                    Label(error, systemImage: "exclamationmark.triangle.fill")
                        .foregroundStyle(.red)
                }
            }

            Section {
                LabeledContent("Open clipboard history") {
                    HStack(spacing: 6) {
                        if let hotkey = settings.config.clipboard.hotkey,
                           let problem = settings.problem(with: hotkey, in: .clipboard) {
                            Image(systemName: "exclamationmark.triangle.fill")
                                .foregroundStyle(.yellow)
                                .help(problem)
                        }
                        HotkeyField(hotkey: $settings.config.clipboard.hotkey)
                    }
                }
            }

            Section {
                Toggle("Choosing an item pastes it", isOn: $settings.config.clipboard.pasteOnOpen)
            } footer: {
                Text(settings.config.clipboard.pasteOnOpen
                    ? "↩ pastes into the app in front; ⌘↩ only copies. Pasting needs Accessibility access."
                    : "↩ only copies; ⌘↩ pastes into the app in front. Pasting needs Accessibility access.")
            }

            Section("Storage") {
                Picker("Keep up to", selection: $settings.config.clipboard.maxItems) {
                    ForEach(options(Self.itemLimits, including: settings.config.clipboard.maxItems), id: \.self) { limit in
                        Text(limit == 0 ? "Unlimited items" : "\(limit) items").tag(limit)
                    }
                }
                Picker("Keep items for", selection: $settings.config.clipboard.retentionDays) {
                    ForEach(options(Self.retentionDays, including: settings.config.clipboard.retentionDays), id: \.self) { days in
                        Text(days == 0 ? "Forever" : days == 1 ? "1 day" : "\(days) days").tag(days)
                    }
                }
                LabeledContent("History") {
                    Button("Clear History…", role: .destructive) { isConfirmingClear = true }
                }
            }

            Section {
                ForEach(settings.config.clipboard.ignoredApps, id: \.self) { bundleID in
                    HStack(spacing: 10) {
                        AppIconView(path: settings.path(forBundleID: bundleID))
                        Text(settings.appName(forBundleID: bundleID))
                        Spacer()
                        Button("Remove", systemImage: "minus.circle") {
                            settings.config.clipboard.ignoredApps.removeAll { $0 == bundleID }
                        }
                        .labelStyle(.iconOnly)
                        .buttonStyle(.borderless)
                    }
                }
                Button("Add App…", systemImage: "plus") {
                    settings.refreshApps()
                    isPickingApp = true
                }
            } header: {
                Text("Ignored Apps")
            } footer: {
                Text("Copies from these apps are never recorded. Copies that apps mark as private, like passwords from a password manager, are never recorded either.")
            }
        }
        .formStyle(.grouped)
        .sheet(isPresented: $isPickingApp) {
            AppPicker(apps: settings.apps) { app in
                isPickingApp = false
                if !settings.config.clipboard.ignoredApps.contains(app.bundleId) {
                    settings.config.clipboard.ignoredApps.append(app.bundleId)
                }
            } onCancel: {
                isPickingApp = false
            }
        }
        .confirmationDialog("Clear clipboard history?", isPresented: $isConfirmingClear) {
            Button("Clear History", role: .destructive) {
                do {
                    try settings.clearClipboardHistory()
                    clearError = nil
                } catch {
                    Logger.clipboard.error("couldn't clear clipboard history: \(error, privacy: .public)")
                    clearError = error.userMessage
                }
            }
        } message: {
            Text("Every item except pinned ones is deleted. This can't be undone.")
        }
    }

    /// The preset choices, plus the current value if it was set to something else.
    /// The presets plus a custom current value, with 0 (no limit) last.
    private func options(_ presets: [UInt32], including current: UInt32) -> [UInt32] {
        let values = current == 0 || presets.contains(current) ? presets : (presets + [current]).sorted()
        return values + [0]
    }
}

struct FileSearchPane: View {
    let settings: LauncherSettings

    var body: some View {
        @Bindable var settings = settings

        Form {
            PaneHeaderSection(pane: .files)

            if let error = settings.saveError {
                Section {
                    Label(error, systemImage: "exclamationmark.triangle.fill")
                        .foregroundStyle(.red)
                }
            }

            Section {
                LabeledContent("Search files") {
                    HStack(spacing: 6) {
                        if let hotkey = settings.config.files.hotkey,
                           let problem = settings.problem(with: hotkey, in: .files) {
                            Image(systemName: "exclamationmark.triangle.fill")
                                .foregroundStyle(.yellow)
                                .help(problem)
                        }
                        HotkeyField(hotkey: $settings.config.files.hotkey)
                    }
                }
            } footer: {
                Text("You can also type “Search Files” in the launcher.")
            }

            Section {
                ForEach(settings.config.files.scopes, id: \.self) { scope in
                    FolderRow(scope: scope) {
                        settings.config.files.scopes.removeAll { $0 == scope }
                    }
                }
                if settings.config.files.scopes.isEmpty {
                    Text("No folders: file search won't find anything.")
                        .foregroundStyle(.secondary)
                }
                Button("Add Folder…", systemImage: "plus", action: addFolders)
            } header: {
                Text("Folders")
            } footer: {
                Text("Results come from Spotlight, so only indexed folders return anything: folders excluded in Spotlight settings, or not indexed yet, find nothing.")
            }
        }
        .formStyle(.grouped)
    }

    private func addFolders() {
        let panel = NSOpenPanel()
        panel.canChooseDirectories = true
        panel.canChooseFiles = false
        panel.allowsMultipleSelection = true
        panel.prompt = "Add"
        guard panel.runModal() == .OK else { return }
        for url in panel.urls {
            let scope = (url.path(percentEncoded: false) as NSString).abbreviatingWithTildeInPath
            let trimmed = scope.count > 1 && scope.hasSuffix("/") ? String(scope.dropLast()) : scope
            if !settings.config.files.scopes.contains(trimmed) {
                settings.config.files.scopes.append(trimmed)
            }
        }
    }
}

private struct FolderRow: View {
    let scope: String
    let onRemove: () -> Void

    private var path: String { (scope as NSString).expandingTildeInPath }

    var body: some View {
        HStack(spacing: 10) {
            Image(nsImage: NSWorkspace.shared.icon(forFile: path))
                .resizable()
                .frame(width: 20, height: 20)
            Text(scope)
            if !FileManager.default.fileExists(atPath: path) {
                Image(systemName: "exclamationmark.triangle.fill")
                    .foregroundStyle(.yellow)
                    .help("This folder doesn't exist")
            }
            Spacer()
            Button("Remove", systemImage: "minus.circle", action: onRemove)
                .labelStyle(.iconOnly)
                .buttonStyle(.borderless)
        }
    }
}
