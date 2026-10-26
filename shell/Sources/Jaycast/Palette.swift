import AppKit
import Carbon.HIToolbox
import Observation
import Quartz
import SwiftUI

/// The launcher palette: a floating Liquid Glass panel with a search field and results.
/// It has three modes (search, clipboard history, file search) sharing the one field.
///
/// The panel is non-activating, so the frontmost app keeps looking active, but it still
/// becomes key so typing goes to the search field. The controller solely owns the frame:
/// it pins the top edge and horizontal centre where the palette opened and resizes around
/// them as results and modes change, so the search field never moves.
@MainActor
final class PaletteWindowController: NSObject, NSWindowDelegate {
    let model: PaletteModel
    private lazy var panel = makePanel()
    /// The top edge and horizontal centre chosen when the palette was shown.
    private var top: CGFloat = 0
    private var centerX: CGFloat = 0
    private var resizeTask: Task<Void, Never>?
    private let quickLook = QuickLookSource()

    init(core: Core, settings: LauncherSettings) {
        model = PaletteModel(core: core, settings: settings)
        super.init()
        model.onClose = { [weak self] in self?.hide() }
        model.onReopen = { [weak self] in
            guard let self else { return }
            self.show(mode: self.model.mode)
        }
        model.onQuickLook = { [weak self] url in self?.toggleQuickLook(url) }
    }

    var isVisible: Bool { panel.isVisible }

    /// For hotkeys: hide if the palette is already up in `mode`, otherwise show it in `mode`
    /// (switching in place if it's up in another mode).
    func toggle(mode: PaletteMode = .search) {
        if panel.isVisible, panel.isKeyWindow, model.mode == mode {
            hide()
        } else {
            show(mode: mode)
        }
    }

    /// Search mode keeps the last query, selected, like Spotlight: typing replaces it, and ⏎
    /// repeats the last search. Clipboard history and file search start empty.
    func show(mode: PaletteMode = .search) {
        if panel.isVisible {
            if model.mode != mode { model.enter(mode) }
            panel.makeKeyAndOrderFront(nil)
            return
        }
        model.prepareForShow(mode: mode)
        let screen = NSScreen.screens.first { NSMouseInRect(NSEvent.mouseLocation, $0.frame, false) } ?? NSScreen.main
        if let visible = screen?.visibleFrame {
            top = visible.maxY - (visible.height * 0.22).rounded()
            centerX = visible.midX
            panel.setFrame(frame(), display: false)
        }
        panel.makeKeyAndOrderFront(nil)
        observeSize()
    }

    func hide() {
        resizeTask?.cancel()
        resizeTask = nil
        if Self.isQuickLookVisible {
            QLPreviewPanel.shared().orderOut(nil)
        }
        panel.orderOut(nil)
        model.didHide()
    }

    func windowDidResignKey(_ notification: Notification) {
        // Quick Look takes key while it's up; the palette stays open behind it.
        guard !Self.isQuickLookVisible else { return }
        hide()
    }

    // MARK: Private

    private static var isQuickLookVisible: Bool {
        QLPreviewPanel.sharedPreviewPanelExists() && QLPreviewPanel.shared().isVisible
    }

    private func frame() -> NSRect {
        let size = PaletteLayout.size(for: model)
        return NSRect(
            x: (centerX - size.width / 2).rounded(),
            y: top - size.height,
            width: size.width,
            height: size.height
        )
    }

    private func observeSize() {
        guard resizeTask == nil else { return }
        resizeTask = Task { [weak self, model] in
            for await _ in Observations({ PaletteLayout.size(for: model) }) {
                guard let self, self.panel.isVisible else { continue }
                self.panel.setFrame(self.frame(), display: true)
            }
        }
    }

    /// Quick Look finds its data source through the key window (the panel) when it opens.
    private func toggleQuickLook(_ url: URL) {
        quickLook.url = url
        guard let preview = QLPreviewPanel.shared() else { return }
        if preview.isVisible {
            preview.orderOut(nil)
        } else {
            preview.makeKeyAndOrderFront(nil)
        }
    }

