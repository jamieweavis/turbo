import AppKit
import Observation
import SwiftUI

// MARK: - Clipboard history

/// The clipboard items the palette lists. Querying is cheap (the core answers from its
/// store), so it re-runs on every keystroke.
@MainActor
@Observable
final class ClipboardBrowser {
    private(set) var items: [ClipboardItem] = []
    var selection = 0
    var error: String?
    @ObservationIgnored private let core: Core
    @ObservationIgnored private var query = ""

    private static let limit: UInt32 = 100

    init(core: Core) {
        self.core = core
    }

    var selected: ClipboardItem? {
        items.indices.contains(selection) ? items[selection] : nil
    }

    /// Keeps the same item selected across refreshes (e.g. a new copy arriving), if it's
    /// still there.
    func refresh(query: String) {
        let selectedID = query == self.query ? selected?.id : nil
        self.query = query
        items = core.clipboardItems(query: query, limit: Self.limit)
        selection = selectedID.flatMap { id in items.firstIndex { $0.id == id } } ?? 0
    }

    func move(by offset: Int) {
        guard !items.isEmpty else { return }
        selection = (selection + offset + items.count) % items.count
    }
    func togglePinSelected() {
        guard let item = selected else { return }
        do {
            try core.setClipboardItemPinned(id: item.id, pinned: !item.pinned)
            refresh(query: query)
        } catch {
            self.error = error.userMessage
        }
    }

    /// Removes the row straight away; a failure brings it back with the error.
    func deleteSelected() {
        guard let item = selected else { return }
        items.remove(at: selection)
        selection = min(selection, max(items.count - 1, 0))
        do {
            try core.deleteClipboardItem(id: item.id)
        } catch {
            self.error = error.userMessage
            refresh(query: query)
        }
    }
}

/// Image previews by stored path, so scrolling and re-rendering don't re-read files.
@MainActor
enum ClipboardImages {
    private static let cache = NSCache<NSString, NSImage>()

    static func image(atPath path: String) -> NSImage? {
        if let image = cache.object(forKey: path as NSString) { return image }
        guard let image = NSImage(contentsOfFile: path) else { return nil }
        cache.setObject(image, forKey: path as NSString)
        return image
    }
}

struct ClipboardModeView: View {
    let model: PaletteModel

    private var browser: ClipboardBrowser { model.clipboard }

    var body: some View {
        VStack(spacing: 0) {
            if browser.items.isEmpty {
                emptyState
            } else {
                HStack(spacing: 0) {
                    list
                        .frame(width: 320)
                    Divider()
                    if let item = browser.selected {
                        ClipboardPreview(item: item)
                    }
                }
            }
            Divider()
            FooterHints(hints: hints)
        }
    }

    private var list: some View {
        ScrollViewReader { proxy in
            ScrollView {
                LazyVStack(spacing: 0) {
                    ForEach(Array(browser.items.enumerated()), id: \.element.id) { index, item in
                        ClipboardRow(item: item, isSelected: index == browser.selection)
                            .id(item.id)
                            .contentShape(.rect)
                            .onContinuousHover { phase in
                                if case .active = phase { model.hover(index) }
                            }
                            .onTapGesture {
                                browser.selection = index
                                model.openSelected()
                            }
                    }
                }
                .padding(PaletteLayout.listPadding)
            }
            .scrollIndicators(.never)
            .onChange(of: browser.selection) {
                if let id = browser.selected?.id { proxy.scrollTo(id) }
            }
        }
    }

    private var emptyState: some View {
        VStack(spacing: 8) {
            Image(systemName: "doc.on.clipboard")
                .font(.system(size: 30))
                .foregroundStyle(.tertiary)
            if !model.clipboardConfig.enabled {
                Text("Clipboard history is off")
                    .font(.headline)
                Text("Turn it on in Settings → Clipboard.")
                    .foregroundStyle(.secondary)
            } else if model.query.trimmingCharacters(in: .whitespaces).isEmpty {
                Text("Nothing copied yet")
                    .font(.headline)
                Text("Things you copy will show up here.")
                    .foregroundStyle(.secondary)
            } else {
                Text("No matches for “\(model.query)”")
                    .foregroundStyle(.secondary)
            }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
    }

    private var hints: [(key: String, label: String)] {
        let pastes = model.clipboardConfig.pasteOnOpen
        return [
            ("↩", pastes ? "Paste" : "Copy"),
            ("⌘↩", pastes ? "Copy" : "Paste"),
            ("⌘P", browser.selected?.pinned == true ? "Unpin" : "Pin"),
            ("⌘⌫", "Delete"),
            ("⌘K", "Actions"),
        ]
    }
}

private struct ClipboardRow: View {
    let item: ClipboardItem
    let isSelected: Bool

