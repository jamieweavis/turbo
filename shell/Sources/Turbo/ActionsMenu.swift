import AppKit
import Observation
import SwiftUI

/// The ⌘K actions menu: everything that can be done with the palette's selection, in a
/// small searchable list floating above the action bar or footer. ↩ runs the highlighted
/// action, Esc or ⌘K closes it. The first action is what ↩ does in the palette, the second
/// (marked ⌘↩) what ⌘↩ does, so the menu also documents the shortcuts.

struct PaletteAction: Identifiable {
    let title: String
    let symbol: String
    /// Shown as keycaps; only describes a shortcut the palette already handles.
    var keys: [String] = []
    var isDestructive = false
    /// Asks on the first ↩ in the menu, for actions that can't be undone.
    var needsConfirmation = false
    let perform: () -> Void

    var id: String { title }
}

enum ActionKeys {
    static let primary = ["↩"]
    static let alternate = ["⌘", "↩"]
    static let pin = ["⌘", "P"]
    static let delete = ["⌘", "⌫"]
    static let quickLook = ["⌘", "Y"]
    static let copy = ["⌘", "C"]
    static let select = ["TAB"]
    static let deselect = ["⎋"]
}

/// The open menu: the actions it was opened with, filtered by its own query.
@MainActor
@Observable
final class ActionsMenu {
    let actions: [PaletteAction]
    var query = "" {
        didSet {
            selection = 0
            confirming = nil
        }
    }
    var selection = 0 {
        didSet {
            if selection != oldValue { confirming = nil }
        }
    }
    /// The action waiting for a second ↩.
    var confirming: String?

    init(actions: [PaletteAction]) {
        self.actions = actions
    }

    /// Actions whose title contains every word typed, in their original order.
    var filtered: [PaletteAction] {
        let words = query.split(whereSeparator: \.isWhitespace)
        guard !words.isEmpty else { return actions }
        return actions.filter { action in words.allSatisfy { action.title.localizedStandardContains($0) } }
    }

    var selected: PaletteAction? {
        let filtered = filtered
        return filtered.indices.contains(selection) ? filtered[selection] : nil
    }

    func move(by offset: Int) {
        let count = filtered.count
        guard count > 0 else { return }
        selection = (selection + offset + count) % count
    }
}

enum ActionsMenuLayout {
    static let width: CGFloat = 340
    static let fieldHeight: CGFloat = 44
    static let rowHeight: CGFloat = 34
    static let padding: CGFloat = 6
    static let maxVisibleRows = 8
    static let cornerRadius: CGFloat = 16

    /// An empty filter still shows its one "No matching actions" row.
    static func listHeight(rows: Int) -> CGFloat {
        CGFloat(max(1, min(rows, maxVisibleRows))) * rowHeight + padding * 2
    }

    /// The menu unfiltered: the palette makes room for this much, so it doesn't resize
    /// while the menu shrinks to fit what's typed.
    @MainActor
    static func fullHeight(for menu: ActionsMenu) -> CGFloat {
        listHeight(rows: menu.actions.count) + 1 + fieldHeight
    }

    /// The menu sits just above the floating action bar, or above the footer hints.
    static func bottomInset(for mode: PaletteMode) -> CGFloat {
        switch mode {
        case .search, .emoji: PaletteLayout.actionBarHeight
        case .clipboard, .files, .colors: PaletteLayout.footerHeight + 8
        case .camera: PaletteLayout.actionBarHeight + PaletteLayout.cameraInset
        }
    }
}

// MARK: - Actions per mode

extension PaletteModel {
    /// What the menu lists for the current selection; empty when nothing is selected.
    func actions() -> [PaletteAction] {
        switch mode {
        case .search: selectedResult.map(searchActions) ?? []
        case .clipboard: clipboardActions()
        case .files: fileActions()
        case .colors: colorActions()
        case .emoji: emojiActions()
        case .camera: cameraActions()
        }
    }

    /// "Paste to Safari": the panel doesn't activate, so the frontmost app is the one a
    /// paste lands in.
    static var pasteTitle: String {
        let target = NSWorkspace.shared.frontmostApplication
            .flatMap { $0 == .current ? nil : $0.localizedName }
        return target.map { "Paste to \($0)" } ?? "Paste"
    }