    private func makePanel() -> PalettePanel {
        let panel = PalettePanel(
            contentRect: NSRect(x: 0, y: 0, width: PaletteLayout.width(for: .search), height: PaletteLayout.searchHeight),
            styleMask: [.borderless, .nonactivatingPanel, .fullSizeContentView],
            backing: .buffered,
            defer: true
        )
        panel.level = .floating
        panel.collectionBehavior = [.canJoinAllSpaces, .fullScreenAuxiliary, .transient, .ignoresCycle]
        panel.isOpaque = false
        panel.backgroundColor = .clear
        panel.hasShadow = true
        panel.isMovable = false
        panel.hidesOnDeactivate = false
        panel.isReleasedWhenClosed = false
        panel.delegate = self
        panel.quickLook = quickLook
        panel.onCancel = { [weak self] in self?.model.cancel() }
        panel.onClose = { [weak self] in self?.hide() }
        panel.onKey = { [weak self] key in self?.model.handle(key) ?? false }

        let hosting = NSHostingView(rootView: PaletteView(model: model))
        // The controller sizes the window; SwiftUI must never resize it to fit its content.
        hosting.sizingOptions = []
        panel.contentView = hosting
        return panel
    }
}

/// Borderless panels can't become key by default; this one must, to take typing. It's also
/// the Quick Look controller, which Quick Look looks for in the key window's responder chain.
private final class PalettePanel: NSPanel {
    var onCancel: (() -> Void)?
    var onClose: (() -> Void)?
    var onKey: ((PaletteKey) -> Bool)?
    var quickLook: QuickLookSource?

    override var canBecomeKey: Bool { true }
    override var canBecomeMain: Bool { false }

    override func cancelOperation(_ sender: Any?) {
        onCancel?()
    }

    override func performKeyEquivalent(with event: NSEvent) -> Bool {
        if event.modifierFlags.intersection(.deviceIndependentFlagsMask) == .command,
           event.charactersIgnoringModifiers == "w" {
            onClose?()
            return true
        }
        if let key = PaletteKey(event, fieldHasSelection: fieldHasSelection), onKey?(key) == true {
            return true
        }
        return super.performKeyEquivalent(with: event)
    }

    /// Whether the search field has text selected, so ⌘C copies that rather than a path.
    private var fieldHasSelection: Bool {
        guard let editor = firstResponder as? NSTextView else { return false }
        return editor.selectedRange().length > 0
    }

    // Quick Look calls these on the main thread, but they're declared nonisolated.
    override func acceptsPreviewPanelControl(_ panel: QLPreviewPanel!) -> Bool {
        MainActor.assumeIsolated { quickLook?.url != nil }
    }

    override func beginPreviewPanelControl(_ panel: QLPreviewPanel!) {
        MainActor.assumeIsolated { panel.dataSource = quickLook }
    }

    override func endPreviewPanelControl(_ panel: QLPreviewPanel!) {
        MainActor.assumeIsolated {
            panel.dataSource = nil
            // Hand the keyboard back to the search field once Quick Look closes.
            if isVisible { makeKey() }
        }
    }
}

/// The one file Quick Look shows.
private final class QuickLookSource: NSObject, QLPreviewPanelDataSource {
    var url: URL?

    func numberOfPreviewItems(in panel: QLPreviewPanel!) -> Int {
        url == nil ? 0 : 1
    }

    func previewPanel(_ panel: QLPreviewPanel!, previewItemAt index: Int) -> (any QLPreviewItem)! {
        url as NSURL?
    }
}

/// The ⌘ shortcuts the palette's modes act on, decoded from a key event.
enum PaletteKey {
    /// ⌘↩: the other of paste/copy, or reveal in Finder.
    case alternateOpen
    /// ⌘P
    case togglePin
    /// ⌘⌫
    case delete
    /// ⌘C, only when no text is selected in the field (otherwise it copies that text).
    case copyPath
    /// ⌘Y
    case quickLook

    init?(_ event: NSEvent, fieldHasSelection: Bool) {
        guard event.modifierFlags.intersection(.deviceIndependentFlagsMask) == .command else { return nil }
        switch Int(event.keyCode) {
        case kVK_Return, kVK_ANSI_KeypadEnter: self = .alternateOpen
        case kVK_Delete: self = .delete
        default:
            switch event.charactersIgnoringModifiers {
            case "p": self = .togglePin
            case "y": self = .quickLook
            case "c" where !fieldHasSelection: self = .copyPath
            default: return nil
            }
        }
    }
}

