import AppKit
import Observation

/// The launcher settings the UI edits, and the apps and commands to set hotkeys for. Settings
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
    /// Every app and built-in command, for the Launcher pane's list.
    private(set) var items: [LauncherItem] = []
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
        items = core.launcherItems()
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
        for (index, action) in config.actionHotkeys.enumerated() {
            assigned.append((.action(index), action.hotkey))
        }
        if config.windows.hotkeysEnabled {
            for window in config.windows.hotkeys {
                assigned.append((.window(window.action), window.hotkey))
            }
        }
        let keepAwake = config.keepAwake
        if let hotkey = keepAwake.toggleHotkey { assigned.append((.keepAwake(.toggle), hotkey)) }
        if let hotkey = keepAwake.onHotkey { assigned.append((.keepAwake(.on), hotkey)) }
        if let hotkey = keepAwake.offHotkey { assigned.append((.keepAwake(.off), hotkey)) }
        if let hotkey = config.colorPicker.hotkey { assigned.append((.colorPicker, hotkey)) }
        for item in config.items {
            if let hotkey = item.hotkey { assigned.append((.item(item.id), hotkey)) }
        }
        return assigned
    }

    /// Whether `hotkey` is also set somewhere other than `slot`.
    func isDuplicate(_ hotkey: Hotkey, in slot: HotkeySlot) -> Bool {
        assignedHotkeys.contains { $0.slot != slot && $0.hotkey.matches(hotkey) }
    }

    // MARK: Items

    func isEnabled(_ item: LauncherItem) -> Bool {
        itemSettings(item.id)?.enabled ?? true
    }

    func setEnabled(_ enabled: Bool, for item: LauncherItem) {
        updateItemSettings(item.id) { $0.enabled = enabled }
    }

    func aliases(for item: LauncherItem) -> [String] {
        itemSettings(item.id)?.aliases ?? []
    }

    func setAliases(_ aliases: [String], for item: LauncherItem) {
        updateItemSettings(item.id) { $0.aliases = aliases }
    }

    /// The item's hotkey, from wherever it's kept: apps, system actions, window actions, palette
    /// modes, Keep Awake and the colour picker have their own settings; the rest are kept with
    /// the item's other settings.
    func hotkey(for item: LauncherItem) -> Hotkey? {
        if let action = item.windowAction { return windowHotkey(for: action) }
        if let bundleID = item.id.stripping(prefix: "app:") {
            return config.appHotkeys.first { $0.bundleId == bundleID }?.hotkey
        }
        if let actionID = item.id.stripping(prefix: "system:") {
            return config.actionHotkeys.first { $0.action == actionID }?.hotkey
        }
        switch item.id {
        case "mode:clipboard": return config.clipboard.hotkey
        case "mode:files": return config.files.hotkey
        case "color:pick": return config.colorPicker.hotkey
        case "keep-awake:toggle": return config.keepAwake.toggleHotkey
        case "keep-awake:on": return config.keepAwake.onHotkey
        case "keep-awake:off": return config.keepAwake.offHotkey
        default: return itemSettings(item.id)?.hotkey
        }
    }

    /// Sets or (with `nil`) clears the item's hotkey where `hotkey(for:)` reads it.
    func setHotkey(_ hotkey: Hotkey?, for item: LauncherItem) {
        if let action = item.windowAction {
            setWindowHotkey(hotkey, for: action)
        } else if let bundleID = item.id.stripping(prefix: "app:") {
            let index = config.appHotkeys.firstIndex { $0.bundleId == bundleID }
            switch (index, hotkey) {
            case (let index?, let hotkey?): config.appHotkeys[index].hotkey = hotkey
            case (let index?, nil): config.appHotkeys.remove(at: index)
            case (nil, let hotkey?): config.appHotkeys.append(AppHotkey(bundleId: bundleID, name: item.title, hotkey: hotkey))
            case (nil, nil): break
            }
        } else if let actionID = item.id.stripping(prefix: "system:") {
            let index = config.actionHotkeys.firstIndex { $0.action == actionID }
            switch (index, hotkey) {
            case (let index?, let hotkey?): config.actionHotkeys[index].hotkey = hotkey
            case (let index?, nil): config.actionHotkeys.remove(at: index)
            case (nil, let hotkey?): config.actionHotkeys.append(ActionHotkey(action: actionID, hotkey: hotkey))
            case (nil, nil): break
            }
        } else {
            switch item.id {
            case "mode:clipboard": config.clipboard.hotkey = hotkey
            case "mode:files": config.files.hotkey = hotkey
            case "color:pick": config.colorPicker.hotkey = hotkey
            case "keep-awake:toggle": config.keepAwake.toggleHotkey = hotkey
            case "keep-awake:on": config.keepAwake.onHotkey = hotkey
            case "keep-awake:off": config.keepAwake.offHotkey = hotkey
            default: updateItemSettings(item.id) { $0.hotkey = hotkey }
            }
        }
    }

    /// Where the item's hotkey is set, matching `assignedHotkeys`.
    func hotkeySlot(for item: LauncherItem) -> HotkeySlot {
        if let action = item.windowAction { return .window(action) }
        if let bundleID = item.id.stripping(prefix: "app:"),
           let index = config.appHotkeys.firstIndex(where: { $0.bundleId == bundleID }) {
            return .app(index)
        }
        if let actionID = item.id.stripping(prefix: "system:"),
           let index = config.actionHotkeys.firstIndex(where: { $0.action == actionID }) {
            return .action(index)
        }
        switch item.id {
        case "mode:clipboard": return .clipboard
        case "mode:files": return .files
        case "color:pick": return .colorPicker
        case "keep-awake:toggle": return .keepAwake(.toggle)
        case "keep-awake:on": return .keepAwake(.on)
        case "keep-awake:off": return .keepAwake(.off)
        default: return .item(item.id)
        }
    }

    /// App hotkeys for apps that aren't installed (any more), so they can still be cleared.
    var missingAppItems: [LauncherItem] {
        let ids = Set(items.map(\.id))
        return config.appHotkeys
            .filter { !ids.contains("app:\($0.bundleId)") }
            .map { LauncherItem(id: "app:\($0.bundleId)", title: $0.name, category: "Application", path: nil, symbol: nil, windowAction: nil) }
    }

    private func itemSettings(_ id: String) -> ItemSettings? {
        config.items.first { $0.id == id }
    }

    /// Changes the item's settings, dropping them once they're back to the defaults.
    private func updateItemSettings(_ id: String, _ change: (inout ItemSettings) -> Void) {
        var settings = itemSettings(id) ?? ItemSettings(id: id, enabled: true, aliases: [], hotkey: nil)
        change(&settings)
        let isDefault = settings.enabled && settings.aliases.isEmpty && settings.hotkey == nil
        if let index = config.items.firstIndex(where: { $0.id == id }) {
            if isDefault {
                config.items.remove(at: index)
            } else {
                config.items[index] = settings
            }
        } else if !isDefault {
            config.items.append(settings)
        }
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
    case action(Int)
    case window(WindowAction)
    case keepAwake(KeepAwakeHotkey)
    case colorPicker
    /// Kept in the launcher item's settings (`ItemSettings.hotkey`).
    case item(String)
}