    var body: some View {
        HStack(spacing: 10) {
            icon
                .frame(width: 22, height: 22)
            Text(item.previewLine)
                .font(.system(size: 13))
                .lineLimit(1)
                .truncationMode(.tail)
            Spacer(minLength: 4)
            if item.pinned {
                Image(systemName: "pin.fill")
                    .font(.system(size: 10))
                    .foregroundStyle(.orange)
            }
            Text(RelativeAge.short(since: item.copiedAt))
                .font(.system(size: 11).monospacedDigit())
                .foregroundStyle(.secondary)
        }
        .padding(.horizontal, 10)
        .frame(height: 36)
        .background {
            if isSelected {
                RoundedRectangle(cornerRadius: 8).fill(PaletteLayout.selectionFill)
            }
        }
    }

    @ViewBuilder
    private var icon: some View {
        if item.kind == .image, let path = item.imagePath, let image = ClipboardImages.image(atPath: path) {
            Image(nsImage: image)
                .resizable()
                .aspectRatio(contentMode: .fill)
                .clipShape(.rect(cornerRadius: 4))
        } else {
            Image(systemName: item.kind.symbol)
                .font(.system(size: 13, weight: .medium))
                .foregroundStyle(.secondary)
        }
    }
}

private struct ClipboardPreview: View {
    let item: ClipboardItem

    var body: some View {
        VStack(spacing: 0) {
            content
                .frame(maxWidth: .infinity, maxHeight: .infinity)
            Divider()
            footer
        }
    }

    @ViewBuilder
    private var content: some View {
        switch item.kind {
        case .image:
            if let path = item.imagePath, let image = ClipboardImages.image(atPath: path) {
                Image(nsImage: image)
                    .resizable()
                    .aspectRatio(contentMode: .fit)
                    .padding(16)
            } else {
                Text(item.text).foregroundStyle(.secondary)
            }
        case .files:
            ScrollView {
                VStack(alignment: .leading, spacing: 8) {
                    ForEach(item.filePaths, id: \.self) { path in
                        HStack(spacing: 8) {
                            Image(nsImage: NSWorkspace.shared.icon(forFile: path))
                                .resizable()
                                .frame(width: 20, height: 20)
                            Text((path as NSString).abbreviatingWithTildeInPath)
                                .font(.system(size: 12))
                                .lineLimit(1)
                                .truncationMode(.middle)
                                .textSelection(.enabled)
                        }
                    }
                }
                .frame(maxWidth: .infinity, alignment: .leading)
                .padding(14)
            }
        case .text, .link:
            ScrollView {
                Text(item.text)
                    .font(.system(size: 12, design: .monospaced))
                    .textSelection(.enabled)
                    .frame(maxWidth: .infinity, alignment: .topLeading)
                    .padding(14)
            }
        }
    }

