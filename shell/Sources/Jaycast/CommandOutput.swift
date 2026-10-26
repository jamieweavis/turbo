import AppKit
import SwiftUI

/// Floating windows showing a shell command's output. One window per command: running the
/// same command again replaces its output; different commands get their own, cascaded.
@MainActor
final class CommandOutputWindows {
    private var windows: [String: NSPanel] = [:]
    private var lastTopLeft: NSPoint?

    private static let size = NSSize(width: 520, height: 320)

    func show(name: String, output: String, success: Bool) {
        let view = CommandOutputView(name: name, output: output, success: success) { [weak self] in
            self?.windows[name]?.close()
        }
        if let panel = windows[name] {
            panel.contentViewController = NSHostingController(rootView: view)
            panel.orderFrontRegardless()
            return
        }

        let panel = NSPanel(
            contentRect: NSRect(origin: .zero, size: Self.size),
            styleMask: [.titled, .closable, .resizable, .utilityWindow],
            backing: .buffered,
            defer: true
        )
        panel.title = name
        panel.level = .floating
        panel.isReleasedWhenClosed = false
        panel.hidesOnDeactivate = false
        panel.contentMinSize = NSSize(width: 320, height: 160)
        panel.contentViewController = NSHostingController(rootView: view)
        panel.setContentSize(Self.size)
        place(panel)
        windows[name] = panel
        NotificationCenter.default.addObserver(
            forName: NSWindow.willCloseNotification,
            object: panel,
            queue: .main
        ) { [weak self] _ in
            MainActor.assumeIsolated { self?.windows[name] = nil }
        }
        panel.orderFrontRegardless()
    }

    /// The first window is centred on the screen with the mouse; later ones cascade from it.
    private func place(_ panel: NSPanel) {
        if let lastTopLeft, !windows.isEmpty {
            self.lastTopLeft = panel.cascadeTopLeft(from: lastTopLeft)
            return
        }
        let screen = NSScreen.screens.first { NSMouseInRect(NSEvent.mouseLocation, $0.frame, false) } ?? NSScreen.main
        if let visible = screen?.visibleFrame {
            panel.setFrameOrigin(NSPoint(x: visible.midX - panel.frame.width / 2, y: visible.midY - panel.frame.height / 2))
        }
        lastTopLeft = panel.cascadeTopLeft(from: NSPoint(x: panel.frame.minX, y: panel.frame.maxY))
    }
}

private struct CommandOutputView: View {
    let name: String
    let output: String
    let success: Bool
    let onClose: () -> Void

    private var text: String {
        output.isEmpty ? "No output" : output
    }

    var body: some View {
        VStack(spacing: 0) {
            HStack(spacing: 8) {
                Image(systemName: success ? "terminal.fill" : "xmark.octagon.fill")
                    .foregroundStyle(success ? Color.secondary : Color.red)
                Text(success ? "Finished" : "Failed")
                    .fontWeight(.semibold)
                    .foregroundStyle(success ? Color.primary : Color.red)
                Spacer()
            }
            .padding(.horizontal, 14)
            .padding(.vertical, 10)
            .background(success ? Color.clear : Color.red.opacity(0.12))

            Divider()

            ScrollView {
                Text(text)
                    .font(.system(.body, design: .monospaced))
                    .foregroundStyle(output.isEmpty ? .secondary : .primary)
                    .textSelection(.enabled)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .padding(14)
            }

            Divider()

            HStack {
                Spacer()
                Button("Copy") {
                    NSPasteboard.general.clearContents()
                    NSPasteboard.general.setString(output, forType: .string)
                }
                    .disabled(output.isEmpty)
                Button("Close", action: onClose)
                    .keyboardShortcut(.defaultAction)
            }
            .padding(12)
        }
    }
}