/// Sizes shared by the views and the controller's size calculation.
enum PaletteLayout {
    static let searchHeight: CGFloat = 60
    static let rowHeight: CGFloat = 44
    /// Calculator results get a taller row for their large answer.
    static let calculatorRowHeight: CGFloat = 64
    static let messageHeight: CGFloat = 36
    static let footerHeight: CGFloat = 30
    static let listPadding: CGFloat = 8
    static let maxVisibleRows = 8
    static let cornerRadius: CGFloat = 26
    /// Clipboard history shows a list and a preview side by side, at a fixed size.
    static let clipboardSize = CGSize(width: 820, height: 470)

    static func width(for mode: PaletteMode) -> CGFloat {
        mode == .clipboard ? clipboardSize.width : 680
    }

    @MainActor
    static func size(for model: PaletteModel) -> CGSize {
        let message = model.message != nil ? messageHeight : 0
        switch model.mode {
        case .search:
            var height = searchHeight + message
            if !model.results.isEmpty {
                let rows = model.results.prefix(maxVisibleRows).reduce(0) { $0 + rowHeight(for: $1) }
                height += 1 + listPadding * 2 + rows
            }
            return CGSize(width: width(for: .search), height: height)
        case .clipboard:
            return clipboardSize
        case .files:
            var height = searchHeight + message
            let rows = min(model.files.results.count, maxVisibleRows)
            if rows > 0 {
                // A divider above the list and another above the footer.
                height += 1 + listPadding * 2 + CGFloat(rows) * rowHeight + 1 + footerHeight
            }
            return CGSize(width: width(for: .files), height: height)
        }
    }

    static func rowHeight(for result: SearchResult) -> CGFloat {
        result.kind == .calculator ? calculatorRowHeight : rowHeight
    }
}

// MARK: - Model

@MainActor
@Observable
final class PaletteModel {
    private(set) var mode: PaletteMode = .search
    var query = "" {
        didSet {
            if query != oldValue { refresh() }
        }
    }
    private(set) var results: [SearchResult] = []
    var selection = 0
    /// A result that needs confirming; ⏎ again runs it, Esc backs out.
    private(set) var confirming: SearchResult?
    private(set) var error: String?
    /// Shown briefly after copying a calculator result, before the palette closes.
    private(set) var copied: String?
    /// Bumped on every show, so the search field takes focus and selects its text.
    private(set) var focusRequest = 0
    /// Bumped when a completion fills the field, so the cursor moves to the end.
    private(set) var cursorToEndRequest = 0

    let clipboard: ClipboardBrowser
    let files: FileBrowser

    @ObservationIgnored var onClose: (() -> Void)?
    /// Shows the palette again, e.g. to report a paste that failed after it closed.
    @ObservationIgnored var onReopen: (() -> Void)?
    @ObservationIgnored var onQuickLook: ((URL) -> Void)?
    @ObservationIgnored private let core: Core
    @ObservationIgnored private let settings: LauncherSettings
    /// Where the mouse was when it last picked a row: rows scrolling under a still pointer
    /// shouldn't steal the keyboard selection.
    @ObservationIgnored private var lastHoverLocation: NSPoint?
    @ObservationIgnored private var closeTask: Task<Void, Never>?

    private static let limit: UInt32 = 20
    private static let copiedVisibleFor = Duration.milliseconds(600)

    init(core: Core, settings: LauncherSettings) {
        self.core = core
        self.settings = settings
        clipboard = ClipboardBrowser(core: core)
        files = FileBrowser(core: core)
    }

    var clipboardConfig: ClipboardConfig { settings.config.clipboard }
    var fileScopes: [String] { settings.config.files.scopes }

