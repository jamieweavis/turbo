import AppKit
import Carbon.HIToolbox

// Display names and pickable cases for the generated core types.

extension Smoothness: CaseIterable, Identifiable {
    public static let allCases: [Smoothness] = [.low, .regular, .high]
    public var id: Self { self }

    var title: String {
        switch self {
        case .low: "Low"
        case .regular: "Regular"
        case .high: "High"
        }
    }
}

extension Trigger: CaseIterable, Identifiable {
    public static let allCases: [Trigger] = [.click, .doubleClick, .hold]
    public var id: Self { self }

    var title: String {
        switch self {
        case .click: "Click"
        case .doubleClick: "Double Click"
        case .hold: "Hold"
        }
    }
}

extension Modifier: CaseIterable, Identifiable {
    /// In the order macOS displays modifiers: ⌃⌥⇧⌘.
    public static let allCases: [Modifier] = [.control, .option, .shift, .command]
    public var id: Self { self }

    var symbol: String {
        switch self {
        case .shift: "⇧"
        case .control: "⌃"
        case .option: "⌥"
        case .command: "⌘"
        }
    }

    var title: String {
        switch self {
        case .shift: "Shift"
        case .control: "Control"
        case .option: "Option"
        case .command: "Command"
        }
    }

    static func from(_ flags: NSEvent.ModifierFlags) -> [Modifier] {
        allCases.filter { modifier in
            switch modifier {
            case .shift: flags.contains(.shift)
            case .control: flags.contains(.control)
            case .option: flags.contains(.option)
            case .command: flags.contains(.command)
            }
        }
    }
}

/// The scroll functions a held modifier can switch on.
enum ScrollFunction: CaseIterable, Identifiable {
    case horizontal, zoom, precise

    var id: Self { self }

    var title: String {
        switch self {
        case .horizontal: "Scroll horizontally"
        case .zoom: "Zoom"
        case .precise: "Precise scrolling"
        }
    }

    var keyPath: WritableKeyPath<ScrollModifierConfig, Modifier?> {
        switch self {
        case .horizontal: \.horizontal
        case .zoom: \.zoom
        case .precise: \.precise
        }
    }
}

/// CoreGraphics button numbers: 0 left, 1 right, 2 middle, 3 back, 4 forward, 5+ extra.
enum MouseButtons {
    static let choices: [UInt32] = Array(2...9)

    static func name(_ button: UInt32) -> String {
        switch button {
        case 2: "Middle button"
        case 3: "Back button"
        case 4: "Forward button"
        default: "Button \(button + 1)"
        }
    }
}

/// `Action` without its payload, for pickers.
enum ActionKind: CaseIterable, Identifiable {
    case back, forward, missionControl, appExpose, showDesktop, launchpad
    case previousSpace, nextSpace, middleClick, keyboardShortcut

    var id: Self { self }

    init(_ action: Action) {
        self = switch action {
        case .back: .back
        case .forward: .forward
        case .missionControl: .missionControl
        case .appExpose: .appExpose
        case .showDesktop: .showDesktop
        case .launchpad: .launchpad
        case .previousSpace: .previousSpace
        case .nextSpace: .nextSpace
        case .middleClick: .middleClick
        case .keyboardShortcut: .keyboardShortcut
        }
    }

    /// The action for every kind except `keyboardShortcut`, which needs a recorded shortcut.
    var action: Action? {
        switch self {
        case .back: .back
        case .forward: .forward
        case .missionControl: .missionControl
        case .appExpose: .appExpose
        case .showDesktop: .showDesktop
        case .launchpad: .launchpad
        case .previousSpace: .previousSpace
        case .nextSpace: .nextSpace
        case .middleClick: .middleClick
        case .keyboardShortcut: nil
        }
    }

    var title: String {
        switch self {
        case .back: "Back"
        case .forward: "Forward"
        case .missionControl: "Mission Control"
        case .appExpose: "App Exposé"
        case .showDesktop: "Show Desktop"
        case .launchpad: "Launchpad"
        case .previousSpace: "Previous Space"
        case .nextSpace: "Next Space"
        case .middleClick: "Middle Click"
        case .keyboardShortcut: "Keyboard Shortcut"
        }
    }
}

extension Action {
    @MainActor
    var title: String {
        if case let .keyboardShortcut(keyCode, modifiers) = self {
            return Shortcut(keyCode: keyCode, modifiers: modifiers).title
        }
        return ActionKind(self).title
    }
}

struct Shortcut: Equatable {
    var keyCode: UInt16
    var modifiers: [Modifier]

    @MainActor
    var title: String {
        Modifier.allCases.filter(modifiers.contains).map(\.symbol).joined() + KeyNames.name(for: keyCode)
    }
}

/// Names keys the way macOS menus do, using the current keyboard layout for printable keys.
@MainActor
enum KeyNames {
    static func name(for keyCode: UInt16) -> String {
        special[Int(keyCode)] ?? translated(keyCode) ?? "Key \(keyCode)"
    }

    private static let special: [Int: String] = {
        var names = [
            kVK_Return: "↩", kVK_Tab: "⇥", kVK_Space: "Space", kVK_Delete: "⌫", kVK_ForwardDelete: "⌦",
            kVK_Escape: "⎋", kVK_LeftArrow: "←", kVK_RightArrow: "→", kVK_UpArrow: "↑", kVK_DownArrow: "↓",
            kVK_Home: "↖", kVK_End: "↘", kVK_PageUp: "⇞", kVK_PageDown: "⇟", kVK_ANSI_KeypadEnter: "⌤",
        ]
        let functionKeys = [
            kVK_F1, kVK_F2, kVK_F3, kVK_F4, kVK_F5, kVK_F6, kVK_F7, kVK_F8, kVK_F9, kVK_F10,
            kVK_F11, kVK_F12, kVK_F13, kVK_F14, kVK_F15, kVK_F16, kVK_F17, kVK_F18, kVK_F19, kVK_F20,
        ]
        for (index, keyCode) in functionKeys.enumerated() {
            names[keyCode] = "F\(index + 1)"
        }
        return names
    }()

    private static func translated(_ keyCode: UInt16) -> String? {
        guard let source = TISCopyCurrentKeyboardLayoutInputSource()?.takeRetainedValue(),
              let layoutData = TISGetInputSourceProperty(source, kTISPropertyUnicodeKeyLayoutData)
        else { return nil }
        let data = Unmanaged<CFData>.fromOpaque(layoutData).takeUnretainedValue() as Data

        var deadKeyState: UInt32 = 0
        var characters = [UniChar](repeating: 0, count: 4)
        var length = 0
        let status = data.withUnsafeBytes { bytes in
            UCKeyTranslate(
                bytes.baseAddress!.assumingMemoryBound(to: UCKeyboardLayout.self),
                keyCode, UInt16(kUCKeyActionDisplay), 0, UInt32(LMGetKbdType()),
                OptionBits(kUCKeyTranslateNoDeadKeysBit), &deadKeyState,
                characters.count, &length, &characters
            )
        }
        guard status == noErr, length > 0 else { return nil }
        return String(utf16CodeUnits: characters, count: length).uppercased()
    }
}
