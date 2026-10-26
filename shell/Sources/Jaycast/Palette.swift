import AppKit
import Observation
import SwiftUI

/// The launcher palette: a floating Liquid Glass panel with a search field and results.
///
/// The panel is non-activating, so the frontmost app keeps looking active, but it still
/// becomes key so typing goes to the search field. The controller solely owns the frame:
/// it pins the top edge where the palette opened and grows or shrinks the height as
/// results change, so the search field never moves.
@MainActor
final class PaletteWindowController: NSObject, NSWindowDelegate {
    let model: PaletteModel
    private lazy var panel = makePanel()
    /// The top edge chosen when the palette was shown; resizes keep it fixed.
    private var top: CGFloat = 0
    private var resizeTask: Task<Void, Never>?

    init(core: Core) {
        model = PaletteModel(core: core)
        super.init()
        model.onClose = { [weak self] in self?.hide() }
    }

    var isVisible: Bool { panel.isVisible }

    func toggle() {
        if panel.isVisible, panel.isKeyWindow {
            hide()
        } else {
            show()
        }
    }

    /// Reopening keeps the last query, selected, like Spotlight: typing replaces it, and ⏎
    /// repeats the last search.
    func show() {
        model.prepareForShow()
        let screen = NSScreen.screens.first { NSMouseInRect(NSEvent.mouseLocation, $0.frame, false) } ?? NSScreen.main
        if let visible = screen?.visibleFrame {
            top = visible.maxY - (visible.height * 0.22).rounded()
            let x = (visible.midX - PaletteLayout.width / 2).rounded()
            panel.setFrame(frame(x: x), display: false)
        }
        panel.makeKeyAndOrderFront(nil)
        observeHeight()
    }

    func hide() {
        resizeTask?.cancel()
        resizeTask = nil
        panel.orderOut(nil)
        model.didHide()
    }

    func windowDidResignKey(_ notification: Notification) {
        hide()
    }

    // MARK: Private

    private func frame(x: CGFloat) -> NSRect {
        let height = PaletteLayout.height(for: model)
        return NSRect(x: x, y: top - height, width: PaletteLayout.width, height: height)
    }

    private func observeHeight() {
        guard resizeTask == nil else { return }
        resizeTask = Task { [weak self, model] in
            for await _ in Observations({ PaletteLayout.height(for: model) }) {
                guard let self, self.panel.isVisible else { continue }
                self.panel.setFrame(self.frame(x: self.panel.frame.minX), display: true)
            }
        }
    }

    private func makePanel() -> PalettePanel {
        let panel = PalettePanel(
            contentRect: NSRect(x: 0, y: 0, width: PaletteLayout.width, height: PaletteLayout.searchHeight),
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
        panel.onCancel = { [weak self] in self?.model.cancel() }
        panel.onClose = { [weak self] in self?.hide() }

        let hosting = NSHostingView(rootView: PaletteView(model: model))
        // The controller sizes the window; SwiftUI must never resize it to fit its content.
        hosting.sizingOptions = []
        panel.contentView = hosting
        return panel
    }
}

/// Borderless panels can't become key by default; this one must, to take typing.
private final class PalettePanel: NSPanel {
    var onCancel: (() -> Void)?
    var onClose: (() -> Void)?

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
        return super.performKeyEquivalent(with: event)
    }
}

/// Sizes shared by the view and the controller's height calculation.
enum PaletteLayout {
    static let width: CGFloat = 680
    static let searchHeight: CGFloat = 60
    static let rowHeight: CGFloat = 44
    static let messageHeight: CGFloat = 36
    static let listPadding: CGFloat = 8
    static let maxVisibleRows = 8
    static let cornerRadius: CGFloat = 26

    @MainActor
    static func height(for model: PaletteModel) -> CGFloat {
        var height = searchHeight
        if model.message != nil {
            height += messageHeight
        }
        if !model.results.isEmpty {
            let rows = min(model.results.count, maxVisibleRows)
            height += 1 + listPadding * 2 + CGFloat(rows) * rowHeight
        }
        return height
    }
}

// MARK: - Model

@MainActor
@Observable
final class PaletteModel {
    var query = "" {
        didSet {
            if query != oldValue { search() }
        }
    }
    private(set) var results: [SearchResult] = []
    var selection = 0
    /// A result that needs confirming; ⏎ again runs it, Esc backs out.
    private(set) var confirming: SearchResult?
    private(set) var error: String?
    /// Bumped on every show, so the search field takes focus and selects its text.
    private(set) var focusRequest = 0

    @ObservationIgnored var onClose: (() -> Void)?
    @ObservationIgnored private let core: Core
    /// Where the mouse was when it last picked a row: rows scrolling under a still pointer
    /// shouldn't steal the keyboard selection.
    @ObservationIgnored private var lastHoverLocation: NSPoint?

    private static let limit: UInt32 = 20

    init(core: Core) {
        self.core = core
    }

    /// The line between the search field and the results, if any.
    var message: PaletteMessage? {
        if let confirming {
            .confirm(confirming.title)
        } else if let error {
            .error(error)
        } else if results.isEmpty, !query.trimmingCharacters(in: .whitespaces).isEmpty {
            .noResults(query)
        } else {
            nil
        }
    }