    private var footer: some View {
        HStack(spacing: 6) {
            if let bundleID = item.sourceBundleId, let icon = AppIcons.icon(forBundleID: bundleID) {
                Image(nsImage: icon)
                    .resizable()
                    .frame(width: 14, height: 14)
            }
            Text(item.sourceApp ?? "Unknown app")
            Spacer()
            Text(Date(timeIntervalSince1970: item.copiedAt), format: .dateTime.day().month().hour().minute())
        }
        .font(.system(size: 11))
        .foregroundStyle(.secondary)
        .padding(.horizontal, 14)
        .frame(height: 28)
    }
}

extension ClipboardItem {
    /// The first non-blank line, for the one-line list row.
    var previewLine: String {
        switch kind {
        case .files:
            let names = filePaths.map { ($0 as NSString).lastPathComponent }
            return names.count > 1 ? "\(names[0]) and \(names.count - 1) more" : names.first ?? text
        case .image, .text, .link:
            let line = text.split(whereSeparator: \.isNewline).first { !$0.allSatisfy(\.isWhitespace) }
            return line.map { $0.trimmingCharacters(in: .whitespaces) } ?? text
        }
    }
}

extension ClipboardKind {
    var symbol: String {
        switch self {
        case .text: "text.alignleft"
        case .link: "link"
        case .image: "photo"
        case .files: "doc.on.doc"
        }
    }
}

// MARK: - File search

/// Spotlight results for the palette. Searching blocks briefly in the core, so it runs off
/// the main actor, debounced, and only the newest search may publish its results.
@MainActor
@Observable
final class FileBrowser {
    private(set) var results: [FileResult] = []
    private(set) var isSearching = false
    var selection = 0
    var error: String?
    @ObservationIgnored private let core: Core
    @ObservationIgnored private var task: Task<Void, Never>?
    @ObservationIgnored private var generation = 0

    private static let debounce = Duration.milliseconds(120)
    private static let limit: UInt32 = 50

    init(core: Core) {
        self.core = core
    }

    var selected: FileResult? {
        results.indices.contains(selection) ? results[selection] : nil
    }

    func search(_ query: String) {
        task?.cancel()
        generation += 1
        let generation = generation
        let query = query.trimmingCharacters(in: .whitespaces)
        error = nil
        guard !query.isEmpty else {
            results = []
            isSearching = false
            return
        }
        isSearching = true
        let limit = Self.limit
        task = Task { [weak self, core] in
            try? await Task.sleep(for: Self.debounce)
            guard !Task.isCancelled else { return }
            let outcome = await Task.detached {
                Result { try core.searchFiles(query: query, limit: limit) }
            }.value
            guard let self, generation == self.generation else { return }
            self.isSearching = false
            switch outcome {
            case .success(let results):
                self.results = results
                self.selection = 0
            case .failure(let error):
                self.results = []
                self.error = error.userMessage
            }
        }
    }

    func move(by offset: Int) {
        guard !results.isEmpty else { return }
        selection = (selection + offset + results.count) % results.count
    }}

struct FilesModeView: View {
    let model: PaletteModel

    private var browser: FileBrowser { model.files }

    var body: some View {
        VStack(spacing: 0) {
            ScrollViewReader { proxy in
                ScrollView {
                    LazyVStack(spacing: 0) {
                        ForEach(Array(browser.results.enumerated()), id: \.element.path) { index, file in
                            FileRow(file: file, isSelected: index == browser.selection)
                                .id(file.path)
                                .contentShape(.rect)
                                .onContinuousHover { phase in
                                    if case .active = phase { model.hover(index) }
                                }
                                .onTapGesture {
                                    browser.selection = index
                                    model.openSelectedFile()
                                }
                        }
                    }
                    .padding(.vertical, PaletteLayout.listPadding)
                    .padding(.horizontal, 8)
                }
                .scrollIndicators(.never)
                .onChange(of: browser.selection) {
                    if let path = browser.selected?.path { proxy.scrollTo(path) }
                }
            }
            Divider()
            FooterHints(hints: [("↩", "Open"), ("⌘↩", "Show in Finder"), ("⌘Y", "Quick Look"), ("⌘K", "Actions")])
        }
    }
}

private struct FileRow: View {
    let file: FileResult
    let isSelected: Bool

    var body: some View {
        HStack(spacing: 12) {
            Image(nsImage: NSWorkspace.shared.icon(forFile: file.path))
                .resizable()
                .frame(width: 28, height: 28)
            VStack(alignment: .leading, spacing: 1) {
                Text(file.name)
                    .font(.system(size: 14, weight: .medium))
                    .lineLimit(1)
                Text(file.parent)
                    .font(.system(size: 11))
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
                    .truncationMode(.middle)
            }
            Spacer(minLength: 0)
            Text(RelativeAge.short(since: file.modified))
                .font(.system(size: 11).monospacedDigit())
                .foregroundStyle(.secondary)
        }
        .padding(.horizontal, 10)
        .frame(height: PaletteLayout.rowHeight)
        .background {
            if isSelected {
                RoundedRectangle(cornerRadius: 10).fill(PaletteLayout.selectionFill)
            }
        }
    }
}

// MARK: - Colours

/// The colour picker's history, filtered by the query in the core.
@MainActor
@Observable
final class ColorBrowser {
    private(set) var colors: [PickedColor] = []
    var selection = 0
    var error: String?
    @ObservationIgnored private let core: Core
    @ObservationIgnored private var query = ""