    /// The line between the search field and the results, if any.
    var message: PaletteMessage? {
        if let copied {
            return .copied(copied)
        }
        if let error = error ?? clipboard.error ?? files.error {
            return .error(error)
        }
        switch mode {
        case .search:
            if let confirming {
                return .confirm(confirming.title)
            }
            if results.isEmpty, !query.trimmingCharacters(in: .whitespaces).isEmpty {
                return .noResults(query)
            }
        case .clipboard:
            break
        case .files:
            if query.trimmingCharacters(in: .whitespaces).isEmpty {
                return .info("Type to search files")
            }
            if files.results.isEmpty {
                return files.isSearching ? .searching : .info(noFilesMessage)
            }
        }
        return nil
    }

    private var noFilesMessage: String {
        let names = fileScopes.map { ($0 as NSString).lastPathComponent }
        return names.isEmpty
            ? "No folders to search. Add some in Settings → File Search."
            : "No files found in \(names.formatted(.list(type: .and)))"
    }

    var selectedResult: SearchResult? {
        results.indices.contains(selection) ? results[selection] : nil
    }

    // MARK: Showing and modes

    func prepareForShow(mode: PaletteMode) {
        if mode == self.mode, mode == .search {
            refresh()
        } else {
            enter(mode)
        }
        focusRequest += 1
        lastHoverLocation = NSEvent.mouseLocation
    }

    /// Switches mode in place, starting from an empty query.
    func enter(_ mode: PaletteMode) {
        self.mode = mode
        confirming = nil
        error = nil
        copied = nil
        if query.isEmpty {
            refresh()
        } else {
            query = ""
        }
        focusRequest += 1
    }

    func didHide() {
        closeTask?.cancel()
        closeTask = nil
        confirming = nil
        error = nil
        copied = nil
    }

    /// Re-runs the current mode's query, if it's `mode` (e.g. the app index or clipboard
    /// history changed underneath).
    func refresh(ifShowing mode: PaletteMode) {
        if self.mode == mode { refresh() }
    }

    private func refresh() {
        switch mode {
        case .search: search()
        case .clipboard: clipboard.refresh(query: query)
        case .files: files.search(query)
        }
    }

    private func search() {
        results = core.search(query: query, limit: Self.limit)
        selection = 0
        confirming = nil
        error = nil
    }

    // MARK: Keys

    func move(by offset: Int) {
        switch mode {
        case .search:
            guard !results.isEmpty else { return }
            confirming = nil
            selection = (selection + offset + results.count) % results.count
        case .clipboard: clipboard.move(by: offset)
        case .files: files.move(by: offset)
        }
    }

    func hover(_ index: Int) {
        let location = NSEvent.mouseLocation
        guard location != lastHoverLocation else { return }
        lastHoverLocation = location
        switch mode {
        case .search:
            if selection != index {
                selection = index
                confirming = nil
            }
        case .clipboard: clipboard.selection = index
        case .files: files.selection = index
        }
    }

    /// ⏎
    func openSelected() {
        switch mode {
        case .search:
            if let result = selectedResult { open(result) }
        case .clipboard:
            pasteSelected(paste: clipboardConfig.pasteOnOpen)
        case .files:
            openSelectedFile()
        }
    }

    /// The ⌘ shortcuts; returns whether the key did something in this mode.
    func handle(_ key: PaletteKey) -> Bool {
        switch (mode, key) {
        case (.search, .alternateOpen):
            openSelected()
        case (.clipboard, .alternateOpen):
            pasteSelected(paste: !clipboardConfig.pasteOnOpen)
        case (.clipboard, .togglePin):
            clipboard.togglePinSelected()
        case (.clipboard, .delete):
            clipboard.deleteSelected()
        case (.files, .alternateOpen):
            revealSelectedFile()
        case (.files, .copyPath):
            copySelectedPath()
        case (.files, .quickLook):
            guard let file = files.selected else { return false }
            onQuickLook?(URL(fileURLWithPath: file.path))
        default:
            return false
        }
        return true
    }

    /// Tab: fill in the selected result's completion (e.g. a quicklink's keyword). Results
    /// without one ignore Tab, so it never surprises by rewriting the query.
    func completeSelected() {
        guard mode == .search, let completion = selectedResult?.completion else { return }
        complete(with: completion)
    }

