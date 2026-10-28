import SwiftUI

// MARK: - Windows

struct WindowsPane: View {
    let settings: LauncherSettings

    /// The gap while its slider is dragged; saved once the drag ends, so every step of the
    /// drag doesn't re-save the settings and re-register the hotkeys.
    @State private var draggedGap: Double?

    var body: some View {
        @Bindable var settings = settings

        Form {
            PaneHeaderSection(pane: .windows)

            SaveErrorSection(error: settings.saveError)

            Section {
                Toggle("Use window hotkeys", isOn: $settings.config.windows.hotkeysEnabled)
                LabeledContent("Gap between windows") {
                    HStack(spacing: 10) {
                        Slider(value: gap, in: 0...40, step: 1) { editing in
                            if editing {
                                draggedGap = Double(settings.config.windows.gap)
                            } else if let draggedGap {
                                settings.config.windows.gap = UInt32(draggedGap.rounded())
                                self.draggedGap = nil
                            }
                        }
                        .frame(width: 180)
                        Text("\(Int((draggedGap ?? Double(settings.config.windows.gap)).rounded())) pt")
                            .monospacedDigit()
                            .foregroundStyle(.secondary)
                            .frame(width: 40, alignment: .trailing)
                    }
                }
                Picker("Almost Maximize size", selection: $settings.config.windows.almostMaximizePercent) {
                    ForEach(almostMaximizeOptions, id: \.self) { percent in
                        Text("\(percent)%").tag(percent)
                    }
                }
            } footer: {
                Text("Moving windows needs Accessibility access. Press a half's hotkey again to cycle its size through ½, ⅔ and ⅓. Every action is also in launcher search, hotkeys or not.")
            }

            ForEach(WindowActionGroup.allCases) { group in
                Section(group.title) {
                    ForEach(group.actions) { action in
                        WindowActionRow(
                            action: action,
                            hotkey: Binding(
                                get: { settings.windowHotkey(for: action) },
                                set: { settings.setWindowHotkey($0, for: action) }
                            ),
                            problem: settings.windowHotkey(for: action).flatMap {
                                settings.problem(with: $0, in: .window(action))
                            }
                        )
                    }
                }
                .disabled(!settings.config.windows.hotkeysEnabled)
            }

            Section {
                HStack {
                    Spacer()
                    Button("Restore Defaults") {
                        settings.config.windows = defaultWindowConfig()
                    }
                    .disabled(settings.config.windows == defaultWindowConfig())
                }
            }
        }
        .formStyle(.grouped)
    }

    /// The presets, plus the current size if it was set by hand in the config.
    private var almostMaximizeOptions: [UInt32] {
        let presets: [UInt32] = [60, 70, 80, 90, 95]
        let current = settings.config.windows.almostMaximizePercent
        return presets.contains(current) ? presets : (presets + [current]).sorted()
    }

    private var gap: Binding<Double> {
        Binding(
            get: { draggedGap ?? Double(settings.config.windows.gap) },
            set: { value in
                if draggedGap != nil {
                    draggedGap = value
                } else {
                    settings.config.windows.gap = UInt32(value.rounded())
                }
            }
        )
    }
}

private struct WindowActionRow: View {
    let action: WindowAction
    @Binding var hotkey: Hotkey?
    let problem: String?

    var body: some View {
        HStack(spacing: 10) {
            WindowActionIcon(action: action)
                .foregroundStyle(.secondary)
                .frame(width: 22)
            Text(action.title)
            Spacer()
            if let problem {
                Image(systemName: "exclamationmark.triangle.fill")
                    .foregroundStyle(.yellow)
                    .help(problem)
            }
            HotkeyField(hotkey: $hotkey)
        }
    }
}

// MARK: - Snippets

extension Snippet: Identifiable {}

struct SnippetsPane: View {
    let settings: LauncherSettings

    @State private var draft: Draft<Snippet>?