    init(core: Core) {
        self.core = core
    }

    var selected: PickedColor? {
        colors.indices.contains(selection) ? colors[selection] : nil
    }

    func refresh(query: String) {
        self.query = query
        colors = core.searchColorHistory(query: query)
        selection = 0
    }

    func move(_ direction: GridDirection) {
        let grid = GridNavigation(sectionCounts: [colors.count], columns: PaletteLayout.colorColumns)
        selection = grid.move(selection, direction)
    }

    /// Removes the swatch straight away; a failure brings it back with the error.
    func deleteSelected() {
        guard let color = selected else { return }
        colors.remove(at: selection)
        selection = min(selection, max(colors.count - 1, 0))
        do {
            try core.deletePickedColor(color: color)
        } catch {
            self.error = error.userMessage
            refresh(query: query)
        }
    }
}

struct ColorsModeView: View {
    let model: PaletteModel

    private var browser: ColorBrowser { model.colors }

    var body: some View {
        VStack(spacing: 0) {
            PaletteGrid(
                sections: [GridSection(title: nil, items: browser.colors)],
                columns: PaletteLayout.colorColumns,
                cellHeight: PaletteLayout.colorCellHeight,
                selection: browser.selection,
                onHover: { model.hover($0) },
                onTap: { index in
                    browser.selection = index
                    model.copySelectedColor()
                }
            ) { color, isSelected in
                ColorCell(color: color, format: model.colorFormat, isSelected: isSelected)
            }
            Divider()
            FooterHints(leading: otherFormats, hints: [("↩", "Copy \(model.colorFormat.title)"), ("⌘↩", "Pick Colour"), ("⌘K", "Actions")])
        }
    }

    /// The selected colour in the two formats ↩ doesn't copy.
    private var otherFormats: String? {
        guard let color = browser.selected else { return nil }
        return ColorFormat.allCases.filter { $0 != model.colorFormat }
            .map { formatColor(color: color, format: $0) }
            .joined(separator: "  ·  ")
    }
}

/// A large swatch with the colour in the copy format and its age underneath.
private struct ColorCell: View {
    let color: PickedColor
    let format: ColorFormat
    let isSelected: Bool

    var body: some View {
        VStack(spacing: 6) {
            RoundedRectangle(cornerRadius: 12)
                .fill(color.color.gradient)
                .overlay(RoundedRectangle(cornerRadius: 12).strokeBorder(.primary.opacity(0.12)))
                .overlay {
                    if isSelected {
                        RoundedRectangle(cornerRadius: 15)
                            .strokeBorder(Color.accentColor, lineWidth: 2.5)
                            .padding(-4)
                    }
                }
                .shadow(color: color.color.opacity(0.35), radius: isSelected ? 8 : 4, y: 2)
            VStack(spacing: 1) {
                Text(formatColor(color: color, format: format))
                    .font(.system(size: 12, weight: .medium).monospaced())
                    .lineLimit(1)
                    .minimumScaleFactor(0.7)
                Text(RelativeAge.short(since: color.pickedAt))
                    .font(.system(size: 10).monospacedDigit())
                    .foregroundStyle(.secondary)
            }
        }
        .padding(8)
        .background {
            if isSelected {
                RoundedRectangle(cornerRadius: 16).fill(PaletteLayout.selectionFill)
            }
        }
    }
}

// MARK: - Emoji

/// Emoji in sections, searched in the core. ↩ pastes into the app that was in front.
@MainActor
@Observable
final class EmojiBrowser {
    private(set) var sections: [EmojiSection] = []
    var selection = 0
    var error: String?
    /// Every section's emoji in grid order, for selecting by flat index.
    private var flat: [EmojiInfo] = []
    @ObservationIgnored private let core: Core