    func open(_ result: SearchResult) {
        guard copied == nil else { return }
        if let completion = result.completion {
            complete(with: completion)
            return
        }
        if result.needsConfirmation, confirming?.id != result.id {
            confirming = result
            return
        }
        do {
            let outcome = try core.openResult(id: result.id, query: query)
            confirming = nil
            error = nil
            switch outcome {
            case .done:
                onClose?()
            case .copied(let text):
                showCopied(text)
            case .showMode(let mode):
                enter(mode)
            }
        } catch {
            confirming = nil
            self.error = error.userMessage
        }
    }

    /// Esc: back out of a confirmation or error, then clear the query, then leave clipboard
    /// or file mode for search, then close.
    func cancel() {
        if confirming != nil {
            confirming = nil
        } else if error != nil || clipboard.error != nil || files.error != nil {
            error = nil
            clipboard.error = nil
            files.error = nil
        } else if !query.isEmpty {
            query = ""
        } else if mode != .search {
            enter(.search)
        } else {
            onClose?()
        }
    }

    // MARK: Clipboard and file actions

    /// Closes first, so the paste lands in the app that was in front.
    func pasteSelected(paste: Bool) {
        guard let item = clipboard.selected else { return }
        onClose?()
        do {
            try core.pasteClipboardItem(id: item.id, paste: paste)
        } catch {
            onReopen?()
            self.error = error.userMessage
        }
    }

    func openSelectedFile() {
        guard let file = files.selected else { return }
        do {
            try core.openFile(path: file.path)
            onClose?()
        } catch {
            self.error = error.userMessage
        }
    }

    func revealSelectedFile() {
        guard let file = files.selected else { return }
        do {
            try core.revealFile(path: file.path)
            onClose?()
        } catch {
            self.error = error.userMessage
        }
    }

    /// Copying a path is a UI convenience, so it stays in the shell.
    func copySelectedPath() {
        guard let file = files.selected else { return }
        NSPasteboard.general.clearContents()
        NSPasteboard.general.setString(file.path, forType: .string)
        showCopied(file.path)
    }

    private func complete(with completion: String) {
        query = completion
        cursorToEndRequest += 1
    }

    private func showCopied(_ text: String) {
        copied = text
        closeTask?.cancel()
        closeTask = Task { [weak self] in
            try? await Task.sleep(for: Self.copiedVisibleFor)
            guard !Task.isCancelled else { return }
            self?.onClose?()
        }
    }
}

enum PaletteMessage: Equatable {
    case copied(String)
    case confirm(String)
    case error(String)
    case noResults(String)
    case info(String)
    case searching
}

// MARK: - Views

private struct PaletteView: View {
    let model: PaletteModel

    var body: some View {
        @Bindable var model = model

        VStack(spacing: 0) {
            HStack(spacing: 12) {
                Image(systemName: model.mode.symbol)
                    .font(.system(size: 20, weight: .medium))
                    .foregroundStyle(.secondary)
                if let title = model.mode.chipTitle {
                    Text(title)
                        .font(.system(size: 13, weight: .semibold))
                        .padding(.horizontal, 9)
                        .padding(.vertical, 4)
                        .background(.primary.opacity(0.1), in: .capsule)
                }
                SearchField(
                    text: $model.query,
                    placeholder: model.mode.placeholder,
                    focusRequest: model.focusRequest,
                    cursorToEndRequest: model.cursorToEndRequest
                ) { command in
                    switch command {
                    case .up: model.move(by: -1)
                    case .down: model.move(by: 1)
                    case .submit: model.openSelected()
                    case .alternateSubmit: _ = model.handle(.alternateOpen)
                    case .complete: model.completeSelected()
                    case .cancel: model.cancel()
                    }
                }
            }
            .padding(.horizontal, 20)
            .frame(height: PaletteLayout.searchHeight)

            if let message = model.message {
                MessageRow(message: message)
                    .frame(height: PaletteLayout.messageHeight)
            }

            switch model.mode {
            case .search:
                if !model.results.isEmpty {
                    Divider().padding(.horizontal, 14)
                    ResultsList(model: model)
                }
            case .clipboard:
                Divider().padding(.horizontal, 14)
                ClipboardModeView(model: model)
            case .files:
                if !model.files.results.isEmpty {
                    Divider().padding(.horizontal, 14)
                    FilesModeView(model: model)
                }
            }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .top)
        .glassEffect(.regular, in: .rect(cornerRadius: PaletteLayout.cornerRadius))
    }
}

