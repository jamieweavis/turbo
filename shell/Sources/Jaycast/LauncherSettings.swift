import AppKit
import Observation

/// The launcher settings the UI edits, and the installed apps to pick hotkeys for. Settings
/// changes are written through to the core, which saves them and re-registers the hotkeys.
@MainActor
@Observable
final class LauncherSettings {
    var config: LauncherConfig {
        didSet {
            if config != oldValue { save() }
        }
    }
    private(set) var apps: [AppInfo] = []
    private(set) var saveError: String?
    /// Hotkeys the system wouldn't register (usually taken by another app).
    private(set) var failed: [Hotkey] = []
    @ObservationIgnored private let core: Core

    init(core: Core) {
        self.core = core
        config = core.launcherConfig()
    }

    /// Re-reads which hotkeys failed to register; call after the core (re)registers them.
    func refreshFailed() {
        failed = core.failedHotkeys()
    }

    /// Why `hotkey` won't work, if it won't: used twice, or claimed by another app.
    func problem(with hotkey: Hotkey, in slot: HotkeySlot) -> String? {
        if isDuplicate(hotkey, in: slot) {
            return "This hotkey is used more than once"
        }
        if failed.contains(where: { $0.matches(hotkey) }) {
            return "Another app is already using this shortcut"
        }
        return nil
    }

    func refreshApps() {
        apps = core.installedApps()
    }

    func addAppHotkey(_ app: AppInfo, hotkey: Hotkey) {
        config.appHotkeys.append(AppHotkey(bundleId: app.bundleId, name: app.name, hotkey: hotkey))
    }

    /// Every global hotkey in the config, with where it's set.
    var assignedHotkeys: [(slot: HotkeySlot, hotkey: Hotkey)] {
        var assigned: [(slot: HotkeySlot, hotkey: Hotkey)] = []
        if let hotkey = config.hotkey { assigned.append((.palette, hotkey)) }
        if let hotkey = config.clipboard.hotkey { assigned.append((.clipboard, hotkey)) }
        if let hotkey = config.files.hotkey { assigned.append((.files, hotkey)) }
        for (index, app) in config.appHotkeys.enumerated() {
            assigned.append((.app(index), app.hotkey))
        }
        if config.windows.hotkeysEnabled {
            for window in config.windows.hotkeys {
                assigned.append((.window(window.action), window.hotkey))
            }
        }
        let caffeinate = config.caffeinate
        if let hotkey = caffeinate.toggleHotkey { assigned.append((.caffeinate(.toggle), hotkey)) }
        if let hotkey = caffeinate.onHotkey { assigned.append((.caffeinate(.on), hotkey)) }
        if let hotkey = caffeinate.offHotkey { assigned.append((.caffeinate(.off), hotkey)) }
        if let hotkey = config.colorPicker.hotkey { assigned.append((.colorPicker, hotkey)) }
        return assigned
    }

    /// Whether `hotkey` is also set somewhere other than `slot`.
    func isDuplicate(_ hotkey: Hotkey, in slot: HotkeySlot) -> Bool {
        assignedHotkeys.contains { $0.slot != slot && $0.hotkey.matches(hotkey) }
    }

    // MARK: Clipboard

    /// Deletes every unpinned clipboard item.
    func clearClipboardHistory() throws {
        try core.clearClipboardHistory()
    }

    /// The display name for an ignored app's bundle id: the index, then Launch Services.
    func appName(forBundleID bundleID: String) -> String {
        if let app = apps.first(where: { $0.bundleId == bundleID }) {
            return app.name
        }
        guard let url = NSWorkspace.shared.urlForApplication(withBundleIdentifier: bundleID) else {
            return bundleID
        }
        return FileManager.default.displayName(atPath: url.path(percentEncoded: false))
            .replacingOccurrences(of: ".app", with: "")
    }

    // MARK: Quicklinks and commands

    /// Adds `quicklink`, or replaces the one with its id.
    func save(_ quicklink: Quicklink) {
        if let index = config.quicklinks.firstIndex(where: { $0.id == quicklink.id }) {
            config.quicklinks[index] = quicklink
        } else {
            config.quicklinks.append(quicklink)
        }
    }

