import Observation
import SwiftUI
import os

/// The core's recently used emoji, refreshed when the palette inserts one.
@MainActor
@Observable
final class EmojiState {
    private(set) var recent: [EmojiInfo] = []
    private(set) var error: String?
    @ObservationIgnored private let core: Core

    init(core: Core) {
        self.core = core
        refresh()
    }

    func refresh() {
        recent = core.recentEmoji()
    }

    func removeRecent(_ emoji: String) {
        do {
            try core.removeRecentEmoji(emoji: emoji)
            error = nil
        } catch {
            Logger.settings.error("couldn't remove recent emoji: \(error, privacy: .public)")
            self.error = error.userMessage
        }
        refresh()
    }

    func clearRecent() {
        do {
            try core.clearRecentEmoji()
            error = nil
        } catch {
            Logger.settings.error("couldn't clear recent emoji: \(error, privacy: .public)")
            self.error = error.userMessage
        }
        refresh()
    }
}

struct EmojiPane: View {
    let state: EmojiState
    let settings: LauncherSettings

    @State private var isConfirmingClear = false

    var body: some View {
        @Bindable var settings = settings

        Form {
            PaneHeaderSection(pane: .emoji)

            if let error = state.error ?? settings.saveError {
                Section {
                    Label(error, systemImage: "exclamationmark.triangle.fill")
                        .foregroundStyle(.red)
                }
            }

            Section {
                LabeledContent("Open emoji picker") {
                    HStack(spacing: 6) {
                        if let hotkey = settings.emojiHotkey,
                           let problem = settings.problem(with: hotkey, in: .item(LauncherSettings.emojiItemID)) {
                            Image(systemName: "exclamationmark.triangle.fill")
                                .foregroundStyle(.yellow)
                                .help(problem)
                        }
                        HotkeyField(hotkey: $settings.emojiHotkey)
                    }
                }
            } footer: {
                Text("Or search for “Search Emoji” in the launcher.")
            }

            Section {
                Toggle("Choosing an emoji pastes it", isOn: $settings.config.emoji.pasteOnOpen)
            } footer: {
                Text(settings.config.emoji.pasteOnOpen
                    ? "↩ pastes into the app in front; ⌘↩ only copies. Pasting needs Accessibility access."
                    : "↩ only copies; ⌘↩ pastes into the app in front. Pasting needs Accessibility access.")
            }

            Section {
                if state.recent.isEmpty {
                    Text("Emoji you use appear here.")
                        .foregroundStyle(.secondary)
                } else {
                    LazyVGrid(columns: Array(repeating: GridItem(.flexible(), spacing: 6), count: 12), spacing: 6) {
                        ForEach(state.recent, id: \.emoji) { emoji in
                            Text(emoji.emoji)
                                .font(.system(size: 26))
                                .frame(maxWidth: .infinity)
                                .help([emoji.name, emoji.shortcode.map { ":\($0):" }].compactMap(\.self).joined(separator: "  "))
                                .contentShape(Rectangle())
                                .contextMenu {
                                    Button("Remove from Recently Used", role: .destructive) {
                                        state.removeRecent(emoji.emoji)
                                    }
                                }
                        }
                    }
                    .padding(.vertical, 4)
                }
            } header: {
                HStack {
                    Text("Recently Used")
                    Spacer()
                    if !state.recent.isEmpty {
                        Button("Clear Recently Used…", role: .destructive) { isConfirmingClear = true }
                            .controlSize(.small)
                    }
                }
            }
        }
        .formStyle(.grouped)
        .onAppear { state.refresh() }
        .confirmationDialog("Clear recently used emoji?", isPresented: $isConfirmingClear) {
            Button("Clear Recently Used", role: .destructive) { state.clearRecent() }
        } message: {
            Text("The emoji picker's Recently Used row is emptied. This can't be undone.")
        }
    }
}