extension PaletteMode {
    var symbol: String {
        switch self {
        case .search: "magnifyingglass"
        case .clipboard: "doc.on.clipboard"
        case .files: "doc.text.magnifyingglass"
        }
    }

    /// The label shown in front of the field outside search mode.
    var chipTitle: String? {
        switch self {
        case .search: nil
        case .clipboard: "Clipboard"
        case .files: "Files"
        }
    }

    var placeholder: String {
        switch self {
        case .search: "Search apps and actions, or calculate"
        case .clipboard: "Search clipboard history"
        case .files: "Search files"
        }
    }
}

private struct MessageRow: View {
    let message: PaletteMessage

    var body: some View {
        HStack(spacing: 8) {
            switch message {
            case .copied(let text):
                Image(systemName: "checkmark.circle.fill").foregroundStyle(.green)
                Text("Copied").fontWeight(.semibold)
                Text(text).foregroundStyle(.secondary).lineLimit(1).truncationMode(.middle)
            case .confirm(let title):
                Image(systemName: "exclamationmark.triangle.fill").foregroundStyle(.orange)
                Text("\(title)?").fontWeight(.semibold)
                Text("Press ↩ to confirm, ⎋ to cancel").foregroundStyle(.secondary)
            case .error(let text):
                Image(systemName: "xmark.octagon.fill").foregroundStyle(.red)
                Text(text).lineLimit(1)
            case .noResults(let query):
                Text("No results for “\(query)”").foregroundStyle(.secondary)
            case .info(let text):
                Text(text).foregroundStyle(.secondary).lineLimit(1)
            case .searching:
                ProgressView().controlSize(.small)
                Text("Searching…").foregroundStyle(.secondary)
            }
            Spacer(minLength: 0)
        }
        .font(.callout)
        .padding(.horizontal, 22)
    }
}

private struct ResultsList: View {
    let model: PaletteModel

    var body: some View {
        ScrollViewReader { proxy in
            ScrollView {
                LazyVStack(spacing: 0) {
                    ForEach(Array(model.results.enumerated()), id: \.element.id) { index, result in
                        ResultRow(result: result, isSelected: index == model.selection)
                            .id(result.id)
                            .contentShape(.rect)
                            .onContinuousHover { phase in
                                if case .active = phase { model.hover(index) }
                            }
                            .onTapGesture {
                                model.selection = index
                                model.open(result)
                            }
                    }
                }
                .padding(.vertical, PaletteLayout.listPadding)
                .padding(.horizontal, 8)
            }
            .scrollIndicators(.never)
            .onChange(of: model.selection) {
                if let id = model.selectedResult?.id {
                    proxy.scrollTo(id)
                }
            }
        }
    }
}

private struct ResultRow: View {
    let result: SearchResult
    let isSelected: Bool

    var body: some View {
        HStack(spacing: 12) {
            if result.kind == .calculator {
                calculation
            } else {
                icon
                    .frame(width: 28, height: 28)
                VStack(alignment: .leading, spacing: 1) {
                    Text(result.title)
                        .font(.system(size: 14, weight: .medium))
                        .lineLimit(1)
                    if !result.subtitle.isEmpty {
                        Text(result.subtitle)
                            .font(.system(size: 11))
                            .foregroundStyle(.secondary)
                            .lineLimit(1)
                    }
                }
            }
            Spacer(minLength: 0)
            if isSelected {
                Text(hint.label)
                    .font(.caption)
                    .foregroundStyle(.secondary)
                Text(hint.key)
                    .font(.caption.weight(.semibold))
                    .foregroundStyle(.secondary)
            }
        }
        .padding(.horizontal, 10)
        .frame(height: PaletteLayout.rowHeight(for: result))
        .background {
            if isSelected {
                RoundedRectangle(cornerRadius: 10).fill(.primary.opacity(0.1))
            }
        }
    }