    private func searchActions(_ result: SearchResult) -> [PaletteAction] {
        let primary = primaryAction(for: result)
        let primarySymbol = result.kind == .snippet && !snippetsPasteOnOpen ? "doc.on.doc" : result.primarySymbol
        var actions = [
            PaletteAction(title: primary.label, symbol: primarySymbol, keys: [primary.key]) {
                self.open(result)
            },
        ]
        if result.kind == .snippet {
            let pastes = snippetsPasteOnOpen
            actions.append(PaletteAction(title: pastes ? "Copy to Clipboard" : Self.pasteTitle,
                                         symbol: pastes ? "doc.on.doc" : "text.insert",
                                         keys: ActionKeys.alternate) {
                self.openSnippet(result, paste: !pastes)
            })
        }
        if result.kind == .application, let path = result.path {
            let bundleId = String(result.id.dropFirst("app:".count))
            actions += [
                PaletteAction(title: "Show in Finder", symbol: "folder", keys: ActionKeys.alternate) {
                    self.attempt { try self.core.revealFile(path: path) }
                },
                PaletteAction(title: "Copy Path", symbol: "doc.on.doc") { self.copy(path) },
                PaletteAction(title: "Copy Bundle Identifier", symbol: "number") { self.copy(bundleId) },
            ]
            if core.isAppRunning(bundleId: bundleId) {
                actions += [
                    PaletteAction(title: "Hide Application", symbol: "eye.slash") {
                        self.attempt { try self.core.hideApp(bundleId: bundleId) }
                    },
                    PaletteAction(title: "Quit Application", symbol: "xmark.circle") {
                        self.attempt { try self.core.quitApp(bundleId: bundleId, force: false) }
                    },
                    PaletteAction(title: "Force Quit Application", symbol: "exclamationmark.octagon",
                                  isDestructive: true, needsConfirmation: true) {
                        self.attempt { try self.core.quitApp(bundleId: bundleId, force: true) }
                    },
                ]
            }
        }
        // Calculations aren't ranked, so there's nothing to reset.
        if result.kind != .calculator {
            actions.append(PaletteAction(title: "Reset Ranking", symbol: "arrow.counterclockwise") {
                do {
                    try self.core.resetResultRanking(id: result.id)
                    self.refresh()
                } catch {
                    self.report(error)
                }
            })
        }
        return actions
    }

    /// With rows marked, everything here acts on all of them, and says how many.
    private func clipboardActions() -> [PaletteAction] {
        let targets = clipboard.targets
        guard let item = targets.first else { return [] }
        let several = targets.count > 1
        let items = several ? " \(targets.count) Items" : ""
        let pastes = clipboardConfig.pasteOnOpen
        // Without marks ⌘↩ does the other of paste and copy; with them, ↩ is the only key.
        let otherKeys = clipboard.hasMarks ? [] : ActionKeys.alternate
        let paste = PaletteAction(title: several ? "Paste\(items)" : Self.pasteTitle, symbol: "doc.on.clipboard",
                                  keys: pastes ? ActionKeys.primary : otherKeys) {
            self.pasteSelected(paste: true)
        }
        let copy = PaletteAction(title: several ? "Copy\(items)" : "Copy to Clipboard", symbol: "doc.on.doc",
                                 keys: pastes ? otherKeys : ActionKeys.primary) {
            self.pasteSelected(paste: false)
        }
        var actions = pastes ? [paste, copy] : [copy, paste]
        if !several, item.kind == .files, let path = item.filePaths.first {
            actions.append(PaletteAction(title: "Show in Finder", symbol: "folder") {
                self.attempt { try self.core.revealFile(path: path) }
            })
        }
        let pinned = targets.allSatisfy(\.pinned)
        actions += [
            PaletteAction(title: (pinned ? "Unpin" : "Pin") + items, symbol: pinned ? "pin.slash" : "pin",
                          keys: ActionKeys.pin) {
                self.clipboard.togglePinTargets()
            },
            PaletteAction(title: "Delete" + items, symbol: "trash", keys: ActionKeys.delete, isDestructive: true,
                          needsConfirmation: several) {
                self.deleteTargets(confirmed: true)
            },
        ]
        actions += markActions(for: clipboard)
        actions.append(PaletteAction(title: "Clear Unpinned History", symbol: "trash.slash",
                                     isDestructive: true, needsConfirmation: true) {
            do {
                try self.core.clearClipboardHistory()
                self.refresh()
            } catch {
                self.report(error)
            }
        })
        return actions
    }

