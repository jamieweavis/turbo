// Display names and settings groups for the core's window actions. The titles and symbols
// match the core's palette results (`core/src/windows/mod.rs`).

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

    var symbol: String {
        switch self {
        case .leftHalf: "rectangle.lefthalf.inset.filled"
        case .rightHalf: "rectangle.righthalf.inset.filled"
        case .topHalf: "rectangle.tophalf.inset.filled"
        case .bottomHalf: "rectangle.bottomhalf.inset.filled"
        case .topLeft: "rectangle.inset.topleft.filled"
        case .topRight: "rectangle.inset.topright.filled"
        case .bottomLeft: "rectangle.inset.bottomleft.filled"
        case .bottomRight: "rectangle.inset.bottomright.filled"
        case .leftThird, .leftTwoThirds: "rectangle.leftthird.inset.filled"
        case .centerThird: "rectangle.center.inset.filled"
        case .rightThird, .rightTwoThirds: "rectangle.rightthird.inset.filled"
        case .topLeftSixth: "square.grid.3x3.topleft.filled"
        case .topCenterSixth: "square.grid.3x3.topmiddle.filled"
        case .topRightSixth: "square.grid.3x3.topright.filled"
        case .bottomLeftSixth: "square.grid.3x3.bottomleft.filled"
        case .bottomCenterSixth: "square.grid.3x3.bottommiddle.filled"
        case .bottomRightSixth: "square.grid.3x3.bottomright.filled"
        case .maximize: "rectangle.inset.filled"
        case .almostMaximize: "rectangle.dashed"
        case .center: "rectangle.center.inset.filled"
        case .restore: "arrow.uturn.backward"
        case .nextDisplay: "arrow.right.square"
        case .previousDisplay: "arrow.left.square"
        }
    }
}

/// How the Windows settings pane groups the actions.
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
