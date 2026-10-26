import AppKit
import Carbon.HIToolbox
import SwiftUI

/// Click, then type a shortcut. Escape (without modifiers) cancels.
struct ShortcutRecorder: View {
    @Binding var shortcut: Shortcut?
    /// Start listening as soon as the recorder appears, e.g. right after choosing the action.
    var recordsOnAppear = false

    @State private var monitor: Any?

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
        // Local monitors run on the main thread.
        monitor = NSEvent.addLocalMonitorForEvents(matching: .keyDown) { event in
            MainActor.assumeIsolated {
                let modifiers = Modifier.from(event.modifierFlags)
                if !(event.keyCode == UInt16(kVK_Escape) && modifiers.isEmpty) {
                    shortcut = Shortcut(keyCode: event.keyCode, modifiers: modifiers)
                }
                stopRecording()
            }
            return nil
        }
    }

    private func stopRecording() {
        if let monitor {
            NSEvent.removeMonitor(monitor)
        }
        monitor = nil
    }
}