    /// The large answer with the expression underneath.
    private var calculation: some View {
        VStack(alignment: .leading, spacing: 2) {
            Text(result.title)
                .font(.system(size: 28, weight: .semibold).monospacedDigit())
                .lineLimit(1)
                .minimumScaleFactor(0.5)
            if !result.subtitle.isEmpty {
                Text(result.subtitle)
                    .font(.system(size: 11))
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
            }
        }
        .padding(.leading, 4)
    }

    /// What ⏎ will do with this result.
    private var hint: (label: String, key: String) {
        if result.completion != nil {
            return ("Complete", "⇥")
        }
        if result.id.hasPrefix("mode:") {
            return ("Show", "↩")
        }
        switch result.kind {
        case .application, .quicklink: return ("Open", "↩")
        case .calculator: return ("Copy", "↩")
        case .systemAction, .command: return ("Run", "↩")
        }
    }

    @ViewBuilder
    private var icon: some View {
        if let path = result.path {
            Image(nsImage: AppIcons.icon(forPath: path))
                .resizable()
                .aspectRatio(contentMode: .fit)
        } else {
            Image(systemName: result.symbol ?? "gearshape")
                .font(.system(size: 15, weight: .medium))
                .frame(width: 26, height: 26)
                .background(.primary.opacity(0.08), in: .rect(cornerRadius: 6))
        }
    }
}

/// An unbezeled AppKit text field: SwiftUI's can't select all on demand or hand over the
/// arrow, return and escape keys before the field acts on them.
private struct SearchField: NSViewRepresentable {
    @Binding var text: String
    let placeholder: String
    let focusRequest: Int
    let cursorToEndRequest: Int
    let onCommand: (Command) -> Void

    enum Command { case up, down, submit, alternateSubmit, complete, cancel }

    func makeNSView(context: Context) -> NSTextField {
        let field = NSTextField()
        field.isBordered = false
        field.drawsBackground = false
        field.focusRingType = .none
        field.font = .systemFont(ofSize: 22)
        field.placeholderString = placeholder
        field.cell?.wraps = false
        field.cell?.isScrollable = true
        field.delegate = context.coordinator
        return field
    }

    func updateNSView(_ field: NSTextField, context: Context) {
        context.coordinator.parent = self
        if field.stringValue != text {
            field.stringValue = text
        }
        if field.placeholderString != placeholder {
            field.placeholderString = placeholder
        }
        if context.coordinator.focusRequest != focusRequest {
            context.coordinator.focusRequest = focusRequest
            // The window may not be key yet on the same pass as showing it.
            DispatchQueue.main.async {
                field.window?.makeFirstResponder(field)
                field.selectText(nil)
            }
        }
        if context.coordinator.cursorToEndRequest != cursorToEndRequest {
            context.coordinator.cursorToEndRequest = cursorToEndRequest
            let end = (field.stringValue as NSString).length
            field.currentEditor()?.selectedRange = NSRange(location: end, length: 0)
        }
    }

    func makeCoordinator() -> Coordinator {
        Coordinator(parent: self)
    }

    @MainActor
    final class Coordinator: NSObject, NSTextFieldDelegate {
        var parent: SearchField
        var focusRequest = 0
        var cursorToEndRequest = 0

        init(parent: SearchField) {
            self.parent = parent
        }

        func controlTextDidChange(_ notification: Notification) {
            guard let field = notification.object as? NSTextField else { return }
            parent.text = field.stringValue
        }

        /// ⌃N/⌃P arrive as moveDown:/moveUp: through the standard key bindings. ⌘↩ is
        /// normally caught as a key equivalent first; this is the fallback.
        func control(_ control: NSControl, textView: NSTextView, doCommandBy selector: Selector) -> Bool {
            switch selector {
            case #selector(NSResponder.moveUp(_:)): parent.onCommand(.up)
            case #selector(NSResponder.moveDown(_:)): parent.onCommand(.down)
            case #selector(NSResponder.insertNewline(_:)):
                let command = NSApp.currentEvent?.modifierFlags.contains(.command) == true
                parent.onCommand(command ? .alternateSubmit : .submit)
            case #selector(NSResponder.insertTab(_:)): parent.onCommand(.complete)
            case #selector(NSResponder.cancelOperation(_:)): parent.onCommand(.cancel)
            default: return false
            }
            return true
        }
    }
}
