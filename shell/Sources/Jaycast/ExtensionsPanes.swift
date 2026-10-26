import AppKit
import SwiftUI

extension Quicklink: Identifiable {}
extension ShellCommand: Identifiable {}

/// Something being added or edited in a sheet; `isNew` decides the sheet's title.
private struct Draft<Item: Identifiable>: Identifiable {
    var item: Item
    let isNew: Bool
    var id: Item.ID { item.id }
}

// MARK: - Quicklinks

struct QuicklinksPane: View {
    let settings: LauncherSettings

    @State private var draft: Draft<Quicklink>?

    var body: some View {
        Form {
            SaveErrorSection(error: settings.saveError)

            Section {
                ForEach(settings.config.quicklinks) { quicklink in
                    ExtensionRow(
                        symbol: "link",
                        name: quicklink.name,
                        keyword: quicklink.keyword,
                        detail: quicklink.url,
                        problem: settings.isKeywordTaken(quicklink.keyword, excludingID: quicklink.id)
                            ? "Another quicklink or command uses this keyword" : nil,
                        onEdit: { draft = Draft(item: quicklink, isNew: false) },
                        onRemove: { settings.config.quicklinks.removeAll { $0.id == quicklink.id } }
                    )
                }
                if settings.config.quicklinks.isEmpty {
                    Text("No quicklinks").foregroundStyle(.secondary)
                }
                Button("Add Quicklink", systemImage: "plus") {
                    draft = Draft(item: Quicklink(id: UUID().uuidString, name: "", keyword: "", url: ""), isNew: true)
                }
            } footer: {
                Text("Type a quicklink's keyword, a space and your search, like “g rust traits”, then press ↩.")
            }
        }
        .formStyle(.grouped)
        .sheet(item: $draft) { draft in
            QuicklinkEditor(draft: draft, settings: settings) { self.draft = nil }
        }
    }
}

private struct QuicklinkEditor: View {
    @State var draft: Draft<Quicklink>
    let settings: LauncherSettings
    let onDone: () -> Void

    private var isValid: Bool {
        !draft.item.name.trimmingCharacters(in: .whitespaces).isEmpty
            && !draft.item.url.trimmingCharacters(in: .whitespaces).isEmpty
    }

    var body: some View {
        EditorSheet(title: draft.isNew ? "New Quicklink" : "Edit Quicklink", canSave: isValid) {
            Form {
                TextField("Name", text: $draft.item.name, prompt: Text("Search Google"))
                KeywordField(keyword: $draft.item.keyword, isTaken: settings.isKeywordTaken(draft.item.keyword, excludingID: draft.item.id))
                TextField("URL", text: $draft.item.url, prompt: Text("https://www.google.com/search?q={query}"))
                    .font(.system(.body, design: .monospaced))
                    .autocorrectionDisabled()
                Section {
                    EmptyView()
                } footer: {
                    Text("`{query}` is replaced with what you type after the keyword, and `{clipboard}` with the clipboard's text. Both are URL-encoded.")
                }
            }
            .formStyle(.grouped)
        } leading: {
            Button("Test") { test() }
                .disabled(draft.item.url.trimmingCharacters(in: .whitespaces).isEmpty)
                .help("Open the URL with “test” as the query")
        } onCancel: {
            onDone()
        } onSave: {
            settings.save(trimmed(draft.item))
            onDone()
        }
    }

    private func trimmed(_ quicklink: Quicklink) -> Quicklink {
        var quicklink = quicklink
        quicklink.name = quicklink.name.trimmingCharacters(in: .whitespaces)
        quicklink.keyword = quicklink.keyword.trimmingCharacters(in: .whitespaces)
        quicklink.url = quicklink.url.trimmingCharacters(in: .whitespaces)
        return quicklink
    }

    /// Fills the placeholders roughly as the core does, to try a template before saving.
    private func test() {
        let encode = { (text: String) in
            text.addingPercentEncoding(withAllowedCharacters: .urlQueryAllowed.subtracting(CharacterSet(charactersIn: "&+=?#"))) ?? text
        }
        let clipboard = NSPasteboard.general.string(forType: .string) ?? ""
        let url = draft.item.url
            .trimmingCharacters(in: .whitespaces)
            .replacingOccurrences(of: "{query}", with: encode("test"))
            .replacingOccurrences(of: "{clipboard}", with: encode(clipboard))
        if let url = URL(string: url) {
            NSWorkspace.shared.open(url)
        } else {
            NSSound.beep()
        }
    }
}

// MARK: - Commands

struct CommandsPane: View {
    let settings: LauncherSettings

    @State private var draft: Draft<ShellCommand>?

    var body: some View {
        Form {
            SaveErrorSection(error: settings.saveError)

            Section {
                ForEach(settings.config.commands) { command in
                    ExtensionRow(
                        symbol: "terminal",
                        name: command.name,
                        keyword: command.keyword,
                        detail: command.command,
                        problem: settings.isKeywordTaken(command.keyword, excludingID: command.id)
                            ? "Another quicklink or command uses this keyword" : nil,
                        onEdit: { draft = Draft(item: command, isNew: false) },
                        onRemove: { settings.config.commands.removeAll { $0.id == command.id } }
                    )
                }
                if settings.config.commands.isEmpty {
                    Text("No commands").foregroundStyle(.secondary)
                }
                Button("Add Command", systemImage: "plus") {
                    draft = Draft(
                        item: ShellCommand(
                            id: UUID().uuidString,
                            name: "",
                            keyword: "",
                            command: "",
                            output: .discard,
                            needsConfirmation: false
                        ),
                        isNew: true
                    )
                }
            } footer: {
                Text("Run shell commands from the launcher by name, or by keyword followed by an argument.")
            }
        }
        .formStyle(.grouped)
        .sheet(item: $draft) { draft in
            CommandEditor(draft: draft, settings: settings) { self.draft = nil }
        }
    }
}