    init(core: Core) {
        self.core = core
    }

    var isEmpty: Bool { flat.isEmpty }

    var selected: EmojiInfo? {
        flat.indices.contains(selection) ? flat[selection] : nil
    }

    func refresh(query: String) {
        sections = core.searchEmoji(query: query)
        flat = sections.flatMap(\.emojis)
        selection = 0
    }

    func move(_ direction: GridDirection) {
        let grid = GridNavigation(sectionCounts: sections.map(\.emojis.count), columns: PaletteLayout.emojiColumns)
        selection = grid.move(selection, direction)
    }
}

struct EmojiModeView: View {
    let model: PaletteModel

    private var browser: EmojiBrowser { model.emoji }

    var body: some View {
        PaletteGrid(
            sections: browser.sections.map { GridSection(title: $0.title, items: $0.emojis, showsCount: true) },
            columns: PaletteLayout.emojiColumns,
            cellHeight: PaletteLayout.emojiCellHeight,
            spacing: PaletteLayout.emojiSpacing,
            padding: PaletteLayout.emojiPadding,
            selection: browser.selection,
            onHover: { model.hover($0) },
            onTap: { index in
                browser.selection = index
                model.insertSelectedEmoji(paste: true)
            }
        ) { emoji, isSelected in
            EmojiCell(emoji: emoji, isSelected: isSelected)
        }
        // Tiles scrolling under the action bar fade out.
        .mask {
            VStack(spacing: 0) {
                Rectangle()
                LinearGradient(colors: [.black, .clear], startPoint: .top, endPoint: .bottom)
                    .frame(height: PaletteLayout.actionBarHeight)
            }
        }
        .safeAreaInset(edge: .bottom, spacing: 0) {
            FloatingActionBar(onShowSettings: model.onShowSettings, actions: actions)
        }
    }

    private var actions: [FloatingAction] {
        guard browser.selected != nil else { return [] }
        return [
            FloatingAction(label: PaletteModel.pasteTitle, keys: ["↩"]) {
                model.insertSelectedEmoji(paste: true)
            },
            FloatingAction(label: "Copy", keys: ["⌘", "↩"]) {
                model.insertSelectedEmoji(paste: false)
            },
            .showActions(model),
        ]
    }
}

/// An emoji on its own tile, outlined when selected.
private struct EmojiCell: View {
    let emoji: EmojiInfo
    let isSelected: Bool

    var body: some View {
        Text(emoji.emoji)
            .font(.system(size: 40))
            .frame(maxWidth: .infinity, maxHeight: .infinity)
            .background(.primary.opacity(isSelected ? 0.14 : 0.05), in: .rect(cornerRadius: 14))
            .overlay {
                if isSelected {
                    RoundedRectangle(cornerRadius: 14).strokeBorder(.primary.opacity(0.85), lineWidth: 2)
                }
            }
            .help([emoji.name, emoji.shortcode.map { ":\($0):" }].compactMap(\.self).joined(separator: "  "))
            .animation(.snappy(duration: 0.15), value: isSelected)
    }
}

// MARK: - Grids

enum GridDirection {
    case left, right, up, down
}

/// Arrow-key movement through a grid whose sections each start a new row. Left and right
/// step through items in order; up and down keep the column (or the last item of a shorter
/// row). Both wrap around at the ends.
struct GridNavigation {
    /// Each row's flat item indices.
    private let rows: [Range<Int>]
    private let count: Int

    init(sectionCounts: [Int], columns: Int) {
        var rows: [Range<Int>] = []
        var start = 0
        for sectionCount in sectionCounts {
            for rowStart in stride(from: start, to: start + sectionCount, by: columns) {
                rows.append(rowStart..<min(rowStart + columns, start + sectionCount))
            }
            start += sectionCount
        }
        self.rows = rows
        count = start
    }

