import AppKit
import ApplicationServices
import Observation

/// Tracks whether Jaycast is trusted for Accessibility, which the event tap,
/// window management and text expansion all need.
///
/// macOS posts no notification when the user flips the switch in System Settings,
/// so while someone is watching (the onboarding window), this polls once a second.
@MainActor
@Observable
final class AccessibilityPermission {
    private(set) var isGranted = false
    @ObservationIgnored private var pollTimer: Timer?

    func refresh() {
        isGranted = AXIsProcessTrusted()
    }

    /// Shows the system prompt, which adds Jaycast to the Accessibility list.
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

    func stopWatching() {
        pollTimer?.invalidate()
        pollTimer = nil
    }
}
