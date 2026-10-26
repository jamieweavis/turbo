import AppKit
import SwiftUI

struct LauncherPane: View {
    let settings: LauncherSettings

    @State private var filter = ""

    /// The list's sections, in order. Items come from the core grouped this way already.
    private static let categories = ["Application", "Jaycast", "System", "Window", "System Settings"]

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
                TextField("Search", text: $filter, prompt: Text("Search apps and commands"))
                    .labelsHidden()
            } header: {
                Text("Apps and Commands")
            } footer: {
                Text("Untick an item to hide it from search; its hotkey still works. Aliases are extra names to find it by, separated by commas — typing one exactly puts the item first. An app's hotkey brings it to the front, launching it if needed, and hides it if it's already in front.")
            }

            let items = filteredItems
            ForEach(Self.categories, id: \.self) { category in
                let rows = items.filter { $0.category == category }
                if !rows.isEmpty {
                    Section {
                        ForEach(rows, id: \.id) { item in
                            ItemRow(settings: settings, item: item)
                        }
                    } header: {
                        ItemColumnsHeader(title: category == "Application" ? "Applications" : category)
                    }
                }
            }
            if items.isEmpty {
                Section {
                    Text(settings.items.isEmpty ? "Indexing apps…" : "No matches")
                        .foregroundStyle(.secondary)
                        .frame(maxWidth: .infinity)
                }
            }
        }
        .formStyle(.grouped)
        .onAppear { settings.refreshApps() }
    }

    private var filteredItems: [LauncherItem] {
        let all = settings.items + settings.missingAppItems
        let filter = filter.trimmingCharacters(in: .whitespaces)
        guard !filter.isEmpty else { return all }
        return all.filter { item in
            item.title.localizedCaseInsensitiveContains(filter)
                || settings.aliases(for: item).contains { $0.localizedCaseInsensitiveContains(filter) }
        }
    }
}

/// Column widths shared by the list's header and rows, so they line up as a grid.
private enum ItemColumns {
    static let alias: CGFloat = 130
    static let hotkey: CGFloat = 150
}

private struct ItemColumnsHeader: View {
    let title: String

    var body: some View {
        HStack(spacing: 10) {
            Text(title)
            Spacer()
            Text("Alias")
                .frame(width: ItemColumns.alias, alignment: .leading)
            Text("Hotkey")
                .frame(width: ItemColumns.hotkey, alignment: .leading)
        }
    }
}

/// One app or command: whether it shows in search, its aliases and its hotkey.
private struct ItemRow: View {
    let settings: LauncherSettings
    let item: LauncherItem

    var body: some View {
        let enabled = settings.isEnabled(item)
        let hotkey = settings.hotkey(for: item)

        HStack(spacing: 10) {
            Toggle("Show in search", isOn: Binding(
                get: { enabled },
                set: { settings.setEnabled($0, for: item) }
            ))
            .toggleStyle(.checkbox)
            .labelsHidden()
            .help(enabled ? "Shown in search" : "Hidden from search")

            Group {
                if item.path != nil || item.symbol == nil {
                    AppIconView(path: item.path)
                } else {
                    ActionIconView(symbol: item.symbol)
                }
            }
            .opacity(enabled ? 1 : 0.5)
            Text(item.title)
                .lineLimit(1)
                .foregroundStyle(enabled ? .primary : .secondary)
            if item.category == "Application", item.path == nil {
                Text("Not installed")
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }

            Spacer(minLength: 4)

            AliasField(aliases: settings.aliases(for: item)) { settings.setAliases($0, for: item) }
                .frame(width: ItemColumns.alias)

            HStack(spacing: 6) {
                HotkeyField(hotkey: Binding(
                    get: { hotkey },
                    set: { settings.setHotkey($0, for: item) }
                ))
                if let hotkey, let problem = settings.problem(with: hotkey, in: settings.hotkeySlot(for: item)) {
                    Image(systemName: "exclamationmark.triangle.fill")
                        .foregroundStyle(.yellow)
                        .help(problem)
                }
            }
            .frame(width: ItemColumns.hotkey, alignment: .leading)
        }
    }
}

/// Comma-separated aliases, saved when editing ends rather than on every keystroke (each save
/// writes the config and re-registers the hotkeys).
private struct AliasField: View {
    let aliases: [String]
    let onCommit: ([String]) -> Void

    @State private var text = ""
    @FocusState private var isFocused: Bool

    var body: some View {
        TextField("Alias", text: $text, prompt: Text("Add alias"))
            .labelsHidden()
            .textFieldStyle(.roundedBorder)
            .focused($isFocused)
            .onAppear { text = aliases.joined(separator: ", ") }
            .onChange(of: aliases) { _, aliases in
                if !isFocused { text = aliases.joined(separator: ", ") }
            }
            .onChange(of: isFocused) { _, focused in
                if !focused { commit() }
            }
            .onSubmit(commit)
    }

    private func commit() {
        let parsed = text
            .split(separator: ",")
            .map { $0.trimmingCharacters(in: .whitespaces) }
            .filter { !$0.isEmpty }
        if parsed != aliases { onCommit(parsed) }
        text = parsed.joined(separator: ", ")
    }
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

/// A searchable list of installed apps.
struct AppPicker: View {
    let apps: [AppInfo]
    let onPick: (AppInfo) -> Void
    let onCancel: () -> Void

    @State private var filter = ""
    @State private var selection: String?

    private var filteredApps: [AppInfo] {
        let filter = filter.trimmingCharacters(in: .whitespaces)
        guard !filter.isEmpty else { return apps }
        return apps.filter { $0.name.localizedCaseInsensitiveContains(filter) }
    }

    var body: some View {
        VStack(spacing: 0) {
            TextField("Search apps", text: $filter)
                .textFieldStyle(.roundedBorder)
                .padding(12)

            List(filteredApps, id: \.bundleId, selection: $selection) { app in
                HStack(spacing: 8) {
                    AppIconView(path: app.path)
                    Text(app.name)
                }
            }
            .overlay {
                if filteredApps.isEmpty {
                    Text(apps.isEmpty ? "No apps indexed yet" : "No matches")
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

    private func pick(_ bundleID: String) {
        if let app = apps.first(where: { $0.bundleId == bundleID }) {
            onPick(app)
        }
    }
}

extension String {
    func stripping(prefix: String) -> String? {
        hasPrefix(prefix) ? String(dropFirst(prefix.count)) : nil
    }
}