private struct CommandEditor: View {
    @State var draft: Draft<ShellCommand>
    let settings: LauncherSettings
    let onDone: () -> Void

    private var isValid: Bool {
        !draft.item.name.trimmingCharacters(in: .whitespaces).isEmpty
            && !draft.item.command.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
    }

    var body: some View {
        EditorSheet(title: draft.isNew ? "New Command" : "Edit Command", canSave: isValid) {
            Form {
                TextField("Name", text: $draft.item.name, prompt: Text("Flush DNS"))
                KeywordField(keyword: $draft.item.keyword, isTaken: settings.isKeywordTaken(draft.item.keyword, excludingID: draft.item.id))
                LabeledContent("Command") {
                    TextEditor(text: $draft.item.command)
                        .font(.system(.body, design: .monospaced))
                        .autocorrectionDisabled()
                        .scrollContentBackground(.hidden)
                        .padding(4)
                        .background(.background.secondary, in: .rect(cornerRadius: 6))
                        .frame(minHeight: 90)
                }
                Picker("Output", selection: $draft.item.output) {
                    Text("Discard").tag(CommandOutput.discard)
                    Text("Show output").tag(CommandOutput.show)
                    Text("Copy output").tag(CommandOutput.copy)
                }
                Toggle("Ask before running", isOn: $draft.item.needsConfirmation)
                Section {
                    EmptyView()
                } footer: {
                    Text("Runs with `/bin/zsh -lc` in your home folder. `{query}` is replaced with what you type after the keyword, and `{clipboard}` with the clipboard's text. Both are shell-quoted for you, so write them bare: `echo {query}`, not `echo \"{query}\"`, which would add literal quote marks. Failures are always shown.")
                }
            }
            .formStyle(.grouped)
        } leading: {
            EmptyView()
        } onCancel: {
            onDone()
        } onSave: {
            var command = draft.item
            command.name = command.name.trimmingCharacters(in: .whitespaces)
            command.keyword = command.keyword.trimmingCharacters(in: .whitespaces)
            command.command = command.command.trimmingCharacters(in: .whitespacesAndNewlines)
            settings.save(command)
            onDone()
        }
    }
}

// MARK: - Shared pieces

private struct SaveErrorSection: View {
    let error: String?

    var body: some View {
        if let error {
            Section {
                Label(error, systemImage: "exclamationmark.triangle.fill")
                    .foregroundStyle(.red)
            }
        }
    }
}

private struct ExtensionRow: View {
    let symbol: String
    let name: String
    let keyword: String
    let detail: String
    let problem: String?
    let onEdit: () -> Void
    let onRemove: () -> Void

    var body: some View {
        HStack(spacing: 10) {
            Image(systemName: symbol)
                .foregroundStyle(.secondary)
                .frame(width: 20)
            VStack(alignment: .leading, spacing: 2) {
                HStack(spacing: 6) {
                    Text(name)
                    if !keyword.isEmpty {
                        Text(keyword)
                            .font(.caption.monospaced())
                            .padding(.horizontal, 5)
                            .padding(.vertical, 1)
                            .background(.quaternary, in: .rect(cornerRadius: 4))
                    }
                }
                Text(detail)
                    .font(.caption.monospaced())
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
                    .truncationMode(.middle)
            }
            Spacer()
            if let problem {
                Image(systemName: "exclamationmark.triangle.fill")
                    .foregroundStyle(.yellow)
                    .help(problem)
            }
            Button("Edit", systemImage: "pencil", action: onEdit)
                .labelStyle(.iconOnly)
                .buttonStyle(.borderless)
            Button("Remove", systemImage: "minus.circle", action: onRemove)
                .labelStyle(.iconOnly)
                .buttonStyle(.borderless)
        }
        .contentShape(.rect)
        .onTapGesture(count: 2, perform: onEdit)
    }
}

private struct KeywordField: View {
    @Binding var keyword: String
    let isTaken: Bool

    var body: some View {
        LabeledContent {
            HStack(spacing: 6) {
                TextField("Keyword", text: $keyword, prompt: Text("Optional: defaults to the name"))
                    .labelsHidden()
                    .autocorrectionDisabled()
                if isTaken {
                    Image(systemName: "exclamationmark.triangle.fill")
                        .foregroundStyle(.yellow)
                        .help("Another quicklink or command uses this keyword")
                }
            }
        } label: {
            Text("Keyword")
        }
    }
}

/// A sheet with a title, a form, and Cancel / Save buttons.
private struct EditorSheet<Content: View, Leading: View>: View {
    let title: String
    let canSave: Bool
    @ViewBuilder let content: Content
    @ViewBuilder let leading: Leading
    let onCancel: () -> Void
    let onSave: () -> Void

    var body: some View {
        VStack(spacing: 0) {
            Text(title)
                .font(.headline)
                .padding(.top, 16)
            content
            HStack {
                leading
                Spacer()
                Button("Cancel", action: onCancel)
                    .keyboardShortcut(.cancelAction)
                Button("Save", action: onSave)
                    .keyboardShortcut(.defaultAction)
                    .disabled(!canSave)
            }
            .padding(16)
        }
        .frame(width: 520)
        .fixedSize(horizontal: false, vertical: true)
    }
}