    var body: some View {
        @Bindable var settings = settings

        Form {
            PaneHeaderSection(pane: .snippets)

            SaveErrorSection(error: settings.saveError)

            Section {
                Toggle("Expand keywords as you type", isOn: $settings.config.snippets.expansion)
            } footer: {
                Text("Type a snippet's keyword in any app and it's replaced with the snippet. Needs Accessibility access.")
            }

            Section {
                Toggle("Choosing an item pastes it", isOn: $settings.config.snippets.pasteOnOpen)
            } footer: {
                Text(settings.config.snippets.pasteOnOpen
                    ? "Snippets are also in launcher search: ↩ pastes into the app in front; ⌘↩ only copies. Pasting needs Accessibility access."
                    : "Snippets are also in launcher search: ↩ only copies; ⌘↩ pastes into the app in front. Pasting needs Accessibility access.")
            }

            Section {
                ForEach(settings.config.snippets.snippets) { snippet in
                    ExtensionRow(
                        symbol: "text.quote",
                        name: snippet.name,
                        keyword: snippet.keyword,
                        detail: snippet.text.oneLine,
                        detailTruncation: .tail,
                        problem: settings.snippetKeywordProblem(snippet.keyword, excludingID: snippet.id),
                        onEdit: { draft = Draft(item: snippet, isNew: false) },
                        onRemove: { settings.config.snippets.snippets.removeAll { $0.id == snippet.id } }
                    )
                }
                if settings.config.snippets.snippets.isEmpty {
                    Text("No snippets").foregroundStyle(.secondary)
                }
                Button("Add Snippet", systemImage: "plus") {
                    draft = Draft(item: Snippet(id: UUID().uuidString, name: "", keyword: "", text: ""), isNew: true)
                }
            }
        }
        .formStyle(.grouped)
        .sheet(item: $draft) { draft in
            SnippetEditor(draft: draft, settings: settings) { self.draft = nil }
        }
    }
}

private struct SnippetEditor: View {
    @State var draft: Draft<Snippet>
    let settings: LauncherSettings
    let onDone: () -> Void

    @State private var selection: TextSelection?

    private static let placeholders: [(title: String, placeholder: String)] = [
        ("Clipboard Text", "{clipboard}"),
        ("Date", "{date}"),
        ("Time", "{time}"),
        ("Date and Time", "{datetime}"),
        ("Cursor Position", "{cursor}"),
    ]

    private var isValid: Bool {
        !draft.item.name.trimmingCharacters(in: .whitespaces).isEmpty && !draft.item.text.isEmpty
    }

    private var keywordProblem: String? {
        settings.snippetKeywordProblem(draft.item.keyword, excludingID: draft.item.id)
    }

    var body: some View {
        EditorSheet(title: draft.isNew ? "New Snippet" : "Edit Snippet", canSave: isValid) {
            Form {
                TextField("Name", text: $draft.item.name, prompt: Text("Home Address"))
                LabeledContent {
                    HStack(spacing: 6) {
                        TextField("Keyword", text: $draft.item.keyword, prompt: Text("Optional, like ;addr"))
                            .labelsHidden()
                            .font(.system(.body, design: .monospaced))
                            .autocorrectionDisabled()
                        if let keywordProblem {
                            Image(systemName: "exclamationmark.triangle.fill")
                                .foregroundStyle(.yellow)
                                .help(keywordProblem)
                        }
                    }
                } label: {
                    Text("Keyword")
                }
                LabeledContent("Text") {
                    TextEditor(text: $draft.item.text, selection: $selection)
                        .font(.system(.body, design: .monospaced))
                        .autocorrectionDisabled()
                        .scrollContentBackground(.hidden)
                        .padding(4)
                        .background(.background.secondary, in: .rect(cornerRadius: 6))
                        .frame(minHeight: 140)
                }
                Section {
                    EmptyView()
                } footer: {
                    VStack(alignment: .leading, spacing: 6) {
                        if let keywordProblem {
                            Label(keywordProblem, systemImage: "exclamationmark.triangle.fill")
                                .foregroundStyle(.orange)
                        }
                        Text("Start keywords with a character you don't otherwise type, like `;`, so they don't expand by accident. Placeholders are filled in when the snippet is inserted; `{cursor}` is where the caret ends up. Write `{{` and `}}` for literal braces.")
                    }
                }
            }
            .formStyle(.grouped)
        } leading: {
            Menu("Insert Placeholder") {
                ForEach(Self.placeholders, id: \.placeholder) { item in
                    Button("\(item.title)  \(item.placeholder)") { insert(item.placeholder) }
                }
            }
            .fixedSize()
        } onCancel: {
            onDone()
        } onSave: {
            var snippet = draft.item
            snippet.name = snippet.name.trimmingCharacters(in: .whitespaces)
            snippet.keyword = snippet.keyword.trimmingCharacters(in: .whitespaces)
            settings.save(snippet)
            onDone()
        }
    }

    /// Replaces the selection (or inserts at the caret) with `placeholder`; appends when the
    /// text editor has no selection to go by.
    private func insert(_ placeholder: String) {
        var text = draft.item.text
        if case let .selection(range)? = selection?.indices,
           range.lowerBound >= text.startIndex, range.upperBound <= text.endIndex {
            let start = text.distance(from: text.startIndex, to: range.lowerBound)
            text.replaceSubrange(range, with: placeholder)
            let caret = text.index(text.startIndex, offsetBy: start + placeholder.count)
            draft.item.text = text
            selection = TextSelection(insertionPoint: caret)
        } else {
            draft.item.text = text + placeholder
            selection = TextSelection(insertionPoint: draft.item.text.endIndex)
        }
    }
}
