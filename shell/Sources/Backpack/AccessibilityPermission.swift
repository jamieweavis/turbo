import AppKit
import ApplicationServices
import Observation
import os

/// Tracks whether Backpack is trusted for Accessibility, which the event tap,
/// window management and text expansion all need.
///
/// macOS posts no notification when the user flips the switch in System Settings, so this
/// polls once a second for the app's lifetime. `AXIsProcessTrusted` is a cheap check, and
/// polling also catches the grant being revoked while the event tap is running.
@MainActor
@Observable
final class AccessibilityPermission {
    private(set) var isGranted = false
    @ObservationIgnored private var pollTimer: Timer?

    func refresh() {
        // Assign only on change: @Observable notifies on every set, and observers start or
        // stop the event tap in response.
        let trusted = AXIsProcessTrusted()
        if trusted != isGranted {
            Logger.app.info("Accessibility \(trusted ? "granted" : "revoked", privacy: .public)")
            isGranted = trusted
        }
    }

    /// Shows the system prompt, which adds Backpack to the Accessibility list.
    func request() {
        let options = ["AXTrustedCheckOptionPrompt": true] as CFDictionary
        isGranted = AXIsProcessTrustedWithOptions(options)
    }

    func openSystemSettings() {
        let url = URL(string: "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility")!
        NSWorkspace.shared.open(url)
    }

    func startWatching() {
        refresh()
        guard pollTimer == nil else { return }
        pollTimer = Timer.scheduledTimer(withTimeInterval: 1, repeats: true) { [weak self] _ in
            MainActor.assumeIsolated { self?.refresh() }
        }
    }
}
