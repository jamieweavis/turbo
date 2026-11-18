import SwiftUI

struct MousePane: View {
    @Bindable var settings: MouseSettings

    /// Index of the binding whose editor popover is open.
    @State private var editing: Int?

    var body: some View {
        Form {
            PaneHeaderSection(pane: .mouse)

            if let error = settings.saveError {
                Section {
                    Label(error, systemImage: "exclamationmark.triangle.fill")
                        .foregroundStyle(.red)
                }
            }

            smoothScrolling
            scrollModifiers
            buttons
        }
        .formStyle(.grouped)
    }

    private var smoothScrolling: some View {
        Section {
            Toggle("Smooth scrolling", isOn: $settings.config.smoothScroll.enabled)

            Group {
                Picker("Smoothness", selection: $settings.config.smoothScroll.smoothness) {
                    ForEach(Smoothness.allCases) { Text($0.title).tag($0) }
                }

                Toggle(isOn: $settings.config.smoothScroll.trackpadSimulationVertical) {
                    Text("Vertical trackpad simulation")
                    Text("Scrolls up and down like a trackpad: content bounces at the top and bottom like it does in macOS.")
                }

                Toggle(isOn: $settings.config.smoothScroll.trackpadSimulationHorizontal) {
                    Text("Horizontal trackpad simulation")
                    Text("Scrolls sideways like a trackpad: content bounces at the edges, and swipe gestures work, such as going back in Safari.")
                }

                LabeledContent("Speed") {
                    HStack {
                        Slider(value: speed, in: 0.25...3.0)
                        Text(settings.config.smoothScroll.speed, format: .number.precision(.fractionLength(2)))
                            .monospacedDigit()
                            .frame(width: 36, alignment: .trailing)
                    }
                }

                Toggle(isOn: $settings.config.smoothScroll.acceleration) {
                    Text("Acceleration")
                    Text("Fast turns of the wheel scroll progressively further.")
                }
            }
            .disabled(!settings.config.smoothScroll.enabled)
        } header: {
            Text("Smooth Scrolling")
        } footer: {
            Text("Only mice are affected; the trackpad keeps working as usual.")
        }
    }

    /// Snaps to 0.05 steps; a `step:` slider would draw a tick mark for every one of them.
    private var speed: Binding<Double> {
        Binding(
            get: { settings.config.smoothScroll.speed },
            set: { settings.config.smoothScroll.speed = ($0 * 20).rounded() / 20 }
        )
    }

    private var scrollModifiers: some View {
        Section {
            Toggle(isOn: $settings.config.scrollModifiers.reverseDirection) {
                Text("Reverse wheel direction")
                Text("Doesn't change the trackpad's natural scrolling.")
            }

            ForEach(ScrollFunction.allCases) { function in
                modifierPicker(for: function)
            }
        } header: {
            Text("Scroll Modifiers")
        } footer: {
            Text("Hold a modifier key while scrolling to switch modes.")
        }
    }

    private func modifierPicker(for function: ScrollFunction) -> some View {
        let selection = Binding(
            get: { settings.config.scrollModifiers[keyPath: function.keyPath] },
            set: { settings.setModifier($0, for: function) }
        )
        return Picker(function.title, selection: selection) {
            Text("None").tag(Modifier?.none)
            ForEach(Modifier.allCases) { modifier in
                let owner = settings.function(using: modifier).flatMap { $0 == function ? nil : $0 }
                Text(owner.map { "\(modifier.symbol) \(modifier.title) (used for \($0.title.lowercased()))" }
                    ?? "\(modifier.symbol) \(modifier.title)")
                    .tag(Modifier?.some(modifier))
                    .disabled(owner != nil)
            }
        }
    }

    private var buttons: some View {
        Section("Buttons") {
            Toggle("Button bindings", isOn: $settings.config.buttonsEnabled)

            Group {
                if settings.config.buttons.isEmpty {
                    Text("No button bindings")
                        .foregroundStyle(.secondary)
                }

                ForEach(settings.config.buttons.indices, id: \.self) { index in
                    ButtonBindingRow(
                        binding: binding(at: index),
                        isDuplicate: isDuplicate(at: index),
                        isEditing: Binding(
                            get: { editing == index },
                            set: { editing = $0 ? index : nil }
                        ),
                        onRemove: { remove(at: index) }
                    )
                }

                Button("Add Binding", systemImage: "plus", action: addBinding)
            }
            .disabled(!settings.config.buttonsEnabled)
        }
    }

