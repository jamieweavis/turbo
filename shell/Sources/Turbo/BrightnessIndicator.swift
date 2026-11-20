import AppKit
import Observation
import SwiftUI

/// The on-screen indicator shown when the brightness keys change an external display,
/// styled after macOS's own: a Liquid Glass card under the menu bar at the top right, with
/// the display's name and a slider that can be dragged to keep adjusting.
///
/// The system's indicator is drawn by Control Center, which has no API for other displays,
/// so this recreates it. One panel per display, reused across presses.
@MainActor
final class BrightnessIndicator {
    private let settings: DisplaySettings
    private var huds: [UInt32: BrightnessHUD] = [:]

    init(settings: DisplaySettings) {
        self.settings = settings
    }

    func show(on displayID: UInt32) {
        guard let screen = NSScreen.screens.first(where: { $0.displayID == displayID }) else { return }
        let hud = huds[displayID] ?? BrightnessHUD(displayID: displayID, settings: settings)
        huds[displayID] = hud
        hud.show(on: screen)
    }
}

/// A borderless, non-activating panel: it takes clicks and drags without stealing focus
/// from the frontmost app. It stays up while hovered or dragged, then fades out.
@MainActor
private final class BrightnessHUD {
    private let panel: NSPanel
    private let interaction = HUDInteraction()
    private var hideTask: Task<Void, Never>?

    private static let size = NSSize(width: 300, height: 76)
    /// Gap from the screen's right edge and from the bottom of the menu bar.
    private static let margin: CGFloat = 8
    private static let visibleFor = Duration.milliseconds(1500)

    init(displayID: UInt32, settings: DisplaySettings) {
        panel = NSPanel(
            contentRect: NSRect(origin: .zero, size: Self.size),
            styleMask: [.borderless, .nonactivatingPanel],
            backing: .buffered,
            defer: true
        )
        panel.level = .statusBar
        panel.collectionBehavior = [.canJoinAllSpaces, .stationary, .ignoresCycle, .fullScreenAuxiliary]
        panel.isOpaque = false
        panel.backgroundColor = .clear
        panel.hasShadow = false
        panel.becomesKeyOnlyIfNeeded = true

        let view = BrightnessHUDView(displayID: displayID, settings: settings, interaction: interaction)
        panel.contentView = FirstMouseHostingView(rootView: view)

        interaction.onIdle = { [weak self] in self?.scheduleHide() }
        interaction.onBusy = { [weak self] in self?.hideTask?.cancel() }
    }

    func show(on screen: NSScreen) {
        let visible = screen.visibleFrame
        panel.setFrameOrigin(NSPoint(
            x: visible.maxX - Self.size.width - Self.margin,
            y: visible.maxY - Self.size.height - Self.margin
        ))

        if !panel.isVisible || panel.alphaValue < 1 {
            interaction.appearance += 1
            panel.alphaValue = 1
            panel.orderFrontRegardless()
        }
        if !interaction.isBusy {
            scheduleHide()
        }
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

/// Hover and drag state shared between the panel (which owns the hide timer) and its view.
@MainActor
@Observable
private final class HUDInteraction {
    var isHovering = false { didSet { changed() } }
    var isDragging = false { didSet { changed() } }
    /// Bumped each time the panel appears, to replay the entrance animation.
    var appearance = 0

    @ObservationIgnored var onBusy: (() -> Void)?
    @ObservationIgnored var onIdle: (() -> Void)?

    var isBusy: Bool { isHovering || isDragging }

    private func changed() {
        isBusy ? onBusy?() : onIdle?()
    }
}

/// Lets the first click land on the slider even though the panel is never key.
private final class FirstMouseHostingView<Content: View>: NSHostingView<Content> {
    override func acceptsFirstMouse(for event: NSEvent?) -> Bool { true }
}

private struct BrightnessHUDView: View {
    let displayID: UInt32
    let settings: DisplaySettings
    let interaction: HUDInteraction

    @State private var scale = 1.0

    private var display: DisplayInfo? {
        settings.displays.first { $0.id == displayID }
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            Text(display?.name ?? "Display")
                .font(.system(size: 13, weight: .semibold))
                .lineLimit(1)

            HStack(spacing: 10) {
                Image(systemName: "sun.min.fill")
                    .font(.system(size: 11, weight: .semibold))
                BrightnessSlider(
                    value: display?.brightness ?? 0,
                    onChange: { settings.setBrightness($0, for: displayID) },
                    isDragging: Binding(get: { interaction.isDragging }, set: { interaction.isDragging = $0 })
                )
                Image(systemName: "sun.max.fill")
                    .font(.system(size: 15, weight: .semibold))
            }
        }
        .foregroundStyle(.white)
        .padding(.horizontal, 18)
        .padding(.vertical, 14)
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .leading)
        .glassEffect(.regular, in: .rect(cornerRadius: 24))
        .scaleEffect(scale, anchor: .topTrailing)
        .onHover { interaction.isHovering = $0 }
        .onChange(of: interaction.appearance, initial: true) {
            scale = 0.92
            withAnimation(.spring(duration: 0.3, bounce: 0.25)) { scale = 1 }
        }
    }
}

/// A thick rounded bar with one tick per brightness-key step underneath. Clicking jumps to
/// a point; dragging scrubs.
private struct BrightnessSlider: View {
    let value: Double
    let onChange: (Double) -> Void
    @Binding var isDragging: Bool

    private static let steps = 16
    private static let barHeight: CGFloat = 6

    var body: some View {
        GeometryReader { proxy in
            let width = proxy.size.width
            VStack(spacing: 5) {
                ZStack(alignment: .leading) {
                    Capsule().fill(.white.opacity(0.25))
                    Capsule()
                        .fill(.white)
                        .frame(width: max(Self.barHeight, width * min(max(value, 0), 1)))
                }
                .frame(height: Self.barHeight)
                .animation(isDragging ? nil : .easeOut(duration: 0.12), value: value)

                HStack(spacing: 0) {
                    ForEach(0...Self.steps, id: \.self) { index in
                        Circle()
                            .fill(.white.opacity(0.35))
                            .frame(width: 2, height: 2)
                        if index < Self.steps { Spacer(minLength: 0) }
                    }
                }
            }
            .frame(maxHeight: .infinity)
            .contentShape(.rect)
            .gesture(
                DragGesture(minimumDistance: 0)
                    .onChanged { drag in
                        isDragging = true
                        onChange(min(max(drag.location.x / width, 0), 1))
                    }
                    .onEnded { _ in isDragging = false }
            )
        }
        .frame(height: 22)
        .accessibilityElement()
        .accessibilityLabel("Brightness")
        .accessibilityValue("\(Int((value * 100).rounded())) percent")
        .accessibilityAdjustableAction { direction in
            let step = 1.0 / Double(Self.steps)
            onChange(min(max(value + (direction == .increment ? step : -step), 0), 1))
        }
    }
}

extension NSScreen {
    var displayID: UInt32? {
        (deviceDescription[NSDeviceDescriptionKey("NSScreenNumber")] as? NSNumber)?.uint32Value
    }
}
