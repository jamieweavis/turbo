import AppKit
import SwiftUI

struct LauncherPane: View {
    let settings: LauncherSettings

    @State private var filter = ""
    @State private var show = ItemFilter.all
    /// A category to show alone, or nil for every category.
    @State private var category: String?
    /// Empty keeps the core's order: apps by name, then the commands grouped by category.
    @State private var sortOrder: [KeyPathComparator<ItemRowModel>] = []
    @Environment(\.appearsActive) private var appearsActive

    private static let categories = ["Application", "Backpack", "System", "Window", "System Settings"]

    var body: some View {
        @Bindable var settings = settings
        let rows = rows()

        VStack(alignment: .leading, spacing: 12) {
            PaneHeader(pane: .launcher)

            if let error = settings.saveError {
                Label(error, systemImage: "exclamationmark.triangle.fill")
                    .foregroundStyle(.red)
            }

            // Plain controls rather than a Form: a Form above the table sizes to its ideal
            // height, which overflows the window and shoves everything under the title bar.
            GroupBox {
                VStack(spacing: 8) {
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
                    Divider()
                    Toggle("Show calculator and conversion results", isOn: $settings.config.calculator)
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .help("Type maths, units or currencies, like “12% of 80” or “5 km in miles”. Press ↩ to copy the answer.")
                }
                .padding(6)
            }

            HStack(spacing: 8) {
                TextField("Search", text: $filter, prompt: Text("Search apps and commands"))
                    .translucentField()
                Picker("Show", selection: $show) {
                    ForEach(ItemFilter.allCases) { Text($0.title).tag($0) }
                }
                .fixedSize()
                Picker("Type", selection: $category) {
                    Text("All Types").tag(String?.none)
                    Divider()
                    ForEach(Self.categories, id: \.self) { Text(Self.plural($0)).tag(Optional($0)) }
                }
                .labelsHidden()
                .fixedSize()
            }

            Table(rows, sortOrder: $sortOrder) {
                TableColumn("") { row in
                    Toggle("Show in search", isOn: Binding(
                        get: { row.enabled },
                        set: { settings.setEnabled($0, for: row.item) }
                    ))
                    .toggleStyle(.checkbox)
                    .labelsHidden()
                    .help(row.enabled ? "Shown in search" : "Hidden from search")
                }
                .width(18)

                TableColumn("Name", value: \.title) { row in
                    ItemNameCell(row: row)
                }
                .width(min: 140, ideal: 200)

                TableColumn("Type", value: \.category) { row in
                    Text(row.category)
                        .foregroundStyle(.secondary)
                }
                .width(min: 70, ideal: 90)

                TableColumn("Alias") { row in
                    AliasField(aliases: row.aliases) { settings.setAliases($0, for: row.item) }
                }
                .width(min: 90, ideal: 120)

                TableColumn("Hotkey") { row in
                    HStack(spacing: 4) {
                        HotkeyField(hotkey: Binding(
                            get: { row.hotkey },
                            set: { settings.setHotkey($0, for: row.item) }
                        ))
                        if let problem = row.problem {
                            Image(systemName: "exclamationmark.triangle.fill")
                                .foregroundStyle(.yellow)
                                .help(problem)
                        }
                    }
                }
                .width(min: 150, ideal: 160)
            }
            // The table's background and the stripes on every other row are opaque, so when the
            // window goes inactive and its background lightens, they turn into a dark block. Swap
            // them for a translucent tint, which lightens along with the window, until it's back.
            .alternatingRowBackgrounds(appearsActive ? .enabled : .disabled)
            .scrollContentBackground(appearsActive ? .automatic : .hidden)
            .background(appearsActive ? .clear : Color.primary.opacity(0.05))
            .overlay {
                if rows.isEmpty {
                    Text(settings.items.isEmpty ? "Indexing apps…" : "No matches")
                        .foregroundStyle(.secondary)
                }
            }
            // The table's ideal height is every row stacked; a small one keeps it from growing the
            // window to fit them all.
            .frame(minHeight: 120, idealHeight: 300, maxHeight: .infinity)

            Text("Untick an item to hide it from search; its hotkey still works. Aliases are extra names to find it by, separated by commas — typing one exactly puts it first. An app's hotkey brings it to the front, or hides it if it's already in front.")
                .font(.caption)
                .foregroundStyle(.secondary)
                // Not fixedSize: the window's minimum size measures the pane at zero width,
                // where a fixed-height caption wraps a word per line and is thousands of points
                // tall, forcing the window taller than the screen.
                .frame(maxWidth: .infinity, alignment: .leading)
        }
        .padding(20)
        .onAppear { settings.refreshApps() }
    }

