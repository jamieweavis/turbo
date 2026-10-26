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
    func problem(with hotkey: Hotkey, ignoringAppAt index: Int? = nil) -> String? {
        if isDuplicate(hotkey, ignoringAppAt: index) {
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

    /// Whether `hotkey` is also used by the palette or another app hotkey than `index`.
    func isDuplicate(_ hotkey: Hotkey, ignoringAppAt index: Int? = nil) -> Bool {
        if index != nil, let palette = config.hotkey, palette.matches(hotkey) {
            return true
        }
        return config.appHotkeys.indices.contains { other in
            other != index && config.appHotkeys[other].hotkey.matches(hotkey)
        }
    }

    /// The app's bundle path for its icon: the index first, then Launch Services.
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

/// Receives the core's launcher callbacks (hotkeys on the main thread, app index changes and
/// command results on background threads) and forwards them, in order, to the main actor.
final class LauncherObserverBridge: LauncherObserver {
    private let onTogglePalette: @MainActor @Sendable () -> Void
    private let onAppsChanged: @MainActor @Sendable () -> Void
    private let onCommandFinished: @MainActor @Sendable (_ name: String, _ output: String, _ success: Bool) -> Void

    init(
        onTogglePalette: @escaping @MainActor @Sendable () -> Void,
        onAppsChanged: @escaping @MainActor @Sendable () -> Void,
        onCommandFinished: @escaping @MainActor @Sendable (_ name: String, _ output: String, _ success: Bool) -> Void
    ) {
        self.onTogglePalette = onTogglePalette
        self.onAppsChanged = onAppsChanged
        self.onCommandFinished = onCommandFinished
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

    // TODO(step 5): open the palette in this mode.
    func showMode(mode: PaletteMode) {}

    // TODO(step 5): refresh clipboard history.
    func clipboardChanged() {}

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

    static func icon(forPath path: String) -> NSImage {
        if let icon = cache[path] { return icon }
        // Resolve symlinks (e.g. /Applications/Safari.app) so the icon has no alias badge.
        let resolved = URL(fileURLWithPath: path).resolvingSymlinksInPath().path(percentEncoded: false)
        let icon = NSWorkspace.shared.icon(forFile: resolved)
        cache[path] = icon
        return icon
    }
}
