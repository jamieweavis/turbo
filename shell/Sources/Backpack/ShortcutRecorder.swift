import AppKit
import Carbon.HIToolbox
import SwiftUI

/// Click, then type a shortcut. Escape (without modifiers) cancels.
struct ShortcutRecorder: View {
    @Binding var shortcut: Shortcut?
    /// Start listening as soon as the recorder appears, e.g. right after choosing the action.
    var recordsOnAppear = false
    /// For global hotkeys: a bare key (other than F1–F20) would swallow normal typing, so
    /// it's refused with a beep and recording continues.
    var requiresModifier = false

    @State private var monitor: Any?

    /// Told when any recorder starts or stops listening, so global hotkeys can be paused:
    /// otherwise pressing an existing hotkey would fire it instead of being recorded.
    static var onRecordingChanged: (@MainActor (Bool) -> Void)?

    var body: some View {
        Button {
            if monitor == nil { startRecording() } else { stopRecording() }
        } label: {
            Text(monitor != nil ? "Type shortcut…" : shortcut?.title ?? "Record Shortcut")
                .frame(minWidth: 110)
        }
        .onAppear { if recordsOnAppear { startRecording() } }
        .onDisappear { stopRecording() }
    }

    private func startRecording() {
        guard monitor == nil else { return }
        Self.onRecordingChanged?(true)
        // Local monitors run on the main thread.
        monitor = NSEvent.addLocalMonitorForEvents(matching: .keyDown) { event in
            MainActor.assumeIsolated {
                let modifiers = Modifier.from(event.modifierFlags)
                let isEscape = event.keyCode == UInt16(kVK_Escape) && modifiers.isEmpty
                if !isEscape, requiresModifier, modifiers.isEmpty, !KeyNames.isFunctionKey(event.keyCode) {
                    NSSound.beep()
                    return
                }
                if !isEscape {
                    shortcut = Shortcut(keyCode: event.keyCode, modifiers: modifiers)
                }
                stopRecording()
            }
            return nil
        }
    }

    private func stopRecording() {
        guard let monitor else { return }
        NSEvent.removeMonitor(monitor)
        self.monitor = nil
        Self.onRecordingChanged?(false)
    }
}
