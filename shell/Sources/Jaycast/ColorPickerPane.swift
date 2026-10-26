import AppKit
import Observation
import SwiftUI

/// Picks colours with the system's magnifier and shows the core's history of them.
@MainActor
@Observable
final class ColorPickerState {
    private(set) var history: [PickedColor] = []
    private(set) var error: String?
    /// What was just copied, shown briefly as confirmation.
    private(set) var copied: String?
    @ObservationIgnored private let core: Core
    /// Held while the magnifier is showing; also stops a second hotkey press starting another.
    @ObservationIgnored private var sampler: NSColorSampler?
    @ObservationIgnored private var clearCopied: Task<Void, Never>?

    init(core: Core) {
        self.core = core
        refresh()
    }

    /// Shows the magnifier; a click records the colour and copies it, Escape cancels.
    func pick() {
        guard sampler == nil else { return }
        let sampler = NSColorSampler()
        self.sampler = sampler
        sampler.show { [weak self] color in
            MainActor.assumeIsolated {
                guard let self else { return }
                self.sampler = nil
                guard let color = color?.usingColorSpace(.sRGB) else { return }
                func byte(_ component: CGFloat) -> UInt8 { UInt8((min(max(component, 0), 1) * 255).rounded()) }
                self.perform {
                    let text = try self.core.recordPickedColor(
                        red: byte(color.redComponent),
                        green: byte(color.greenComponent),
                        blue: byte(color.blueComponent)
                    )
                    self.showCopied(text)
                }
                self.refresh()
            }
        }
    }

    func copy(_ color: PickedColor, as format: ColorFormat) {
        perform {
            try core.copyColor(color: color, format: format)
            showCopied(formatColor(color: color, format: format))
        }
    }

    func delete(_ color: PickedColor) {
        perform { try core.deletePickedColor(color: color) }
        refresh()
    }

    func clear() {
        perform { try core.clearColorHistory() }
        refresh()
    }

    private func refresh() {
        history = core.colorHistory()
    }

    private func perform(_ action: () throws -> Void) {
        do {
            try action()
            error = nil
        } catch {
            self.error = error.userMessage
        }
    }

    private func showCopied(_ text: String) {
        copied = text
        clearCopied?.cancel()
        clearCopied = Task { [weak self] in
            try? await Task.sleep(for: .seconds(2))
            if !Task.isCancelled { self?.copied = nil }
        }
    }
}

extension ColorFormat: CaseIterable {
    public static var allCases: [ColorFormat] { [.hex, .rgb, .hsl] }

    var title: String {
        switch self {
        case .hex: "Hex"
        case .rgb: "RGB"
        case .hsl: "HSL"
        }
    }
}

extension PickedColor {
    var color: Color {
        Color(.sRGB, red: Double(red) / 255, green: Double(green) / 255, blue: Double(blue) / 255)
    }
}

/// A rounded square of a colour, outlined so white and black still show on any background.
struct ColorSwatch: View {
    let color: PickedColor
    var size: CGFloat = 20

    var body: some View {
        RoundedRectangle(cornerRadius: size / 4)
            .fill(color.color)
            .overlay(RoundedRectangle(cornerRadius: size / 4).strokeBorder(.separator))
            .frame(width: size, height: size)
    }
}

struct ColorPickerPane: View {
    let picker: ColorPickerState
    let settings: LauncherSettings

    @State private var isConfirmingClear = false

    var body: some View {
        @Bindable var settings = settings
        let format = settings.config.colorPicker.format

        Form {
            if let error = picker.error ?? settings.saveError {
                Section {
                    Label(error, systemImage: "exclamationmark.triangle.fill")
                        .foregroundStyle(.red)
                }
            }

            Section {
                LabeledContent("Pick colour") {
                    HStack(spacing: 6) {
                        if let hotkey = settings.config.colorPicker.hotkey,
                           let problem = settings.problem(with: hotkey, in: .colorPicker) {
                            Image(systemName: "exclamationmark.triangle.fill")
                                .foregroundStyle(.yellow)
                                .help(problem)
                        }
                        HotkeyField(hotkey: $settings.config.colorPicker.hotkey)
                    }
                }
                Picker("Copy as", selection: $settings.config.colorPicker.format) {
                    ForEach(ColorFormat.allCases, id: \.self) { format in
                        Text("\(format.title) — \(formatColor(color: Self.example, format: format))").tag(format)
                    }
                }
                Button("Pick Colour", systemImage: "eyedropper") { picker.pick() }
            } footer: {
                Text("Click anywhere on screen to copy the colour under the magnifier. Press Escape to cancel.")
            }

            Section {
                if picker.history.isEmpty {
                    Text("Colours you pick appear here.")
                        .foregroundStyle(.secondary)
                } else {
                    ForEach(picker.history, id: \.self) { color in
                        HStack(spacing: 10) {
                            ColorSwatch(color: color, size: 22)
                            Text(formatColor(color: color, format: format))
                                .monospaced()
                                .textSelection(.enabled)
                            Spacer()
                            if picker.copied == formatColor(color: color, format: format) {
                                Text("Copied")
                                    .font(.caption)
                                    .foregroundStyle(.secondary)
                            }
                            Menu {
                                ForEach(ColorFormat.allCases, id: \.self) { other in
                                    Button("Copy \(formatColor(color: color, format: other))") {
                                        picker.copy(color, as: other)
                                    }
                                }
                                Divider()
                                Button("Delete", role: .destructive) { picker.delete(color) }
                            } label: {
                                Image(systemName: "ellipsis.circle")
                            } primaryAction: {
                                picker.copy(color, as: format)
                            }
                            .menuStyle(.borderlessButton)
                            .menuIndicator(.hidden)
                            .fixedSize()
                            .help("Copy \(formatColor(color: color, format: format))")
                        }
                    }
                }
            } header: {
                HStack {
                    Text("History")
                    Spacer()
                    if !picker.history.isEmpty {
                        Button("Clear History…", role: .destructive) { isConfirmingClear = true }
                            .controlSize(.small)
                    }
                }
            }
        }
        .formStyle(.grouped)
        .confirmationDialog("Clear colour history?", isPresented: $isConfirmingClear) {
            Button("Clear History", role: .destructive) { picker.clear() }
        } message: {
            Text("Every picked colour is deleted. This can't be undone.")
        }
    }

    private static let example = PickedColor(red: 26, green: 43, blue: 60, pickedAt: 0)
}