    func save(_ command: ShellCommand) {
        if let index = config.commands.firstIndex(where: { $0.id == command.id }) {
            config.commands[index] = command
        } else {
            config.commands.append(command)
        }
    }

    /// Whether another quicklink or command (not `id`) already uses `keyword`. Keywords
    /// decide what `<keyword> <argument>` runs, so they must be unique across both lists.
    func isKeywordTaken(_ keyword: String, excludingID id: String) -> Bool {
        let keyword = keyword.trimmingCharacters(in: .whitespaces).lowercased()
        guard !keyword.isEmpty else { return false }
        let quicklinks = config.quicklinks.lazy.filter { $0.id != id }.map(\.keyword)
        let commands = config.commands.lazy.filter { $0.id != id }.map(\.keyword)
        return (Array(quicklinks) + Array(commands)).contains { $0.trimmingCharacters(in: .whitespaces).lowercased() == keyword }
    }

    // MARK: Windows

    func windowHotkey(for action: WindowAction) -> Hotkey? {
        config.windows.hotkeys.first { $0.action == action }?.hotkey
    }

    /// Sets or (with `nil`) clears an action's hotkey, keeping the others' order.
    func setWindowHotkey(_ hotkey: Hotkey?, for action: WindowAction) {
        var hotkeys = config.windows.hotkeys
        if let index = hotkeys.firstIndex(where: { $0.action == action }) {
            if let hotkey {
                hotkeys[index].hotkey = hotkey
            } else {
                hotkeys.remove(at: index)
            }
        } else if let hotkey {
            hotkeys.append(WindowHotkey(action: action, hotkey: hotkey))
        }
        config.windows.hotkeys = hotkeys
    }

    // MARK: Snippets

    /// Adds `snippet`, or replaces the one with its id.
    func save(_ snippet: Snippet) {
        if let index = config.snippets.snippets.firstIndex(where: { $0.id == snippet.id }) {
            config.snippets.snippets[index] = snippet
        } else {
            config.snippets.snippets.append(snippet)
        }
    }

    /// Why `keyword` won't expand as expected next to the other snippets' (not `id`), if it
    /// won't: expansion fires as soon as a keyword is typed, so a keyword that starts another
    /// one always wins over it.
    func snippetKeywordProblem(_ keyword: String, excludingID id: String) -> String? {
        let keyword = keyword.trimmingCharacters(in: .whitespaces)
        guard !keyword.isEmpty else { return nil }
        let others = config.snippets.snippets
            .filter { $0.id != id }
            .map { $0.keyword.trimmingCharacters(in: .whitespaces) }
            .filter { !$0.isEmpty }
        if others.contains(keyword) {
            return "Another snippet uses this keyword"
        }
        if let shorter = others.first(where: { keyword.hasPrefix($0) }) {
            return "Never expands: “\(shorter)” is typed first and expands instead"
        }
        if let longer = others.first(where: { $0.hasPrefix(keyword) }) {
            return "Stops “\(longer)” from expanding, since this keyword is typed first"
        }
        return nil
    }

    /// The app's bundle path for its icon: the index first, then Launch Services.
    func path(forBundleID bundleID: String) -> String? {
        apps.first { $0.bundleId == bundleID }?.path
            ?? NSWorkspace.shared.urlForApplication(withBundleIdentifier: bundleID)?.path(percentEncoded: false)
    }

    private func save() {
        do {
            try core.setLauncherConfig(launcher: config)
            saveError = nil
            refreshFailed()
        } catch {
            saveError = error.userMessage
        }
    }
}

/// Where a global hotkey is set, for telling duplicates apart from the hotkey itself.
enum HotkeySlot: Equatable {
    case palette, clipboard, files
    case app(Int)
    case window(WindowAction)
    case caffeinate(CaffeinateHotkey)
    case colorPicker
}

enum CaffeinateHotkey {
    case toggle, on, off
}