    var selectedResult: SearchResult? {
        results.indices.contains(selection) ? results[selection] : nil
    }

    func search() {
        results = core.search(query: query, limit: Self.limit)
        selection = 0
        confirming = nil
        error = nil
    }

    func prepareForShow() {
        search()
        focusRequest += 1
        lastHoverLocation = NSEvent.mouseLocation
    }

    func didHide() {
        confirming = nil
        error = nil
    }

    func move(by offset: Int) {
        guard !results.isEmpty else { return }
        confirming = nil
        selection = (selection + offset + results.count) % results.count
    }

    func hover(_ index: Int) {
        let location = NSEvent.mouseLocation
        guard location != lastHoverLocation else { return }
        lastHoverLocation = location
        if selection != index {
            selection = index
            confirming = nil
        }
    }

    func openSelected() {
        guard let result = selectedResult else { return }
        open(result)
    }

    func open(_ result: SearchResult) {
        if result.needsConfirmation, confirming?.id != result.id {
            confirming = result
            return
        }
        do {
            try core.openResult(id: result.id, query: query)
            confirming = nil
            error = nil
            onClose?()
        } catch {
            confirming = nil
            self.error = error.userMessage
        }
    }

    /// Esc: back out of a confirmation, then clear the query, then close.
    func cancel() {
        if confirming != nil {
            confirming = nil
        } else if error != nil {
            error = nil
        } else if !query.isEmpty {
            query = ""
        } else {
            onClose?()
        }
    }
}

enum PaletteMessage: Equatable {
    case confirm(String)
    case error(String)
    case noResults(String)
}

// MARK: - Views

private struct PaletteView: View {
    let model: PaletteModel

    var body: some View {
        @Bindable var model = model

        VStack(spacing: 0) {
            HStack(spacing: 12) {
                Image(systemName: "magnifyingglass")
                    .font(.system(size: 20, weight: .medium))
                    .foregroundStyle(.secondary)
                SearchField(text: $model.query, focusRequest: model.focusRequest) { command in
                    switch command {
                    case .up: model.move(by: -1)
                    case .down: model.move(by: 1)
                    case .submit: model.openSelected()
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

            if !model.results.isEmpty {
                Divider().padding(.horizontal, 14)
                ResultsList(model: model)
            }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .top)
        .glassEffect(.regular, in: .rect(cornerRadius: PaletteLayout.cornerRadius))
    }
}

private struct MessageRow: View {
    let message: PaletteMessage

    var body: some View {
        HStack(spacing: 8) {
            switch message {
            case .confirm(let title):
                Image(systemName: "exclamationmark.triangle.fill").foregroundStyle(.orange)
                Text("\(title)?").fontWeight(.semibold)
                Text("Press ↩ to confirm, ⎋ to cancel").foregroundStyle(.secondary)
            case .error(let text):
                Image(systemName: "xmark.octagon.fill").foregroundStyle(.red)
                Text(text).lineLimit(1)
            case .noResults(let query):
                Text("No results for “\(query)”").foregroundStyle(.secondary)
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
            Spacer(minLength: 0)
            if isSelected {
                Text(result.kind == .application ? "Open" : "Run")
                    .font(.caption)
                    .foregroundStyle(.secondary)
                Text("↩")
                    .font(.caption.weight(.semibold))
                    .foregroundStyle(.secondary)
            }
        }
        .padding(.horizontal, 10)
        .frame(height: PaletteLayout.rowHeight)
        .background {
            if isSelected {
                RoundedRectangle(cornerRadius: 10).fill(.primary.opacity(0.1))
            }
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
    let focusRequest: Int
    let onCommand: (Command) -> Void

    enum Command { case up, down, submit, cancel }

    func makeNSView(context: Context) -> NSTextField {
        let field = NSTextField()
        field.isBordered = false
        field.drawsBackground = false
        field.focusRingType = .none
        field.font = .systemFont(ofSize: 22)
        field.placeholderString = "Search apps and actions"
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
        if context.coordinator.focusRequest != focusRequest {
            context.coordinator.focusRequest = focusRequest
            // The window may not be key yet on the same pass as showing it.
            DispatchQueue.main.async {
                field.window?.makeFirstResponder(field)
                field.selectText(nil)
            }
        }
    }

    func makeCoordinator() -> Coordinator {
        Coordinator(parent: self)
    }

    @MainActor
    final class Coordinator: NSObject, NSTextFieldDelegate {
        var parent: SearchField
        var focusRequest = 0

        init(parent: SearchField) {
            self.parent = parent
        }

        func controlTextDidChange(_ notification: Notification) {
            guard let field = notification.object as? NSTextField else { return }
            parent.text = field.stringValue
        }

        /// ⌃N/⌃P arrive as moveDown:/moveUp: through the standard key bindings.
        func control(_ control: NSControl, textView: NSTextView, doCommandBy selector: Selector) -> Bool {
            switch selector {
            case #selector(NSResponder.moveUp(_:)): parent.onCommand(.up)
            case #selector(NSResponder.moveDown(_:)): parent.onCommand(.down)
            case #selector(NSResponder.insertNewline(_:)): parent.onCommand(.submit)
            case #selector(NSResponder.cancelOperation(_:)): parent.onCommand(.cancel)
            default: return false
            }
            return true
        }
    }
}