    /// Marking rows for bulk actions, so the menu teaches Tab and ⎋, and offers Select All.
    private func markActions(for browser: some MultiSelecting) -> [PaletteAction] {
        let marked = browser.isSelectionMarked
        var actions = [
            PaletteAction(title: marked ? "Deselect" : "Select", symbol: marked ? "circle" : "checkmark.circle",
                          keys: ActionKeys.select) {
                self.toggleMarkSelected()
            },
        ]
        if !browser.allMarked {
            actions.append(PaletteAction(title: "Select All", symbol: "checklist") {
                self.markAll()
            })
        }
        if browser.hasMarks {
            actions.append(PaletteAction(title: "Deselect All", symbol: "xmark.circle", keys: ActionKeys.deselect) {
                self.clearMarks()
            })
        }
        return actions
    }

    private func fileActions() -> [PaletteAction] {
        guard let file = files.selected else { return [] }
        return [
            PaletteAction(title: "Open", symbol: "arrow.up.forward.app", keys: ActionKeys.primary) {
                self.openSelectedFile()
            },
            PaletteAction(title: "Show in Finder", symbol: "folder", keys: ActionKeys.alternate) {
                self.revealSelectedFile()
            },
            PaletteAction(title: "Quick Look", symbol: "eye", keys: ActionKeys.quickLook) {
                self.onQuickLook?(URL(fileURLWithPath: file.path))
            },
            PaletteAction(title: "Copy Path", symbol: "doc.on.doc", keys: ActionKeys.copy) {
                self.copy(file.path)
            },
            PaletteAction(title: "Copy Name", symbol: "textformat") { self.copy(file.name) },
        ]
    }

    /// With swatches marked, copying and deleting act on all of them.
    private func colorActions() -> [PaletteAction] {
        let targets = colors.targets
        guard !targets.isEmpty else { return [] }
        let several = targets.count > 1
        let formats = [colorFormat] + ColorFormat.allCases.filter { $0 != colorFormat }
        var actions = formats.enumerated().map { index, format in
            PaletteAction(title: several ? "Copy \(targets.count) as \(format.title)" : "Copy \(format.title)",
                          symbol: "doc.on.doc", keys: index == 0 ? ActionKeys.primary : []) {
                self.copyColors(as: format)
            }
        }
        actions += [
            PaletteAction(title: "Pick Colour", symbol: "eyedropper", keys: ActionKeys.alternate) {
                self.pickColor()
            },
            PaletteAction(title: several ? "Delete \(targets.count) Colours" : "Delete", symbol: "trash",
                          keys: ActionKeys.delete, isDestructive: true, needsConfirmation: several) {
                self.deleteTargets(confirmed: true)
            },
        ]
        actions += markActions(for: colors)
        actions.append(PaletteAction(title: "Clear Colour History", symbol: "trash.slash",
                                     isDestructive: true, needsConfirmation: true) {
            do {
                try self.core.clearColorHistory()
                self.refresh()
                self.onColorsChanged?()
            } catch {
                self.report(error)
            }
        })
        return actions
    }

    private func emojiActions() -> [PaletteAction] {
        guard let selected = emoji.selected else { return [] }
        let pastes = emojiPasteOnOpen
        let paste = PaletteAction(title: Self.pasteTitle, symbol: "doc.on.clipboard",
                                  keys: pastes ? ActionKeys.primary : ActionKeys.alternate) {
            self.insertSelectedEmoji(paste: true)
        }
        let copy = PaletteAction(title: "Copy", symbol: "doc.on.doc",
                                 keys: pastes ? ActionKeys.alternate : ActionKeys.primary) {
            self.insertSelectedEmoji(paste: false)
        }
        var actions = pastes ? [paste, copy] : [copy, paste]
        actions.append(PaletteAction(title: "Copy Name", symbol: "textformat") { self.copy(selected.name) })
        if let shortcode = selected.shortcode {
            actions.append(PaletteAction(title: "Copy Shortcode", symbol: "chevron.left.forwardslash.chevron.right") {
                self.copy(":\(shortcode):")
            })
        }
        return actions
    }

    /// Mirroring, then every camera with the one showing ticked.
    private func cameraActions() -> [PaletteAction] {
        var actions: [PaletteAction] = []
        actions.append(PaletteAction(title: cameraMirrored ? "Turn Off Mirroring" : "Mirror Preview",
                                     symbol: "arrow.left.and.right") {
            self.toggleCameraMirror()
        })
        for device in camera.cameras {
            actions.append(PaletteAction(title: device.name, symbol: device.selected ? "checkmark" : device.kind.symbol) {
                self.selectCamera(device.id)
            })
        }
        return actions
    }

    /// Runs a core call, closing the palette after it unless `closing` is false (when the
    /// action shows its own feedback); a failure shows under the search field instead.
    private func attempt(closing: Bool = true, _ body: () throws -> Void) {
        do {
            try body()
            if closing { onClose?() }
        } catch {
            report(error)
        }
    }
}

