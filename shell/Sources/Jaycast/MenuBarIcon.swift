import AppKit

enum MenuBarIcon {
    /// SF Symbols has no rocket, so draw one as a template image (tinted by the menu bar like a symbol).
    static let rocket: NSImage = {
        let image = NSImage(size: NSSize(width: 18, height: 18), flipped: false) { _ in
            // Drawn upright around the origin, then tilted 45° and scaled to fill the square.
            let transform = NSAffineTransform()
            transform.translateX(by: 9, yBy: 9)
            transform.rotate(byDegrees: -45)
            transform.scaleX(by: 1.3, yBy: 1.3)
            transform.translateX(by: 0, yBy: 0.8)
            transform.concat()

            // One silhouette: pointed nose, straight body, curved fins joined by one arc across the base.
            let rocket = NSBezierPath()
            rocket.move(to: NSPoint(x: 0, y: 8))
            rocket.curve(to: NSPoint(x: 2.8, y: 1.5), controlPoint1: NSPoint(x: 1.8, y: 6.4), controlPoint2: NSPoint(x: 2.8, y: 4.2))
            rocket.line(to: NSPoint(x: 2.8, y: -1))
            rocket.curve(to: NSPoint(x: 5, y: -4.5), controlPoint1: NSPoint(x: 4.6, y: -1.6), controlPoint2: NSPoint(x: 5.2, y: -3))
            rocket.curve(to: NSPoint(x: -5, y: -4.5), controlPoint1: NSPoint(x: 2, y: -2.6), controlPoint2: NSPoint(x: -2, y: -2.6))
            rocket.curve(to: NSPoint(x: -2.8, y: -1), controlPoint1: NSPoint(x: -5.2, y: -3), controlPoint2: NSPoint(x: -4.6, y: -1.6))
            rocket.line(to: NSPoint(x: -2.8, y: 1.5))
            rocket.curve(to: NSPoint(x: 0, y: 8), controlPoint1: NSPoint(x: -2.8, y: 4.2), controlPoint2: NSPoint(x: -1.8, y: 6.4))
            rocket.close()
            rocket.fill()

            // Exhaust flame, set just off the base so the two shapes read separately.
            let flame = NSBezierPath()
            flame.move(to: NSPoint(x: -1.6, y: -3.9))
            flame.curve(to: NSPoint(x: 1.6, y: -3.9), controlPoint1: NSPoint(x: -0.8, y: -3.5), controlPoint2: NSPoint(x: 0.8, y: -3.5))
            flame.curve(to: NSPoint(x: 0, y: -7.5), controlPoint1: NSPoint(x: 1.6, y: -5.3), controlPoint2: NSPoint(x: 0.6, y: -6.5))
            flame.curve(to: NSPoint(x: -1.6, y: -3.9), controlPoint1: NSPoint(x: -0.6, y: -6.5), controlPoint2: NSPoint(x: -1.6, y: -5.3))
            flame.close()
            flame.fill()
            return true
        }
        image.isTemplate = true
        image.accessibilityDescription = "Jaycast"
        return image
    }()
}