enum KeepAwakeHotkey {
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
    private let onKeepAwakeChanged: @MainActor @Sendable (Bool) -> Void
    private let onPickColor: @MainActor @Sendable () -> Void

    init(
        onTogglePalette: @escaping @MainActor @Sendable () -> Void,
        onAppsChanged: @escaping @MainActor @Sendable () -> Void,
        onCommandFinished: @escaping @MainActor @Sendable (_ name: String, _ output: String, _ success: Bool) -> Void,
        onShowMode: @escaping @MainActor @Sendable (PaletteMode) -> Void,
        onClipboardChanged: @escaping @MainActor @Sendable () -> Void,
        onKeepAwakeChanged: @escaping @MainActor @Sendable (Bool) -> Void,
        onPickColor: @escaping @MainActor @Sendable () -> Void
    ) {
        self.onTogglePalette = onTogglePalette
        self.onAppsChanged = onAppsChanged
        self.onCommandFinished = onCommandFinished
        self.onShowMode = onShowMode
        self.onClipboardChanged = onClipboardChanged
        self.onKeepAwakeChanged = onKeepAwakeChanged
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

    func keepAwakeChanged(active: Bool) {
        DispatchQueue.main.async { [onKeepAwakeChanged] in
            MainActor.assumeIsolated { onKeepAwakeChanged(active) }
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