    func move(_ selection: Int, _ direction: GridDirection) -> Int {
        guard count > 0 else { return 0 }
        switch direction {
        case .left: return (selection - 1 + count) % count
        case .right: return (selection + 1) % count
        case .up, .down:
            guard let row = rows.firstIndex(where: { $0.contains(selection) }) else { return 0 }
            let column = selection - rows[row].lowerBound
            let target = rows[(row + (direction == .up ? -1 : 1) + rows.count) % rows.count]
            return min(target.lowerBound + column, target.upperBound - 1)
        }
    }
}

struct GridSection<Item> {
    let title: String?
    let items: [Item]
    /// Shows the number of items after the title: "Pinned 9".
    var showsCount = false
}

/// A scrolling grid of sections for the palette, selected by flat index across sections.
/// Cells are identified by that index, since an item can appear in more than one section.
struct PaletteGrid<Item, Cell: View>: View {
    let sections: [GridSection<Item>]
    let columns: Int
    let cellHeight: CGFloat
    var spacing = PaletteLayout.gridSpacing
    var padding = PaletteLayout.gridPadding
    let selection: Int
    let onHover: (Int) -> Void
    let onTap: (Int) -> Void
    @ViewBuilder let cell: (Item, Bool) -> Cell

    var body: some View {
        ScrollViewReader { proxy in
            ScrollView {
                LazyVGrid(
                    columns: Array(repeating: GridItem(.flexible(), spacing: spacing), count: columns),
                    spacing: spacing
                ) {
                    ForEach(Array(sectionOffsets.enumerated()), id: \.offset) { sectionIndex, offset in
                        let section = sections[sectionIndex]
                        Section {
                            ForEach(Array(section.items.enumerated()), id: \.offset) { itemIndex, item in
                                let index = offset + itemIndex
                                cell(item, index == selection)
                                    .frame(height: cellHeight)
                                    .id(index)
                                    .contentShape(.rect)
                                    .onContinuousHover { phase in
                                        if case .active = phase { onHover(index) }
                                    }
                                    .onTapGesture { onTap(index) }
                            }
                        } header: {
                            if let title = section.title {
                                HStack(spacing: 6) {
                                    Text(title)
                                        .foregroundStyle(.secondary)
                                    if section.showsCount {
                                        Text("\(section.items.count)")
                                            .monospacedDigit()
                                            .foregroundStyle(.tertiary)
                                    }
                                }
                                .font(.system(size: 12, weight: .semibold))
                                .frame(maxWidth: .infinity, alignment: .leading)
                                .padding(.horizontal, 2)
                                .frame(height: PaletteLayout.gridHeaderHeight, alignment: .bottom)
                                .padding(.bottom, 2)
                            }
                        }
                    }
                }
                .padding(.vertical, PaletteLayout.listPadding)
                .padding(.horizontal, padding)
            }
            .scrollIndicators(.never)
            .onChange(of: selection) {
                proxy.scrollTo(selection)
            }
        }
    }

    /// Each section's first flat index.
    private var sectionOffsets: [Int] {
        var offsets: [Int] = []
        var start = 0
        for section in sections {
            offsets.append(start)
            start += section.items.count
        }
        return offsets
    }
}

// MARK: - Shared

/// The key hints along the bottom of clipboard, file, colour and emoji mode, optionally
/// with a description of the selection on the left.
private struct FooterHints: View {
    var leading: String?
    let hints: [(key: String, label: String)]

    var body: some View {
        HStack(spacing: 14) {
            if let leading {
                Text(leading)
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
                    .truncationMode(.tail)
            }
            Spacer(minLength: 0)
            ForEach(hints, id: \.key) { hint in
                HStack(spacing: 6) {
                    Text(hint.label).foregroundStyle(.secondary)
                    Keycaps(keys: hint.key.map(String.init))
                }
            }
        }
        .font(.caption)
        .padding(.horizontal, 16)
        .frame(height: PaletteLayout.footerHeight)
    }
}

/// Compact ages for list rows: "now", "5m", "3h", "2d", then a short date.
enum RelativeAge {
    static func short(since timestamp: Double, now: Date = .now) -> String {
        let date = Date(timeIntervalSince1970: timestamp)
        let seconds = now.timeIntervalSince(date)
        switch seconds {
        case ..<60: return "now"
        case ..<3600: return "\(Int(seconds / 60))m"
        case ..<86_400: return "\(Int(seconds / 3600))h"
        case ..<(7 * 86_400): return "\(Int(seconds / 86_400))d"
        default: return date.formatted(.dateTime.day().month(.abbreviated))
        }
    }
}
