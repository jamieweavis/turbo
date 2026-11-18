import AppKit

/// Backpack is a menu-bar-only (accessory) app, which macOS won't activate. While one of its
/// ordinary windows (settings, onboarding) is open it becomes a regular app, so the window
/// can take focus and gets a ⌘⇥ entry and the menu bar. It goes back once they're all closed.
@MainActor
enum RegularAppWindows {
    private static var observers: [ObjectIdentifier: any NSObjectProtocol] = [:]

    static func show(_ window: NSWindow) {
        let id = ObjectIdentifier(window)
        if observers[id] == nil {
            observers[id] = NotificationCenter.default.addObserver(
                forName: NSWindow.willCloseNotification, object: window, queue: .main
            ) { _ in
                MainActor.assumeIsolated { didClose(id) }
            }
        }
        if window.isMiniaturized { window.deminiaturize(nil) }
        NSApp.setActivationPolicy(.regular)
        // Plain `activate()` is only a request, and it's refused when another app is
        // frontmost, as it is behind the non-activating palette when ⌘, opens settings.
        NSApp.activate(ignoringOtherApps: true)
        window.makeKeyAndOrderFront(nil)
    }

    private static func didClose(_ id: ObjectIdentifier) {
        if let observer = observers.removeValue(forKey: id) {
            NotificationCenter.default.removeObserver(observer)
        }
        if observers.isEmpty {
            NSApp.setActivationPolicy(.accessory)
        }
    }
}