    /// Bounds-checked, because SwiftUI can still read a row's binding while it is being removed.
    private func binding(at index: Int) -> Binding<ButtonBinding> {
        Binding(
            get: {
                settings.config.buttons.indices.contains(index)
                    ? settings.config.buttons[index]
                    : ButtonBinding(button: 2, trigger: .click, action: .middleClick)
            },
            set: { newValue in
                if settings.config.buttons.indices.contains(index) {
                    settings.config.buttons[index] = newValue
                }
            }
        )
    }

    private func isDuplicate(at index: Int) -> Bool {
        let buttons = settings.config.buttons
        guard buttons.indices.contains(index) else { return false }
        let target = buttons[index]
        return buttons.indices.contains { $0 != index && buttons[$0].button == target.button && buttons[$0].trigger == target.trigger }
    }

    private func remove(at index: Int) {
        editing = nil
        if settings.config.buttons.indices.contains(index) {
            settings.config.buttons.remove(at: index)
        }
    }

    /// Adds the first button/trigger pair that isn't bound yet, and opens its editor.
    private func addBinding() {
        let used = Set(settings.config.buttons.map { "\($0.button)-\($0.trigger)" })
        let pairs = MouseButtons.choices.flatMap { button in Trigger.allCases.map { (button, $0) } }
        let (button, trigger) = pairs.first { !used.contains("\($0.0)-\($0.1)") } ?? (2, .click)
        settings.config.buttons.append(ButtonBinding(button: button, trigger: trigger, action: .missionControl))
        editing = settings.config.buttons.count - 1
    }
}

private struct ButtonBindingRow: View {
    @Binding var binding: ButtonBinding
    let isDuplicate: Bool
    @Binding var isEditing: Bool
    let onRemove: () -> Void

    var body: some View {
        HStack {
            VStack(alignment: .leading) {
                Text(MouseButtons.name(binding.button))
                Text(binding.trigger.title)
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }

            if isDuplicate {
                Image(systemName: "exclamationmark.triangle.fill")
                    .foregroundStyle(.yellow)
                    .help("Another binding uses the same button and trigger")
            }

            Spacer()

            Text(binding.action.title)
                .foregroundStyle(.secondary)

            Button("Edit", systemImage: "pencil") { isEditing = true }
                .labelStyle(.iconOnly)
                .buttonStyle(.borderless)
                .popover(isPresented: $isEditing, arrowEdge: .trailing) {
                    ButtonBindingEditor(binding: $binding)
                }

            Button("Remove", systemImage: "minus.circle", action: onRemove)
                .labelStyle(.iconOnly)
                .buttonStyle(.borderless)
        }
    }
}

private struct ButtonBindingEditor: View {
    @Binding var binding: ButtonBinding

    /// "Keyboard Shortcut" was picked but nothing recorded yet; the old action stays until then.
    @State private var awaitingShortcut = false

    var body: some View {
        Form {
            Picker("Button", selection: $binding.button) {
                ForEach(MouseButtons.choices, id: \.self) { Text(MouseButtons.name($0)).tag($0) }
            }

            Picker("Trigger", selection: $binding.trigger) {
                ForEach(Trigger.allCases) { Text($0.title).tag($0) }
            }

            Picker("Action", selection: actionKind) {
                ForEach(ActionKind.allCases) { Text($0.title).tag($0) }
            }

            if actionKind.wrappedValue == .keyboardShortcut {
                LabeledContent("Shortcut") {
                    ShortcutRecorder(shortcut: shortcut, recordsOnAppear: awaitingShortcut)
                }
            }
        }
        .formStyle(.grouped)
        .frame(width: 360)
        .fixedSize(horizontal: false, vertical: true)
    }

    private var actionKind: Binding<ActionKind> {
        Binding(
            get: { awaitingShortcut ? .keyboardShortcut : ActionKind(binding.action) },
            set: { kind in
                if let action = kind.action {
                    awaitingShortcut = false
                    binding.action = action
                } else if ActionKind(binding.action) != .keyboardShortcut {
                    awaitingShortcut = true
                }
            }
        )
    }

    private var shortcut: Binding<Shortcut?> {
        Binding(
            get: {
                guard case let .keyboardShortcut(keyCode, modifiers) = binding.action else { return nil }
                return Shortcut(keyCode: keyCode, modifiers: modifiers)
            },
            set: { shortcut in
                guard let shortcut else { return }
                awaitingShortcut = false
                binding.action = .keyboardShortcut(keyCode: shortcut.keyCode, modifiers: shortcut.modifiers)
            }
        )
    }
}