extension SearchResult {
    /// The primary action's icon in the actions menu.
    var primarySymbol: String {
        if completion != nil { return "arrow.right.to.line" }
        switch kind {
        case .application, .settingsPane: return "arrow.up.forward.app"
        case .quicklink: return "link"
        case .calculator: return "doc.on.doc"
        case .systemAction, .command: return "play"
        case .windowAction: return "macwindow"
        case .snippet: return "text.insert"
        }
    }
}

// MARK: - View

struct ActionsMenuView: View {
    let model: PaletteModel
    @Bindable var menu: ActionsMenu

    var body: some View {
        VStack(spacing: 0) {
            list
                .frame(height: ActionsMenuLayout.listHeight(rows: menu.filtered.count))
            Divider()
            SearchField(text: $menu.query, placeholder: "Search for actions…", fontSize: 14, focusRequest: 1) { command in
                switch command {
                case .up: menu.move(by: -1)
                case .down: menu.move(by: 1)
                case .submit, .alternateSubmit:
                    if let action = menu.selected { model.run(action) }
                case .cancel: model.closeActions()
                default: break
                }
            }
            .padding(.horizontal, 14)
            .frame(height: ActionsMenuLayout.fieldHeight)
        }
        .frame(width: ActionsMenuLayout.width)
        // Lighter than the palette behind it, so it reads as a layer on top.
        .background(
            Color.appearance(light: .white.opacity(0.85), dark: Color(white: 0.17).opacity(0.82)),
            in: .rect(cornerRadius: ActionsMenuLayout.cornerRadius)
        )
        .glassEffect(.regular.tint(.white.opacity(0.06)), in: .rect(cornerRadius: ActionsMenuLayout.cornerRadius))
        .overlay {
            RoundedRectangle(cornerRadius: ActionsMenuLayout.cornerRadius)
                .strokeBorder(Color.appearance(light: .black.opacity(0.1), dark: .white.opacity(0.16)), lineWidth: 1)
        }
        .shadow(color: .appearance(light: .black.opacity(0.18), dark: .black.opacity(0.35)), radius: 18, y: 6)
    }

    private var list: some View {
        let actions = menu.filtered
        return ScrollViewReader { proxy in
            ScrollView {
                LazyVStack(spacing: 0) {
                    if actions.isEmpty {
                        Text("No matching actions")
                            .font(.system(size: 13))
                            .foregroundStyle(.secondary)
                            .frame(maxWidth: .infinity, alignment: .leading)
                            .padding(.horizontal, 10)
                            .frame(height: ActionsMenuLayout.rowHeight)
                    }
                    ForEach(Array(actions.enumerated()), id: \.element.id) { index, action in
                        ActionRow(action: action, isSelected: index == menu.selection,
                                  isConfirming: menu.confirming == action.id)
                            .id(action.id)
                            .contentShape(.rect)
                            .onHover { inside in
                                if inside { menu.selection = index }
                            }
                            .onTapGesture {
                                menu.selection = index
                                model.run(action)
                            }
                    }
                }
                .padding(ActionsMenuLayout.padding)
            }
            .scrollIndicators(.never)
            .onChange(of: menu.selection) {
                if let id = menu.selected?.id { proxy.scrollTo(id) }
            }
        }
    }
}

private struct ActionRow: View {
    let action: PaletteAction
    let isSelected: Bool
    let isConfirming: Bool

    var body: some View {
        HStack(spacing: 10) {
            Image(systemName: action.symbol)
                .font(.system(size: 12, weight: .medium))
                .frame(width: 18)
                .foregroundStyle(action.isDestructive ? AnyShapeStyle(.red) : AnyShapeStyle(.secondary))
            Text(isConfirming ? "\(action.title)? Press ↩ to confirm" : action.title)
                .font(.system(size: 13, weight: isConfirming ? .semibold : .regular))
                .foregroundStyle(action.isDestructive ? AnyShapeStyle(.red) : AnyShapeStyle(.primary))
                .lineLimit(1)
            Spacer(minLength: 8)
            if !action.keys.isEmpty {
                Keycaps(keys: action.keys)
            }
        }
        .padding(.horizontal, 10)
        .frame(height: ActionsMenuLayout.rowHeight)
        .background {
            if isSelected {
                RoundedRectangle(cornerRadius: 8).fill(Color.appearance(light: .black.opacity(0.08), dark: .white.opacity(0.14)))
            }
        }
    }
}