/// Receives the core's launcher callbacks (hotkeys on the main thread, app index changes and
/// command results on background threads) and forwards them, in order, to the main actor.
final class LauncherObserverBridge: LauncherObserver {
    private let onTogglePalette: @MainActor @Sendable () -> Void
    private let onAppsChanged: @MainActor @Sendable () -> Void
    private let onCommandFinished: @MainActor @Sendable (_ name: String, _ output: String, _ success: Bool) -> Void
    private let onShowMode: @MainActor @Sendable (PaletteMode) -> Void
    private let onClipboardChanged: @MainActor @Sendable () -> Void
    private let onCaffeinateChanged: @MainActor @Sendable (Bool) -> Void
    private let onPickColor: @MainActor @Sendable () -> Void

    init(
        onTogglePalette: @escaping @MainActor @Sendable () -> Void,
        onAppsChanged: @escaping @MainActor @Sendable () -> Void,
        onCommandFinished: @escaping @MainActor @Sendable (_ name: String, _ output: String, _ success: Bool) -> Void,
        onShowMode: @escaping @MainActor @Sendable (PaletteMode) -> Void,
        onClipboardChanged: @escaping @MainActor @Sendable () -> Void,
        onCaffeinateChanged: @escaping @MainActor @Sendable (Bool) -> Void,
        onPickColor: @escaping @MainActor @Sendable () -> Void
    ) {
        self.onTogglePalette = onTogglePalette
        self.onAppsChanged = onAppsChanged
        self.onCommandFinished = onCommandFinished
        self.onShowMode = onShowMode
        self.onClipboardChanged = onClipboardChanged
        self.onCaffeinateChanged = onCaffeinateChanged
        self.onPickColor = onPickColor
    }

    func togglePalette() {
        DispatchQueue.main.async { [onTogglePalette] in
            MainActor.assumeIsolated { onTogglePalette() }
        }
    }

    func appsChanged() {
        DispatchQueue.main.async { [onAppsChanged] in
            MainActor.assumeIsolated { onAppsChanged() }
        }
    }

    func showMode(mode: PaletteMode) {
        DispatchQueue.main.async { [onShowMode] in
            MainActor.assumeIsolated { onShowMode(mode) }
        }
    }

    func clipboardChanged() {
        DispatchQueue.main.async { [onClipboardChanged] in
            MainActor.assumeIsolated { onClipboardChanged() }
        }
    }

    func caffeinateChanged(active: Bool) {
        DispatchQueue.main.async { [onCaffeinateChanged] in
            MainActor.assumeIsolated { onCaffeinateChanged(active) }
        }
    }

    func pickColor() {
        DispatchQueue.main.async { [onPickColor] in
            MainActor.assumeIsolated { onPickColor() }
        }
    }

    func commandFinished(name: String, output: String, success: Bool) {
        DispatchQueue.main.async { [onCommandFinished] in
            MainActor.assumeIsolated { onCommandFinished(name, output, success) }
        }
    }
}

/// App icons by bundle path, cached since the palette redraws rows on every keystroke.
@MainActor
enum AppIcons {
    private static var cache: [String: NSImage] = [:]
    private static var pathsByBundleID: [String: String?] = [:]

    /// The installed app's icon for a bundle id, if Launch Services knows the app.
    static func icon(forBundleID bundleID: String) -> NSImage? {
        let path = pathsByBundleID[bundleID] ?? {
            let path = NSWorkspace.shared.urlForApplication(withBundleIdentifier: bundleID)?.path(percentEncoded: false)
            pathsByBundleID[bundleID] = path
            return path
        }()
        return path.map(icon(forPath:))
    }

    static func icon(forPath path: String) -> NSImage {
        if let icon = cache[path] { return icon }
        // Resolve symlinks (e.g. /Applications/Safari.app) so the icon has no alias badge.
        let resolved = URL(fileURLWithPath: path).resolvingSymlinksInPath().path(percentEncoded: false)
        let icon = NSWorkspace.shared.icon(forFile: resolved)
        cache[path] = icon
        return icon
    }
}
