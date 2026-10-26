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
                RoundedRectangle(cornerRadius: 8).fill(.primary.opacity(0.1))
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
            FooterHints(hints: [("↩", "Open"), ("⌘↩", "Show in Finder"), ("⌘Y", "Quick Look"), ("⌘C", "Copy Path")])
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
                RoundedRectangle(cornerRadius: 10).fill(.primary.opacity(0.1))
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

    func move(by offset: Int) {
        guard !colors.isEmpty else { return }
        selection = (selection + offset + colors.count) % colors.count
    }

    /// Removes the row straight away; a failure brings it back with the error.
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
            ScrollViewReader { proxy in
                ScrollView {
                    LazyVStack(spacing: 0) {
                        ForEach(Array(browser.colors.enumerated()), id: \.element) { index, color in
                            ColorRow(color: color, format: model.colorFormat, isSelected: index == browser.selection)
                                .id(color)
                                .contentShape(.rect)
                                .onContinuousHover { phase in
                                    if case .active = phase { model.hover(index) }
                                }
                                .onTapGesture {
                                    browser.selection = index
                                    model.copySelectedColor()
                                }
                        }
                    }
                    .padding(.vertical, PaletteLayout.listPadding)
                    .padding(.horizontal, 8)
                }
                .scrollIndicators(.never)
                .onChange(of: browser.selection) {
                    if let color = browser.selected { proxy.scrollTo(color) }
                }
            }
            Divider()
            FooterHints(hints: [("↩", "Copy \(model.colorFormat.title)"), ("⌘↩", "Pick Colour"), ("⌘⌫", "Delete")])
        }
    }
}

/// The colour in the copy format, with the other two formats underneath.
private struct ColorRow: View {
    let color: PickedColor
    let format: ColorFormat
    let isSelected: Bool

    var body: some View {
        HStack(spacing: 12) {
            ColorSwatch(color: color, size: 28)
            VStack(alignment: .leading, spacing: 1) {
                Text(formatColor(color: color, format: format))
                    .font(.system(size: 14, weight: .medium).monospaced())
                    .lineLimit(1)
                Text(ColorFormat.allCases.filter { $0 != format }.map { formatColor(color: color, format: $0) }.joined(separator: "  ·  "))
                    .font(.system(size: 11).monospaced())
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
            }
            Spacer(minLength: 0)
            Text(RelativeAge.short(since: color.pickedAt))
                .font(.system(size: 11).monospacedDigit())
                .foregroundStyle(.secondary)
        }
        .padding(.horizontal, 10)
        .frame(height: PaletteLayout.rowHeight)
        .background {
            if isSelected {
                RoundedRectangle(cornerRadius: 10).fill(.primary.opacity(0.1))
            }
        }
    }
}

// MARK: - Shared

/// The key hints along the bottom of clipboard, file and colour mode.
private struct FooterHints: View {
    let hints: [(key: String, label: String)]

    var body: some View {
        HStack(spacing: 14) {
            Spacer()
            ForEach(hints, id: \.key) { hint in
                HStack(spacing: 4) {
                    Text(hint.label).foregroundStyle(.secondary)
                    Text(hint.key).fontWeight(.semibold).foregroundStyle(.secondary)
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
