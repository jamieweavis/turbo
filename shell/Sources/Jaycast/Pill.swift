import AppKit
import Observation
import SwiftUI

/// A small Liquid Glass capsule near the top of the screen under the pointer, confirming an
/// action that has no window of its own (a caffeinate hotkey, a picked colour). One panel,
/// reused: showing again swaps the content and restarts the timer.
@MainActor
final class Pill {
    enum Content: Equatable {
        case caffeinate(Bool)
        case color(PickedColor, text: String)
    }

    private let panel: NSPanel
    private let state = PillState()
    private let hosting: NSHostingView<PillView>
    private var hideTask: Task<Void, Never>?

    /// Gap from the bottom of the menu bar.
    private static let margin: CGFloat = 12
    private static let visibleFor = Duration.milliseconds(1400)

    init() {
        panel = NSPanel(
            contentRect: .zero,
            styleMask: [.borderless, .nonactivatingPanel],
            backing: .buffered,
            defer: true
        )
        panel.level = .statusBar
        panel.collectionBehavior = [.canJoinAllSpaces, .stationary, .ignoresCycle, .fullScreenAuxiliary]
        panel.isOpaque = false
        panel.backgroundColor = .clear
        panel.hasShadow = false
        panel.ignoresMouseEvents = true

        hosting = NSHostingView(rootView: PillView(state: state))
        panel.contentView = hosting
    }

    func show(_ content: Content) {
        state.content = content
        hosting.layoutSubtreeIfNeeded()
        let size = hosting.fittingSize

        let mouse = NSEvent.mouseLocation
        let screen = NSScreen.screens.first { $0.frame.contains(mouse) } ?? NSScreen.main
        if let visible = screen?.visibleFrame {
            panel.setFrame(NSRect(
                x: (visible.midX - size.width / 2).rounded(),
                y: visible.maxY - size.height - Self.margin,
                width: size.width,
                height: size.height
            ), display: true)
        }

        if !panel.isVisible || panel.alphaValue < 1 {
            state.appearance += 1
            panel.alphaValue = 1
            panel.orderFrontRegardless()
        }
        scheduleHide()
    }

    private func scheduleHide() {
        hideTask?.cancel()
        hideTask = Task { [weak self] in
            try? await Task.sleep(for: Self.visibleFor)
            guard !Task.isCancelled else { return }
            self?.fadeOut()
        }
    }

    private func fadeOut() {
        NSAnimationContext.runAnimationGroup { context in
            context.duration = 0.25
            panel.animator().alphaValue = 0
        } completionHandler: { [weak self] in
            MainActor.assumeIsolated {
                guard let self, self.panel.alphaValue == 0 else { return }
                self.panel.orderOut(nil)
            }
        }
    }
}

@MainActor
@Observable
private final class PillState {
    var content: Pill.Content?
    /// Bumped each time the panel appears, to replay the entrance animation.
    var appearance = 0
}

private struct PillView: View {
    let state: PillState

    @State private var scale = 1.0

    var body: some View {
        HStack(spacing: 8) {
            switch state.content {
            case .caffeinate(let active):
                Image(systemName: active ? "cup.and.saucer.fill" : "cup.and.saucer")
                    .font(.system(size: 12, weight: .semibold))
                Text(active ? "Caffeinate on" : "Caffeinate off")
            case .color(let color, let text):
                Circle()
                    .fill(color.color)
                    .overlay(Circle().strokeBorder(.white.opacity(0.4)))
                    .frame(width: 14, height: 14)
                Text(text)
                    .monospaced()
            case nil:
                EmptyView()
            }
        }
        .font(.system(size: 13, weight: .medium))
        .foregroundStyle(.white)
        .lineLimit(1)
        .fixedSize()
        .padding(.horizontal, 14)
        .padding(.vertical, 8)
        .glassEffect(.regular, in: .capsule)
        .scaleEffect(scale, anchor: .top)
        .onChange(of: state.appearance, initial: true) {
            scale = 0.9
            withAnimation(.spring(duration: 0.3, bounce: 0.25)) { scale = 1 }
        }
    }
}
