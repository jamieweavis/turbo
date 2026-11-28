import AppKit
import Carbon.HIToolbox
import Observation
import Quartz
import SwiftUI
import os

/// The launcher palette: a floating Liquid Glass panel with a search field and results.
/// It has six modes (search, clipboard history, file search, picked colours, emoji, camera
/// preview) sharing the one field. Colours and emoji show a grid rather than a list.
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
    private let core: Core
    /// ⌘, from the palette: it hides and the settings window opens.
    var onShowSettings: (() -> Void)?

    init(core: Core, settings: LauncherSettings) {
        self.core = core
        model = PaletteModel(core: core, settings: settings)
        super.init()
        model.onClose = { [weak self] in self?.hide() }
        model.onReopen = { [weak self] in
            guard let self else { return }
            self.show(mode: self.model.mode)
        }
        model.onQuickLook = { [weak self] url in self?.toggleQuickLook(url) }
        model.onShowSettings = { [weak self] in self?.showSettings() }
    }

    var isVisible: Bool { panel.isVisible }

    /// The panel's frame on screen, for `just screenshots`.
    var panelFrame: NSRect { panel.frame }

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
        core.setPaletteVisible(visible: true)
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
        core.setPaletteVisible(visible: false)
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

    private func showSettings() {
        onShowSettings?()
        if panel.isVisible { hide() }
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
        // Open settings while the palette is still key, so the activation request comes
        // from a key window; the palette hides when settings takes key.
        panel.onShowSettings = { [weak self] in self?.showSettings() }
        panel.onKey = { [weak self] key in self?.model.handle(key) ?? false }

        let hosting = NSHostingView(rootView: PaletteView(model: model))
        // The controller sizes the window; SwiftUI must never resize it to fit its content.
        hosting.sizingOptions = []
        // The glass's backdrop covers the whole frame, faintly tinting the corners outside its
        // rounded shape (and squaring off the window shadow); clip it to the rounded shape.
        hosting.wantsLayer = true
        hosting.layer?.cornerRadius = PaletteLayout.cornerRadius
        hosting.layer?.cornerCurve = .continuous
        hosting.layer?.masksToBounds = true
        panel.contentView = hosting
        return panel
    }
}

/// Borderless panels can't become key by default; this one must, to take typing. It's also
/// the Quick Look controller, which Quick Look looks for in the key window's responder chain.
private final class PalettePanel: NSPanel {
    var onCancel: (() -> Void)?
    var onClose: (() -> Void)?
    var onShowSettings: (() -> Void)?
    var onKey: ((PaletteKey) -> Bool)?
    var quickLook: QuickLookSource?

    override var canBecomeKey: Bool { true }
    override var canBecomeMain: Bool { false }

    override func cancelOperation(_ sender: Any?) {
        onCancel?()
    }

    /// Keys only land here when nothing in the panel has focus (the camera's read-only
    /// field): Esc still backs out, and typing is dropped without a beep.
    override func keyDown(with event: NSEvent) {
        guard firstResponder === self else {
            super.keyDown(with: event)
            return
        }
        if Int(event.keyCode) == kVK_Escape { onCancel?() }
    }

