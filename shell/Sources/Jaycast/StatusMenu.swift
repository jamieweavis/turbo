import AppKit
import SwiftUI

/// The menu bar popover: display brightness on top, then the app's toggles and actions.
struct StatusMenu: View {
    let model: AppModel
    let onShowOnboarding: () -> Void
    let onShowSettings: () -> Void

    @Environment(\.dismiss) private var dismiss
    private let core = coreInfo()

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            if model.displays.builtinBrightness != nil
                || (model.displays.config.enabled && !model.displays.displays.isEmpty)
            {
                displays
                Divider()
            }

            mouse
            Divider()
            keyboard

            if !model.accessibility.isGranted {
                MenuRow(title: "Grant Accessibility…", systemImage: "hand.raised") {
                    dismiss()
                    onShowOnboarding()
                }
            }

            if let error = model.inputError {
                Label("Input unavailable: \(error)", systemImage: "exclamationmark.triangle.fill")
                    .font(.caption)
                    .foregroundStyle(.red)
            }

            Divider()

            HStack {
                Button("Settings…") {
                    dismiss()
                    onShowSettings()
                }
                .keyboardShortcut(",")

                Spacer()

                Text("Jaycast \(core.version)")
                    .font(.caption)
                    .foregroundStyle(.tertiary)

                Spacer()

                Button("Quit") { NSApp.terminate(nil) }
                    .keyboardShortcut("q")
            }
            .controlSize(.small)
        }
        .padding(14)
        .frame(width: 300)
        .background(WindowResizeRedraw())
        .task {
            // Follow changes macOS makes to the built-in display while the menu is open.
            while !Task.isCancelled {
                model.displays.refreshBuiltin()
                try? await Task.sleep(for: .seconds(1))
            }
        }
    }

    private var mouse: some View {
        @Bindable var mouse = model.mouse

        return VStack(alignment: .leading, spacing: 10) {
            Text("Mouse")
                .font(.subheadline.weight(.semibold))
                .foregroundStyle(.secondary)

            MenuToggle(title: "Smooth scrolling", systemImage: "arrow.up.and.down", isOn: $mouse.config.smoothScroll.enabled)
            MenuToggle(title: "Reverse direction", systemImage: "arrow.up.arrow.down", isOn: $mouse.config.scrollModifiers.reverseDirection)
        }
    }

    private var keyboard: some View {
        @Bindable var hyperKey = model.hyperKey

        return VStack(alignment: .leading, spacing: 10) {
            Text("Keyboard")
                .font(.subheadline.weight(.semibold))
                .foregroundStyle(.secondary)

            VStack(alignment: .leading, spacing: 4) {
                MenuToggle(title: "Hyper key", systemImage: "diamond.fill", isOn: $hyperKey.config.enabled)

                Text(hyperKey.config.key.title)
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }
        }
    }

    private var displays: some View {
        VStack(alignment: .leading, spacing: 10) {
            Text("Displays")
                .font(.subheadline.weight(.semibold))
                .foregroundStyle(.secondary)

            if let brightness = model.displays.builtinBrightness {
                BrightnessSliderRow(
                    name: "Built-in Display",
                    brightness: brightness,
                    onChange: { model.displays.setBuiltinBrightness($0) }
                )
            }

            if model.displays.config.enabled {
                ForEach(model.displays.displays, id: \.id) { display in
                    DisplayBrightnessRow(display: display, settings: model.displays)
                }
            }

            if let error = model.displays.brightnessError {
                Text(error)
                    .font(.caption)
                    .foregroundStyle(.red)
            }
        }
    }
}

/// Redraws the menu bar window's frame when its content changes height.
///
/// A `.window` MenuBarExtra resizes when its content does (a display row appearing, an error
/// line), but macOS leaves the old shadow and rounded border behind, so the corners render
/// doubled. Invalidating the shadow after each resize makes the window redraw its frame.
private struct WindowResizeRedraw: NSViewRepresentable {
    func makeNSView(context: Context) -> NSView { ResizeWatcher() }
    func updateNSView(_ nsView: NSView, context: Context) {}

    private final class ResizeWatcher: NSView {
        override func viewWillMove(toWindow newWindow: NSWindow?) {
            super.viewWillMove(toWindow: newWindow)
            let center = NotificationCenter.default
            if let window { center.removeObserver(self, name: NSWindow.didResizeNotification, object: window) }
            if let newWindow {
                center.addObserver(
                    self, selector: #selector(windowDidResize), name: NSWindow.didResizeNotification, object: newWindow
                )
            }
        }

        @objc private func windowDidResize() {
            window?.invalidateShadow()
            window?.contentView?.needsDisplay = true
        }
    }
}

/// An external display's brightness slider, or a note when it can't be controlled.
/// Shared by the menu bar popover and the Monitor settings pane.
struct DisplayBrightnessRow: View {
    let display: DisplayInfo
    let settings: DisplaySettings

    var body: some View {
        if display.supportsDdc {
            BrightnessSliderRow(
                name: display.name,
                brightness: display.brightness,
                onChange: { settings.setBrightness($0, for: display.id) }
            )
        } else {
            VStack(alignment: .leading, spacing: 4) {
                Text(display.name)
                    .font(.callout)
                Text("Brightness control not supported")
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }
        }
    }
}

/// A display's name above a brightness slider.
struct BrightnessSliderRow: View {
    let name: String
    let brightness: Double
    let onChange: (Double) -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            Text(name)
                .font(.callout)

            HStack(spacing: 8) {
                Image(systemName: "sun.min")
                    .foregroundStyle(.secondary)
                Slider(value: Binding(get: { brightness }, set: onChange), in: 0...1)
                    .labelsHidden()
                    .frame(maxWidth: .infinity)
                    .accessibilityLabel("\(name) brightness")
                Image(systemName: "sun.max")
                    .foregroundStyle(.secondary)
            }
        }
    }
}

/// A full-width switch row, matching `MenuRow`'s label layout.
private struct MenuToggle: View {
    let title: String
    let systemImage: String
    @Binding var isOn: Bool

    var body: some View {
        Toggle(isOn: $isOn) {
            Label(title, systemImage: systemImage)
                .frame(maxWidth: .infinity, alignment: .leading)
        }
        .toggleStyle(.switch)
    }
}

/// A full-width, borderless button row in the style of Control Center's menu items.
private struct MenuRow: View {
    let title: String
    let systemImage: String
    /// Shown trailing in secondary text, e.g. the shortcut.
    var detail: String? = nil
    let action: () -> Void

    var body: some View {
        Button(action: action) {
            HStack {
                Label(title, systemImage: systemImage)
                Spacer()
                if let detail {
                    Text(detail).foregroundStyle(.secondary)
                }
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            .contentShape(.rect)
        }
        .buttonStyle(.plain)
    }
}