    /// The filtered, sorted rows, each with everything it shows looked up once, so a row's
    /// cells don't each search the config.
    private func rows() -> [ItemRowModel] {
        let filter = filter.trimmingCharacters(in: .whitespaces)
        let rows = (settings.items + settings.missingAppItems).compactMap { item -> ItemRowModel? in
            if let category, item.category != category { return nil }
            let enabled = settings.isEnabled(item)
            let aliases = settings.aliases(for: item)
            let hotkey = settings.hotkey(for: item)
            switch show {
            case .all: break
            case .enabled: if !enabled { return nil }
            case .disabled: if enabled { return nil }
            case .withHotkey: if hotkey == nil { return nil }
            case .withAlias: if aliases.isEmpty { return nil }
            }
            if !filter.isEmpty,
               !item.title.localizedCaseInsensitiveContains(filter),
               !aliases.contains(where: { $0.localizedCaseInsensitiveContains(filter) }) {
                return nil
            }
            return ItemRowModel(
                item: item,
                enabled: enabled,
                aliases: aliases,
                hotkey: hotkey,
                problem: hotkey.flatMap { settings.problem(with: $0, in: settings.hotkeySlot(for: item)) }
            )
        }
        return sortOrder.isEmpty ? rows : rows.sorted(using: sortOrder)
    }

    private static func plural(_ category: String) -> String {
        category == "Application" ? "Applications" : category
    }
}

private enum ItemFilter: CaseIterable, Identifiable {
    case all, enabled, disabled, withHotkey, withAlias

    var id: Self { self }

    var title: String {
        switch self {
        case .all: "All"
        case .enabled: "Shown in Search"
        case .disabled: "Hidden from Search"
        case .withHotkey: "With Hotkey"
        case .withAlias: "With Alias"
        }
    }
}

/// One app or command in the Launcher pane's table.
private struct ItemRowModel: Identifiable {
    let item: LauncherItem
    let enabled: Bool
    let aliases: [String]
    let hotkey: Hotkey?
    /// Why the hotkey won't work, if it won't.
    let problem: String?

    var id: String { item.id }
    var title: String { item.title }
    var category: String { item.category }
}

private struct ItemNameCell: View {
    let row: ItemRowModel

    var body: some View {
        HStack(spacing: 8) {
            Group {
                if row.item.path != nil || row.item.symbol == nil {
                    AppIconView(path: row.item.path)
                } else {
                    ActionIconView(symbol: row.item.symbol)
                }
            }
            .opacity(row.enabled ? 1 : 0.5)
            Text(row.title)
                .lineLimit(1)
                .foregroundStyle(row.enabled ? .primary : .secondary)
            if row.category == "Application", row.item.path == nil {
                Text("Not installed")
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }
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
            .translucentField()
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

/// A text field on a translucent fill instead of the bordered style's opaque one, so it stays
/// in step with the window background as the window goes inactive and lightens.
private struct TranslucentField: ViewModifier {
    @FocusState private var isFocused: Bool

    func body(content: Content) -> some View {
        content
            .textFieldStyle(.plain)
            .focused($isFocused)
            .padding(.horizontal, 6)
            .padding(.vertical, 4)
            .background(Color.primary.opacity(0.06), in: .rect(cornerRadius: 6))
            // The plain style has no focus ring of its own.
            .overlay {
                RoundedRectangle(cornerRadius: 6)
                    .strokeBorder(Color.accentColor, lineWidth: 2)
                    .opacity(isFocused ? 1 : 0)
            }
    }
}

private extension View {
    func translucentField() -> some View { modifier(TranslucentField()) }
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
