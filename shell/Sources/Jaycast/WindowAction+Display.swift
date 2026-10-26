import SwiftUI

// Display names, icons and settings groups for the core's window actions. The titles match
// the core's palette results (`core/src/windows/mod.rs`).

extension WindowAction: Identifiable {
    public var id: Self { self }

    var title: String {
        switch self {
        case .leftHalf: "Left Half"
        case .rightHalf: "Right Half"
        case .topHalf: "Top Half"
        case .bottomHalf: "Bottom Half"
        case .topLeft: "Top Left Quarter"
        case .topRight: "Top Right Quarter"
        case .bottomLeft: "Bottom Left Quarter"
        case .bottomRight: "Bottom Right Quarter"
        case .leftThird: "Left Third"
        case .centerThird: "Center Third"
        case .rightThird: "Right Third"
        case .leftTwoThirds: "Left Two Thirds"
        case .rightTwoThirds: "Right Two Thirds"
        case .topLeftSixth: "Top Left Sixth"
        case .topCenterSixth: "Top Center Sixth"
        case .topRightSixth: "Top Right Sixth"
        case .bottomLeftSixth: "Bottom Left Sixth"
        case .bottomCenterSixth: "Bottom Center Sixth"
        case .bottomRightSixth: "Bottom Right Sixth"
        case .maximize: "Maximize"
        case .almostMaximize: "Almost Maximize"
        case .center: "Center"
        case .restore: "Restore"
        case .nextDisplay: "Next Display"
        case .previousDisplay: "Previous Display"
        }
    }

    /// The part of the screen the action fills, as fractions of it (origin top-left), for
    /// `WindowActionIcon`. Nil for actions without a fixed spot.
    var region: CGRect? {
        let third = 1.0 / 3.0
        return switch self {
        case .leftHalf: CGRect(x: 0, y: 0, width: 0.5, height: 1)
        case .rightHalf: CGRect(x: 0.5, y: 0, width: 0.5, height: 1)
        case .topHalf: CGRect(x: 0, y: 0, width: 1, height: 0.5)
        case .bottomHalf: CGRect(x: 0, y: 0.5, width: 1, height: 0.5)
        case .topLeft: CGRect(x: 0, y: 0, width: 0.5, height: 0.5)
        case .topRight: CGRect(x: 0.5, y: 0, width: 0.5, height: 0.5)
        case .bottomLeft: CGRect(x: 0, y: 0.5, width: 0.5, height: 0.5)
        case .bottomRight: CGRect(x: 0.5, y: 0.5, width: 0.5, height: 0.5)
        case .leftThird: CGRect(x: 0, y: 0, width: third, height: 1)
        case .centerThird: CGRect(x: third, y: 0, width: third, height: 1)
        case .rightThird: CGRect(x: 2 * third, y: 0, width: third, height: 1)
        case .leftTwoThirds: CGRect(x: 0, y: 0, width: 2 * third, height: 1)
        case .rightTwoThirds: CGRect(x: third, y: 0, width: 2 * third, height: 1)
        case .topLeftSixth: CGRect(x: 0, y: 0, width: third, height: 0.5)
        case .topCenterSixth: CGRect(x: third, y: 0, width: third, height: 0.5)
        case .topRightSixth: CGRect(x: 2 * third, y: 0, width: third, height: 0.5)
        case .bottomLeftSixth: CGRect(x: 0, y: 0.5, width: third, height: 0.5)
        case .bottomCenterSixth: CGRect(x: third, y: 0.5, width: third, height: 0.5)
        case .bottomRightSixth: CGRect(x: 2 * third, y: 0.5, width: third, height: 0.5)
        case .maximize: CGRect(x: 0, y: 0, width: 1, height: 1)
        case .almostMaximize: CGRect(x: 0.15, y: 0.15, width: 0.7, height: 0.7)
        case .center: CGRect(x: 0.3, y: 0.25, width: 0.4, height: 0.5)
        case .restore, .nextDisplay, .previousDisplay: nil
        }
    }

    /// The SF Symbol for actions without a `region`.
    var symbol: String {
        switch self {
        case .restore: "arrow.uturn.backward"
        case .nextDisplay: "arrow.right.square"
        case .previousDisplay: "arrow.left.square"
        default: "macwindow"
        }
    }
}

/// A screen outline with the part an action fills shaded in. SF Symbols has nothing for
/// two-thirds or a 2×3 grid, so these are drawn. Takes the foreground style, like a symbol.
struct WindowActionIcon: View {
    let action: WindowAction
    /// The screen outline's width; its height is three quarters of that.
    var width: CGFloat = 20

    var body: some View {
        if let region = action.region {
            Canvas { context, size in
                let screen = CGRect(origin: .zero, size: size).insetBy(dx: 0.6, dy: 0.6)
                context.stroke(Path(roundedRect: screen, cornerRadius: 3), with: .foreground, lineWidth: 1.2)
                // The rest of the screen, faintly, so small cells still read as a position.
                let inside = screen.insetBy(dx: 1.9, dy: 1.9)
                var faint = context
                faint.opacity = 0.2
                faint.fill(Path(roundedRect: inside, cornerRadius: 1.5), with: .foreground)
                let cell = CGRect(
                    x: inside.minX + region.minX * inside.width,
                    y: inside.minY + region.minY * inside.height,
                    width: region.width * inside.width,
                    height: region.height * inside.height
                ).insetBy(dx: 0.25, dy: 0.25)
                context.fill(Path(roundedRect: cell, cornerRadius: 1), with: .foreground)
            }
            .frame(width: width, height: (width * 0.75).rounded())
        } else {
            Image(systemName: action.symbol)
        }
    }
}

/// How the Window Management settings pane groups the actions.
enum WindowActionGroup: CaseIterable, Identifiable {
    case halves, quarters, thirds, sixths, sizeAndPosition, displays

    var id: Self { self }

    var title: String {
        switch self {
        case .halves: "Halves"
        case .quarters: "Quarters"
        case .thirds: "Thirds"
        case .sixths: "Sixths"
        case .sizeAndPosition: "Size & Position"
        case .displays: "Displays"
        }
    }

    var actions: [WindowAction] {
        switch self {
        case .halves: [.leftHalf, .rightHalf, .topHalf, .bottomHalf]
        case .quarters: [.topLeft, .topRight, .bottomLeft, .bottomRight]
        case .thirds: [.leftThird, .centerThird, .rightThird, .leftTwoThirds, .rightTwoThirds]
        case .sixths: [.topLeftSixth, .topCenterSixth, .topRightSixth, .bottomLeftSixth, .bottomCenterSixth, .bottomRightSixth]
        case .sizeAndPosition: [.maximize, .almostMaximize, .center, .restore]
        case .displays: [.nextDisplay, .previousDisplay]
        }
    }
}