    override func performKeyEquivalent(with event: NSEvent) -> Bool {
        if event.modifierFlags.intersection(.deviceIndependentFlagsMask) == .command {
            switch event.charactersIgnoringModifiers {
            case "w":
                onClose?()
                return true
            case ",":
                onShowSettings?()
                return true
            default: break
            }
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
    /// ⌘K: open or close the actions menu.
    case actions

    init?(_ event: NSEvent, fieldHasSelection: Bool) {
        guard event.modifierFlags.intersection(.deviceIndependentFlagsMask) == .command else { return nil }
        switch Int(event.keyCode) {
        case kVK_Return, kVK_ANSI_KeypadEnter: self = .alternateOpen
        case kVK_Delete: self = .delete
        default:
            switch event.charactersIgnoringModifiers {
            case "p": self = .togglePin
            case "y": self = .quickLook
            case "k": self = .actions
            case "c" where !fieldHasSelection: self = .copyPath
            default: return nil
            }
        }
    }
}

/// Sizes shared by the views and the controller's size calculation.
enum PaletteLayout {
    static let searchHeight: CGFloat = 64
    static let rowHeight: CGFloat = 42
    /// The "Suggestions"/"Results" label above the search results.
    static let sectionHeaderHeight: CGFloat = 28
    /// The settings button and primary action floating over the bottom of the results.
    static let actionBarHeight: CGFloat = 56
    /// Calculator results get a taller row for their large answer.
    static let calculatorRowHeight: CGFloat = 64
    static let messageHeight: CGFloat = 36
    static let footerHeight: CGFloat = 30
    static let listPadding: CGFloat = 8
    static let maxVisibleRows = 8
    /// Fill behind the selected row in the result, clipboard and file lists.
    static let selectionFill = Color.appearance(light: .black.opacity(0.09), dark: .white.opacity(0.34))
    static let cornerRadius: CGFloat = 26
    /// Clipboard history shows a list and a preview side by side, at a fixed size.
    static let clipboardSize = CGSize(width: 820, height: 470)
    /// Emoji fill a fixed-size grid, so the palette doesn't jump while typing.
    static let emojiSize = CGSize(width: 720, height: 540)
    static let emojiColumns = 8
    static let emojiSpacing: CGFloat = 10
    static let emojiPadding: CGFloat = 16
    /// Square tiles: the width each of the flexible columns gets.
    static let emojiCellHeight = (emojiSize.width - emojiPadding * 2 - emojiSpacing * CGFloat(emojiColumns - 1))
        / CGFloat(emojiColumns)
    static let colorColumns = 5
    static let colorCellHeight: CGFloat = 124
    /// Colour rows shown before the grid scrolls; the half row hints there's more.
    static let maxVisibleColorRows = 2.5
    static let gridSpacing: CGFloat = 6
    static let gridPadding: CGFloat = 12
    static let gridHeaderHeight: CGFloat = 26

    static func width(for mode: PaletteMode) -> CGFloat {
        switch mode {
        case .clipboard: clipboardSize.width
        case .emoji: emojiSize.width
        case .search, .files, .colors, .camera: 680
        }
    }

    /// Room for the actions menu above whatever it floats over, so it never gets clipped.
    @MainActor
    static func size(for model: PaletteModel) -> CGSize {
        var size = contentSize(for: model)
        if let menu = model.actionsMenu {
            let needed = searchHeight + listPadding + ActionsMenuLayout.fullHeight(for: menu)
                + ActionsMenuLayout.bottomInset(for: model.mode)
            size.height = max(size.height, needed)
        }
        return size
    }

    @MainActor
    private static func contentSize(for model: PaletteModel) -> CGSize {
        let message = model.message != nil ? messageHeight : 0
        switch model.mode {
        case .search:
            var height = searchHeight + message
            if !model.results.isEmpty {
                let rows = model.results.prefix(maxVisibleRows).reduce(0) { $0 + rowHeight(for: $1) }
                height += sectionHeaderHeight + rows + actionBarHeight
            }
            return CGSize(width: width(for: .search), height: height)
        case .clipboard:
            return clipboardSize
        case .emoji:
            return model.emoji.isEmpty ? CGSize(width: emojiSize.width, height: searchHeight + message) : emojiSize
        case .files:
            var height = searchHeight + message
            let rows = min(model.files.results.count, maxVisibleRows)
            if rows > 0 {
                // A divider above the list and another above the footer.
                height += 1 + listPadding * 2 + CGFloat(rows) * rowHeight + 1 + footerHeight
            }
            return CGSize(width: width(for: .files), height: height)
        case .colors:
            var height = searchHeight + message
            let count = model.colors.colors.count
            if count > 0 {
                let rows = min((Double(count) / Double(colorColumns)).rounded(.up), maxVisibleColorRows)
                let grid = rows * colorCellHeight + (rows.rounded(.up) - 1) * gridSpacing
                height += 1 + listPadding * 2 + grid.rounded() + 1 + footerHeight
            }
            return CGSize(width: width(for: .colors), height: height)
        case .camera:
            return CGSize(width: width(for: .camera), height: searchHeight + message + cameraFeedHeight + cameraInset)
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
    /// Whether `mode` was picked from search (so Esc goes back there) rather than opened by
    /// its own hotkey (so Esc closes).
    private var enteredFromSearch = false
    var query = "" {
        didSet {
            if query != oldValue { refresh() }
        }
    }
    private(set) var results: [SearchResult] = []
    var selection = 0
    /// A result that needs confirming; ⏎ again runs it, Esc backs out.
    private(set) var confirming: SearchResult?
    /// ⌘⌫ on several marked rows asks first; ⏎ or ⌘⌫ again deletes them, Esc backs out.
    private(set) var confirmingDelete = false
    private(set) var error: String?
    /// Shown briefly after copying a calculator result, before the palette closes.
    private(set) var copied: String?
    /// Bumped on every show, so the search field takes focus and selects its text.
    private(set) var focusRequest = 0
    /// Bumped when a completion fills the field, so the cursor moves to the end.
    private(set) var cursorToEndRequest = 0
    /// Bumped when the actions menu closes, so typing goes back to the search field.
    private(set) var refocusRequest = 0
    /// The ⌘K menu of everything that can be done with the selection, while it's open.
    private(set) var actionsMenu: ActionsMenu?

    let clipboard: ClipboardBrowser
    let files: FileBrowser
    let colors: ColorBrowser
    let emoji: EmojiBrowser
    let camera: CameraPreview

    @ObservationIgnored var onClose: (() -> Void)?
    /// Shows the palette again, e.g. to report a paste that failed after it closed.
    @ObservationIgnored var onReopen: (() -> Void)?
    @ObservationIgnored var onQuickLook: ((URL) -> Void)?
    /// The action bar's settings button.
    @ObservationIgnored var onShowSettings: (() -> Void)?
    /// Shows the colour picker's magnifier (after the palette has closed).
    @ObservationIgnored var onPickColor: (() -> Void)?
    /// A Keep Awake command turned it on or off here.
    @ObservationIgnored var onKeepAwakeChanged: ((Bool) -> Void)?
    /// An action confirmed what it did (Wi-Fi off, the new volume); the palette has closed.
    @ObservationIgnored var onStatus: ((Status) -> Void)?
    /// A colour was deleted from the history here.
    @ObservationIgnored var onColorsChanged: (() -> Void)?
    /// Called after an emoji is pasted or copied, so its recently used list can refresh.
    @ObservationIgnored var onEmojiUsed: (() -> Void)?
    @ObservationIgnored let core: Core
    @ObservationIgnored private let settings: LauncherSettings
    /// Where the mouse was when it last picked a row: rows scrolling under a still pointer
    /// shouldn't steal the keyboard selection.
    @ObservationIgnored private var lastHoverLocation: NSPoint?
    @ObservationIgnored private var closeTask: Task<Void, Never>?

    private static let limit: UInt32 = 200
    private static let copiedVisibleFor = Duration.milliseconds(600)

    init(core: Core, settings: LauncherSettings) {
        self.core = core
        self.settings = settings
        clipboard = ClipboardBrowser(core: core)
        files = FileBrowser(core: core)
        colors = ColorBrowser(core: core)
        emoji = EmojiBrowser(core: core)
        camera = CameraPreview(core: core)
    }

    var clipboardConfig: ClipboardConfig { settings.config.clipboard }
    var snippetsPasteOnOpen: Bool { settings.config.snippets.pasteOnOpen }
    var fileScopes: [String] { settings.config.files.scopes }
    var colorFormat: ColorFormat { settings.config.colorPicker.format }
    var emojiPasteOnOpen: Bool { settings.config.emoji.pasteOnOpen }
    var cameraMirrored: Bool { settings.config.camera.mirror }

    /// The line between the search field and the results, if any.
    var message: PaletteMessage? {
        if let copied {
            return .copied(copied)
        }
        if let error = error ?? clipboard.error ?? files.error ?? colors.error ?? emoji.error {
            return .error(error)
        }
        if confirmingDelete, let count = multiSelect?.targetIndices.count {
            return .confirm("Delete \(count) \(mode == .colors ? "colours" : "items")")
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
        case .colors:
            if colors.colors.isEmpty {
                return query.trimmingCharacters(in: .whitespaces).isEmpty
                    ? .info("No colours yet. Pick one with Pick Colour.")
                    : .noResults(query)
            }
        case .emoji:
            if emoji.isEmpty {
                return .noResults(query)
            }
        case .camera:
            break
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

    /// Switches mode in place, starting from an empty query. The camera runs only while its
    /// mode is showing.
    func enter(_ mode: PaletteMode, fromSearch: Bool = false) {
        self.mode = mode
        if mode == .camera { camera.start() } else { camera.stop() }
        enteredFromSearch = fromSearch
        actionsMenu = nil
        confirming = nil
        clipboard.clearMarks()
        colors.clearMarks()
        error = nil
        copied = nil
        if query.isEmpty {
            refresh()
        } else {
            query = ""
        }
        if mode == .clipboard { clipboard.selectFirst() }
        focusRequest += 1
    }

    func didHide() {
        camera.stop()
        closeTask?.cancel()
        closeTask = nil
        actionsMenu = nil
        confirming = nil
        error = nil
        copied = nil
    }

    /// Re-runs the current mode's query, if it's `mode` (e.g. the app index or clipboard
    /// history changed underneath).
    func refresh(ifShowing mode: PaletteMode) {
        if self.mode == mode { refresh() }
    }

    func refresh() {
        confirmingDelete = false
        switch mode {
        case .search: search()
        case .clipboard: clipboard.refresh(query: query)
        case .files: files.search(query)
        case .colors: colors.refresh(query: query)
        case .emoji: emoji.refresh(query: query)
        case .camera: break
        }
    }

    private func search() {
        results = core.search(query: query, limit: Self.limit)
        selection = 0
        confirming = nil
        error = nil
    }

    // MARK: Keys

    /// ↑/↓ (a row up or down in grids).
    func move(by offset: Int) {
        confirmingDelete = false
        defer { multiSelect?.cursorMoved() }
        switch mode {
        case .search:
            guard !results.isEmpty else { return }
            confirming = nil
            selection = (selection + offset + results.count) % results.count
        case .clipboard: clipboard.move(by: offset)
        case .files: files.move(by: offset)
        case .colors: colors.move(offset < 0 ? .up : .down)
        case .emoji: emoji.move(offset < 0 ? .up : .down)
        case .camera: break
        }
    }

    /// ⌘↑/⌘↓: the first or last row (or tile).
    func jump(toEnd: Bool) {
        confirmingDelete = false
        defer { multiSelect?.cursorMoved() }
        setSelection(toEnd: toEnd)
    }

    /// ⇧⌘↑/⇧⌘↓ where rows can be marked: marks everything from here to the top or bottom.
    func extendMarks(toEnd: Bool) {
        guard let browser = multiSelect else { return }
        confirmingDelete = false
        let previous = browser.selection
        setSelection(toEnd: toEnd)
        if browser.selection != previous { browser.extendMarks(from: previous) }
    }

    private func setSelection(toEnd: Bool) {
        func index(in count: Int) -> Int { toEnd ? max(count - 1, 0) : 0 }
        switch mode {
        case .search:
            confirming = nil
            selection = index(in: results.count)
        case .clipboard: clipboard.selection = index(in: clipboard.items.count)
        case .files: files.selection = index(in: files.results.count)
        case .colors: colors.selection = index(in: colors.colors.count)
        case .emoji: emoji.selection = index(in: emoji.count)
        case .camera: break
        }
    }

    /// ←/→ in grid modes; elsewhere they move the cursor in the field.
    func moveSideways(by offset: Int) {
        confirmingDelete = false
        defer { multiSelect?.cursorMoved() }
        let direction: GridDirection = offset < 0 ? .left : .right
        switch mode {
        case .colors: colors.move(direction)
        case .emoji: emoji.move(direction)
        case .search, .clipboard, .files, .camera: break
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
        case .colors: colors.selection = index
        case .emoji: emoji.selection = index
        case .camera: break
        }
    }

    /// ⏎
    func openSelected() {
        if confirmingDelete {
            deleteTargets(confirmed: true)
            return
        }
        switch mode {
        case .search:
            if let result = selectedResult { open(result) }
        case .clipboard:
            pasteSelected(paste: clipboardConfig.pasteOnOpen)
        case .files:
            openSelectedFile()
        case .colors:
            copySelectedColor()
        case .emoji:
            insertSelectedEmoji(paste: emojiPasteOnOpen)
        case .camera:
            break
        }
    }

    /// The ⌘ shortcuts; returns whether the key did something in this mode. Any of them
    /// closes the actions menu first, since they act on the selection behind it.
    func handle(_ key: PaletteKey) -> Bool {
        if key != .delete { confirmingDelete = false }
        if key == .actions {
            toggleActions()
            return true
        }
        closeActions()
        switch (mode, key) {
        case (.search, .alternateOpen):
            // The result's second action, as listed in the actions menu; ↩ for the rest.
            if let action = actions().first(where: { $0.keys == ActionKeys.alternate }) {
                action.perform()
            } else {
                openSelected()
            }
        case (.clipboard, .alternateOpen):
            pasteSelected(paste: !clipboardConfig.pasteOnOpen)
        case (.clipboard, .togglePin):
            clipboard.togglePinTargets()
        case (.clipboard, .delete), (.colors, .delete):
            deleteTargets()
        case (.files, .alternateOpen):
            revealSelectedFile()
        case (.files, .copyPath):
            copySelectedPath()
        case (.files, .quickLook):
            guard let file = files.selected else { return false }
            onQuickLook?(URL(fileURLWithPath: file.path))
        case (.colors, .alternateOpen):
            pickColor()
        case (.emoji, .alternateOpen):
            insertSelectedEmoji(paste: !emojiPasteOnOpen)
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
        if result.kind == .snippet {
            openSnippet(result, paste: snippetsPasteOnOpen)
            return
        }
        if result.actsOnFrontApp {
            openInFrontApp(result)
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
                enter(mode, fromSearch: true)
            case .pickColor:
                pickColor()
            case .keepAwake(let active):
                onClose?()
                onKeepAwakeChanged?(active)
            case .status(let status):
                onClose?()
                onStatus?(status)
            }
        } catch {
            Logger.palette.error("couldn't open a \(String(describing: result.kind), privacy: .public) result: \(error, privacy: .public)")
            confirming = nil
            self.error = error.userMessage
        }
    }

    /// Esc: close the actions menu, back out of a confirmation or error, then unmark any
    /// marked rows, then clear the query, then leave clipboard, file, colour or emoji mode for
    /// search if it was opened from there, then close.
    func cancel() {
        if actionsMenu != nil {
            closeActions()
        } else if confirmingDelete {
            confirmingDelete = false
        } else if confirming != nil {
            confirming = nil
        } else if error != nil || clipboard.error != nil || files.error != nil || colors.error != nil
                    || emoji.error != nil {
            error = nil
            clipboard.error = nil
            files.error = nil
            colors.error = nil
            emoji.error = nil
        } else if let browser = multiSelect, browser.hasMarks {
            browser.clearMarks()
        } else if !query.isEmpty {
            query = ""
        } else if mode != .search, enteredFromSearch {
            enter(.search)
        } else {
            onClose?()
        }
    }

    // MARK: Marking rows for bulk actions

    /// The current mode's browser, if its rows can be marked.
    private var multiSelect: (any MultiSelecting)? {
        switch mode {
        case .clipboard: clipboard
        case .colors: colors
        case .search, .files, .emoji, .camera: nil
        }
    }

    /// A click on a clipboard row or colour swatch. ⌘-click marks it and ⇧-click marks the
    /// run up to it; once anything is marked, plain clicks mark too rather than open.
    func click(_ index: Int) {
        guard let browser = multiSelect else { return }
        confirmingDelete = false
        browser.selection = index
        let modifiers = NSEvent.modifierFlags.intersection(.deviceIndependentFlagsMask)
        if modifiers.contains(.shift) {
            browser.extendMarks(toClicked: index)
        } else if modifiers.contains(.command) || browser.hasMarks {
            browser.toggleMark(at: index)
        } else {
            openSelected()
        }
    }

    /// Tab and ⇧Tab: mark or unmark the selected row and step to the next (or previous) one, so
    /// repeating it marks a run. In search, Tab fills in the completion instead.
    func tab(by offset: Int) {
        guard let browser = multiSelect else {
            if offset > 0 { completeSelected() }
            return
        }
        confirmingDelete = false
        browser.toggleMark(at: browser.selection)
        if stepWithoutWrapping(offset > 0 ? (mode.isGrid ? .right : .down) : (mode.isGrid ? .left : .up)) {
            browser.cursorMoved()
        }
    }

    /// The actions menu's Select/Deselect: marks the selected row where it is.
    func toggleMarkSelected() {
        guard let browser = multiSelect else { return }
        browser.toggleMark(at: browser.selection)
    }

    /// ⇧ with an arrow: moves the selection and marks everything from where ⇧ went down.
    func extendMarks(_ direction: GridDirection) {
        guard let browser = multiSelect else { return }
        confirmingDelete = false
        let previous = browser.selection
        if stepWithoutWrapping(direction) {
            browser.extendMarks(from: previous)
        }
    }

    func markAll() {
        multiSelect?.markAll()
    }

    func clearMarks() {
        multiSelect?.clearMarks()
    }

    /// Moves the selection unless that would wrap round the end: a marked run shouldn't
    /// suddenly span the whole list. Returns whether it moved.
    private func stepWithoutWrapping(_ direction: GridDirection) -> Bool {
        guard let browser = multiSelect else { return false }
        let previous = browser.selection
        let forward = direction == .down || direction == .right
        switch mode {
        case .clipboard: clipboard.move(by: forward ? 1 : -1)
        case .colors: colors.move(direction)
        case .search, .files, .emoji, .camera: return false
        }
        guard browser.selection != previous, (browser.selection > previous) == forward else {
            browser.selection = previous
            return false
        }
        return true
    }

    /// ⌘⌫: deletes the marked rows, or the selected one. One row goes straight away; several
    /// ask first (`confirmingDelete`), since they can't be brought back.
    func deleteTargets(confirmed: Bool = false) {
        guard let browser = multiSelect else { return }
        if browser.targetIndices.count > 1, !confirmed, !confirmingDelete {
            confirmingDelete = true
            return
        }
        confirmingDelete = false
        switch mode {
        case .clipboard:
            clipboard.deleteTargets()
        case .colors:
            colors.deleteTargets()
            onColorsChanged?()
        case .search, .files, .emoji, .camera:
            break
        }
    }

    // MARK: Clipboard and file actions

    /// Pastes the marked items together (in the order listed), or the selected one. Closes
    /// first, so the paste lands in the app that was in front; a failure reopens with the
    /// marks as they were.
    func pasteSelected(paste: Bool) {
        let ids = clipboard.targets.map(\.id)
        guard !ids.isEmpty else { return }
        let marks = clipboard.marks
        onClose?()
        do {
            try core.pasteClipboardItems(ids: ids, paste: paste)
        } catch {
            Logger.palette.error("couldn't \(paste ? "paste" : "copy") clipboard items: \(error, privacy: .public)")
            onReopen?()
            clipboard.restoreMarks(marks)
            self.error = error.userMessage
        }
    }

    /// What ⏎ will do with `result`, for the action bar and actions menu: a snippet's
    /// depends on the paste setting.
    func primaryAction(for result: SearchResult) -> (label: String, key: String) {
        if result.kind == .snippet, result.completion == nil, !snippetsPasteOnOpen {
            return ("Copy Snippet", "↩")
        }
        return result.primaryAction
    }

    /// Pasting a snippet closes the palette first, like `openInFrontApp`; copying one keeps
    /// it open to confirm.
    func openSnippet(_ result: SearchResult, paste: Bool) {
        confirming = nil
        error = nil
        guard paste else {
            do {
                if case .copied(let text) = try core.openSnippet(id: result.id, query: query, paste: false) {
                    showCopied(text)
                }
            } catch {
                Logger.palette.error("couldn't copy a snippet: \(error, privacy: .public)")
                self.error = error.userMessage
            }
            return
        }
        onClose?()
        do {
            _ = try core.openSnippet(id: result.id, query: query, paste: true)
        } catch {
            Logger.palette.error("couldn't paste a snippet: \(error, privacy: .public)")
            onReopen?()
            self.error = error.userMessage
        }
    }

    /// Window actions act on the app that was in front, so the palette closes first; a
    /// failure reopens it to show why.
    private func openInFrontApp(_ result: SearchResult) {
        confirming = nil
        error = nil
        onClose?()
        do {
            _ = try core.openResult(id: result.id, query: query)
        } catch {
            Logger.palette.error("couldn't run a \(String(describing: result.kind), privacy: .public) action on the front app: \(error, privacy: .public)")
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
            Logger.palette.error("couldn't open \(file.path, privacy: .private): \(error, privacy: .public)")
            self.error = error.userMessage
        }
    }

    func revealSelectedFile() {
        guard let file = files.selected else { return }
        do {
            try core.revealFile(path: file.path)
            onClose?()
        } catch {
            Logger.palette.error("couldn't reveal \(file.path, privacy: .private): \(error, privacy: .public)")
            self.error = error.userMessage
        }
    }

    func copySelectedPath() {
        guard let file = files.selected else { return }
        copy(file.path)
    }

    /// Copying a path or name is a UI convenience, so it stays in the shell.
    func copy(_ text: String) {
        NSPasteboard.general.clearContents()
        NSPasteboard.general.setString(text, forType: .string)
        showCopied(text)
    }

    // MARK: Colour actions

    func copySelectedColor() {
        copyColors(as: colorFormat)
    }

    /// Copies the selected colour as `format`, or the marked ones, one per line.
    func copyColors(as format: ColorFormat) {
        let targets = colors.targets
        guard let first = targets.first else { return }
        guard targets.count > 1 else {
            do {
                try core.copyColor(color: first, format: format)
                showCopied(formatColor(color: first, format: format))
            } catch {
                Logger.palette.error("couldn't copy a colour: \(error, privacy: .public)")
                self.error = error.userMessage
            }
            return
        }
        // Joining lines for the pasteboard is a UI convenience, like `copy`.
        let lines = targets.map { formatColor(color: $0, format: format) }
        NSPasteboard.general.clearContents()
        NSPasteboard.general.setString(lines.joined(separator: "\n"), forType: .string)
        showCopied("\(targets.count) colours")
    }

    /// The magnifier covers the screen, so the palette gets out of its way first.
    func pickColor() {
        onClose?()
        onPickColor?()
    }

    // MARK: Emoji actions

    /// Pasting closes first, so the emoji lands in the app that was in front; copying shows
    /// it was copied, then closes.
    func insertSelectedEmoji(paste: Bool) {
        guard let selected = emoji.selected, copied == nil else { return }
        if paste { onClose?() }
        do {
            try core.insertEmoji(emoji: selected.emoji, paste: paste)
            onEmojiUsed?()
            if !paste { showCopied(selected.emoji) }
        } catch {
            Logger.palette.error("couldn't \(paste ? "paste" : "copy") an emoji: \(error, privacy: .public)")
            if paste { onReopen?() }
            self.error = error.userMessage
        }
    }

    // MARK: Camera actions

    func openCameraPrivacySettings() {
        do {
            _ = try core.openResult(id: "settings:privacy-camera", query: "")
            onClose?()
        } catch {
            report(error)
        }
    }

    func selectCamera(_ id: String) {
        do { try camera.select(id) } catch { report(error) }
    }

    func toggleCameraMirror() {
        settings.config.camera.mirror.toggle()
    }

    private func complete(with completion: String) {
        query = completion
        cursorToEndRequest += 1
    }

    /// Shows an error from an action under the search field.
    func report(_ error: Error) {
        Logger.palette.error("palette action failed: \(error, privacy: .public)")
        self.error = error.userMessage
    }

    // MARK: Actions menu

    /// ⌘K: lists what can be done with the selection; nothing opens without a selection.
    func toggleActions() {
        if actionsMenu != nil {
            closeActions()
            return
        }
        let actions = actions()
        guard !actions.isEmpty else { return }
        confirming = nil
        actionsMenu = ActionsMenu(actions: actions)
    }

    func closeActions() {
        guard actionsMenu != nil else { return }
        actionsMenu = nil
        refocusRequest += 1
    }

    /// Runs an action from the menu; ones that need confirming ask on the first ↩.
    func run(_ action: PaletteAction) {
        guard let menu = actionsMenu else { return }
        if action.needsConfirmation, menu.confirming != action.id {
            menu.confirming = action.id
            return
        }
        closeActions()
        action.perform()
    }

    func showCopied(_ text: String) {
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
                Group {
                    if model.mode == .search {
                        // The launcher itself wears Turbo's own mark, the menu bar's glyph, sized to
                        // sit where the other modes' symbols do.
                        Image(nsImage: MenuBarIcon.turbo)
                            .renderingMode(.template)
                            .resizable()
                            .frame(width: 22, height: 22)
                    } else {
                        Image(systemName: model.mode.symbol)
                            .font(.system(size: 19, weight: .regular))
                    }
                }
                .foregroundStyle(.secondary)
                SearchField(
                    text: $model.query,
                    placeholder: model.placeholder,
                    focusRequest: model.focusRequest,
                    cursorToEndRequest: model.cursorToEndRequest,
                    refocusRequest: model.refocusRequest,
                    takesSidewaysArrows: model.mode.isGrid,
                    extendsSelection: model.mode.hasMarkableRows,
                    // Nothing to search while the camera shows.
                    isEditable: model.mode != .camera
                ) { command in
                    switch command {
                    case .up: model.move(by: -1)
                    case .down: model.move(by: 1)
                    case .top: model.jump(toEnd: false)
                    case .bottom: model.jump(toEnd: true)
                    case .extendTop: model.extendMarks(toEnd: false)
                    case .extendBottom: model.extendMarks(toEnd: true)
                    case .left: model.moveSideways(by: -1)
                    case .right: model.moveSideways(by: 1)
                    case .submit: model.openSelected()
                    case .alternateSubmit: _ = model.handle(.alternateOpen)
                    case .complete: model.tab(by: 1)
                    case .backtab: model.tab(by: -1)
                    case .extendUp: model.extendMarks(.up)
                    case .extendDown: model.extendMarks(.down)
                    case .extendLeft: model.extendMarks(.left)
                    case .extendRight: model.extendMarks(.right)
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
            case .colors:
                if !model.colors.colors.isEmpty {
                    Divider().padding(.horizontal, 14)
                    ColorsModeView(model: model)
                }
            case .emoji:
                if !model.emoji.isEmpty {
                    EmojiModeView(model: model)
                }
            case .camera:
                CameraModeView(model: model)
            }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .top)
        // Dark glass in dark mode; a frosted white in light mode, so text stays legible over a
        // busy desktop.
        .glassEffect(
            .regular.tint(Color.appearance(light: .white.opacity(0.6), dark: .black.opacity(0.35))),
            in: .rect(cornerRadius: PaletteLayout.cornerRadius)
        )
        .overlay {
            RoundedRectangle(cornerRadius: PaletteLayout.cornerRadius)
                .strokeBorder(Color.appearance(light: .white.opacity(0.7), dark: .white.opacity(0.12)), lineWidth: 1)
        }
        .overlay(alignment: .bottomTrailing) {
            if let menu = model.actionsMenu {
                // The click catcher fills the palette, so the menu needs its own alignment.
                ZStack(alignment: .bottomTrailing) {
                    // A click anywhere else closes the menu without acting on what's under it.
                    Color.black.opacity(0.001)
                        .onTapGesture { model.closeActions() }
                    ActionsMenuView(model: model, menu: menu)
                        .padding(.trailing, 10)
                        .padding(.bottom, ActionsMenuLayout.bottomInset(for: model.mode))
                        .transition(.opacity.combined(with: .scale(scale: 0.97, anchor: .bottomTrailing)))
                }
            }
        }
        .animation(.snappy(duration: 0.15), value: model.actionsMenu != nil)
    }
}

extension Color {
    /// A colour that follows the light or dark appearance it's drawn in.
    static func appearance(light: Color, dark: Color) -> Color {
        Color(nsColor: NSColor(name: nil) { appearance in
            appearance.bestMatch(from: [.aqua, .darkAqua]) == .darkAqua ? NSColor(dark) : NSColor(light)
        })
    }
}

extension PaletteMode {
    var symbol: String {
        switch self {
        case .search: "magnifyingglass"
        case .clipboard: "doc.on.clipboard"
        case .files: "doc.text.magnifyingglass"
        case .colors: "swatchpalette"
        case .emoji: "face.smiling"
        case .camera: "web.camera"
        }
    }

    /// Colours and emoji are grids, where ← and → move the selection.
    var isGrid: Bool {
        self == .colors || self == .emoji
    }

    /// Clipboard items and colours can be marked, to paste, pin, copy or delete several at once.
    var hasMarkableRows: Bool {
        self == .clipboard || self == .colors
    }

    var placeholder: String {
        switch self {
        case .search: "Search for apps and commands…"
        case .clipboard: "Search clipboard history…"
        case .files: "Search files…"
        case .colors: "Search picked colours…"
        case .emoji: "Search emoji…"
        case .camera: "Camera Preview"
        }
    }
}

extension PaletteModel {
    /// The mode's placeholder; Camera Preview names the camera it's showing.
    var placeholder: String {
        if mode == .camera, let name = camera.camera?.name { return name }
        return mode.placeholder
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
    @State private var hasMoreBelow = false

    var body: some View {
        ScrollViewReader { proxy in
            ScrollView {
                LazyVStack(alignment: .leading, spacing: 0) {
                    Text(model.query.trimmingCharacters(in: .whitespaces).isEmpty ? "Suggestions" : "Results")
                        .font(.system(size: 12, weight: .semibold))
                        .foregroundStyle(.secondary)
                        .padding(.horizontal, 10)
                        .frame(height: PaletteLayout.sectionHeaderHeight, alignment: .center)
                    ForEach(Array(model.results.enumerated()), id: \.element.id) { index, result in
                        ResultRow(result: result, hotkey: result.hotkey, isSelected: index == model.selection)
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
                .padding(.horizontal, 8)
                .padding(.bottom, PaletteLayout.listPadding)
            }
            .scrollIndicators(.never)
            .onScrollGeometryChange(for: Bool.self) { geometry in
                // The container stops at the action bar. The panel is sized to the rows, so the bottom padding alone always overflows.
                let hidden = geometry.contentSize.height - geometry.contentOffset.y - geometry.containerSize.height
                return hidden > PaletteLayout.listPadding + 1
            } action: { _, moreBelow in
                hasMoreBelow = moreBelow
            }
            // Rows scrolling under the action bar fade out, but only while there's more to scroll to.
            .mask {
                VStack(spacing: 0) {
                    Rectangle()
                    LinearGradient(colors: [.black, hasMoreBelow ? .clear : .black], startPoint: .top, endPoint: .bottom)
                        .frame(height: PaletteLayout.actionBarHeight)
                }
            }
            .safeAreaInset(edge: .bottom, spacing: 0) {
                ActionBar(model: model)
            }
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
    let hotkey: Hotkey?
    let isSelected: Bool

    var body: some View {
        HStack(spacing: 12) {
            if result.kind == .calculator {
                calculation
            } else {
                icon
                    .frame(width: 24, height: 24)
                Text(result.title)
                    .font(.system(size: 14))
                    .lineLimit(1)
                    .layoutPriority(1)
                if let hotkey {
                    Keycaps(keys: hotkey.keycaps)
                }
            }
            Spacer(minLength: 16)
            if result.kind != .calculator, !result.subtitle.isEmpty {
                Text(result.kind == .snippet ? result.subtitle.oneLine : result.subtitle)
                    .font(.system(size: 13))
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
                    .truncationMode(.middle)
            }
        }
        .padding(.horizontal, 10)
        .frame(height: PaletteLayout.rowHeight(for: result))
        .background {
            if isSelected {
                RoundedRectangle(cornerRadius: 10).fill(PaletteLayout.selectionFill)
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

    @ViewBuilder
    private var icon: some View {
        if let path = result.path {
            Image(nsImage: AppIcons.icon(forPath: path))
                .resizable()
                .aspectRatio(contentMode: .fit)
        } else if result.kind == .windowAction,
                  let action = windowActionForResult(resultId: result.id),
                  action.region != nil {
            WindowActionIcon(action: action, width: 17)
                .frame(width: 24, height: 24)
                .background(.primary.opacity(0.08), in: .rect(cornerRadius: 6))
        } else {
            Image(systemName: result.symbol ?? "gearshape")
                .font(.system(size: 13, weight: .medium))
                .frame(width: 24, height: 24)
                .background(.primary.opacity(0.08), in: .rect(cornerRadius: 6))
        }
    }
}

/// The settings button and the selection's primary action, floating over the bottom of the
/// results.
private struct ActionBar: View {
    let model: PaletteModel

    var body: some View {
        FloatingActionBar(onShowSettings: model.onShowSettings, actions: actions)
    }

    private var actions: [FloatingAction] {
        guard let result = model.selectedResult else { return [] }
        let action = model.primaryAction(for: result)
        return [
            FloatingAction(label: action.label, keys: [action.key]) { model.openSelected() },
            .showActions(model),
        ]
    }
}

struct FloatingAction {
    let label: String
    let keys: [String]
    let perform: () -> Void

    @MainActor
    static func showActions(_ model: PaletteModel) -> FloatingAction {
        FloatingAction(label: "Actions", keys: ["⌘", "K"]) { model.toggleActions() }
    }
}

/// A settings button on the left and the selection's actions in a capsule on the right,
/// floating over the bottom of a scrolling list or grid.
struct FloatingActionBar: View {
    let onShowSettings: (() -> Void)?
    let actions: [FloatingAction]
    var showsSettings = true

    var body: some View {
        HStack {
            if showsSettings { settingsButton }
            Spacer()
            if !actions.isEmpty {
                HStack(spacing: 4) {
                    ForEach(Array(actions.enumerated()), id: \.offset) { index, action in
                        Button(action: action.perform) {
                            HStack(spacing: 8) {
                                Text(action.label)
                                    .font(.system(size: 13, weight: .regular))
                                    .foregroundStyle(index == 0 ? .primary : .secondary)
                                Keycaps(keys: action.keys)
                            }
                            .padding(.leading, index == 0 ? 14 : 8)
                            .padding(.trailing, 8)
                            .frame(height: 36)
                            .contentShape(.rect)
                        }
                        .buttonStyle(.plain)
                    }
                }
                .glassEffect(.regular.interactive(), in: .capsule)
            }
        }
        .padding(.horizontal, 10)
        .frame(height: PaletteLayout.actionBarHeight)
    }

    private var settingsButton: some View {
        Button {
            onShowSettings?()
        } label: {
            Image(systemName: "gearshape")
                .font(.system(size: 14, weight: .medium))
                .foregroundStyle(.secondary)
                .frame(width: 36, height: 36)
                .contentShape(.circle)
        }
        .buttonStyle(.plain)
        .glassEffect(.regular.interactive(), in: .circle)
        .help("Settings (⌘,)")
    }
}

/// Keys drawn as small outlined caps: ⌥ T, ⌘ K.
struct Keycaps: View {
    let keys: [String]

    var body: some View {
        HStack(spacing: 3) {
            ForEach(Array(keys.enumerated()), id: \.offset) { _, key in
                Text(key)
                    .font(.system(size: 11, weight: .medium))
                    .foregroundStyle(.secondary)
                    .padding(.horizontal, key.count > 1 ? 5 : 0)
                    .frame(minWidth: 20, minHeight: 20)
                    .background(.primary.opacity(0.06), in: .rect(cornerRadius: 5))
                    .overlay {
                        RoundedRectangle(cornerRadius: 5).strokeBorder(.primary.opacity(0.18))
                    }
            }
        }
    }
}

extension Hotkey {
    /// One cap per modifier, in menu order, then the key.
    @MainActor
    var keycaps: [String] {
        Modifier.symbols(for: modifiers) + [KeyNames.name(for: keyCode)]
    }
}

extension SearchResult {
    /// Window actions act on the frontmost app, so the palette closes first. Snippets do too
    /// when pasted (`PaletteModel.openSnippet`).
    var actsOnFrontApp: Bool {
        kind == .windowAction
    }

    /// What ⏎ will do with this result, for the action bar.
    var primaryAction: (label: String, key: String) {
        if completion != nil {
            return ("Complete", "TAB")
        }
        if id.hasPrefix("mode:") {
            return ("Show", "↩")
        }
        switch kind {
        case .application: return ("Open Application", "↩")
        case .settingsPane: return ("Open Settings", "↩")
        case .quicklink: return ("Open Link", "↩")
        case .calculator: return ("Copy Answer", "↩")
        case .systemAction, .command: return ("Run Command", "↩")
        case .windowAction: return ("Apply", "↩")
        case .snippet: return ("Paste Snippet", "↩")
        }
    }
}

extension String {
    /// Line breaks and runs of whitespace collapsed to single spaces, for one-line previews.
    var oneLine: String {
        split(whereSeparator: \.isWhitespace).joined(separator: " ")
    }
}

/// An unbezeled AppKit text field: SwiftUI's can't select all on demand or hand over the
/// arrow, return and escape keys before the field acts on them.
struct SearchField: NSViewRepresentable {
    @Binding var text: String
    let placeholder: String
    var fontSize: CGFloat = 22
    let focusRequest: Int
    var cursorToEndRequest = 0
    /// Takes focus back without selecting the text, leaving the cursor at the end.
    var refocusRequest = 0
    /// ← and → move through a grid instead of the cursor.
    var takesSidewaysArrows = false
    /// ⇧ with an arrow marks rows instead of selecting text (sideways only in a grid), and
    /// ⇧Tab is handed over too.
    var extendsSelection = false
    /// Off, the field only shows its placeholder; Esc and the ⌘ shortcuts still reach the
    /// panel.
    var isEditable = true
    let onCommand: (Command) -> Void

    enum Command {
        case up, down, left, right, submit, alternateSubmit, complete, cancel
        case backtab, extendUp, extendDown, extendLeft, extendRight
        /// ⌘↑/⌘↓, and with ⇧ where rows can be marked.
        case top, bottom, extendTop, extendBottom
    }

    func makeNSView(context: Context) -> NSTextField {
        let field = NSTextField()
        field.isBordered = false
        field.drawsBackground = false
        field.focusRingType = .none
        field.font = .systemFont(ofSize: fontSize)
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
        if field.isEditable != isEditable {
            field.isEditable = isEditable
            field.isSelectable = isEditable
            if !isEditable, field.currentEditor() != nil {
                field.window?.makeFirstResponder(nil)
            }
        }
        if context.coordinator.focusRequest != focusRequest {
            context.coordinator.focusRequest = focusRequest
            // The window may not be key yet on the same pass as showing it.
            DispatchQueue.main.async {
                field.window?.makeFirstResponder(field)
                field.selectText(nil)
            }
        }
        if context.coordinator.refocusRequest != refocusRequest {
            context.coordinator.refocusRequest = refocusRequest
            DispatchQueue.main.async {
                guard field.window?.makeFirstResponder(field) == true else { return }
                let end = (field.stringValue as NSString).length
                field.currentEditor()?.selectedRange = NSRange(location: end, length: 0)
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
        var refocusRequest = 0

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
            case #selector(NSResponder.moveToBeginningOfDocument(_:)): parent.onCommand(.top)
            case #selector(NSResponder.moveToEndOfDocument(_:)): parent.onCommand(.bottom)
            case #selector(NSResponder.moveToBeginningOfDocumentAndModifySelection(_:))
                where parent.extendsSelection:
                parent.onCommand(.extendTop)
            case #selector(NSResponder.moveToEndOfDocumentAndModifySelection(_:)) where parent.extendsSelection:
                parent.onCommand(.extendBottom)
            case #selector(NSResponder.moveLeft(_:)) where parent.takesSidewaysArrows: parent.onCommand(.left)
            case #selector(NSResponder.moveRight(_:)) where parent.takesSidewaysArrows: parent.onCommand(.right)
            case #selector(NSResponder.insertNewline(_:)):
                let command = NSApp.currentEvent?.modifierFlags.contains(.command) == true
                parent.onCommand(command ? .alternateSubmit : .submit)
            case #selector(NSResponder.insertTab(_:)): parent.onCommand(.complete)
            case #selector(NSResponder.insertBacktab(_:)) where parent.extendsSelection:
                parent.onCommand(.backtab)
            case #selector(NSResponder.moveUpAndModifySelection(_:)) where parent.extendsSelection:
                parent.onCommand(.extendUp)
            case #selector(NSResponder.moveDownAndModifySelection(_:)) where parent.extendsSelection:
                parent.onCommand(.extendDown)
            case #selector(NSResponder.moveLeftAndModifySelection(_:))
                where parent.extendsSelection && parent.takesSidewaysArrows:
                parent.onCommand(.extendLeft)
            case #selector(NSResponder.moveRightAndModifySelection(_:))
                where parent.extendsSelection && parent.takesSidewaysArrows:
                parent.onCommand(.extendRight)
            case #selector(NSResponder.cancelOperation(_:)): parent.onCommand(.cancel)
            default: return false
            }
            return true
        }
    }
}
