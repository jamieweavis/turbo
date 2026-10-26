import AppKit

enum MenuBarIcon {
    /// SF Symbols has no rocket, so draw one as a template image (tinted by the menu bar like a symbol).
    static let rocket: NSImage = {
        let image = NSImage(size: NSSize(width: 18, height: 18), flipped: false) { _ in
            let transform = NSAffineTransform()
            transform.translateX(by: 9, yBy: 9)
            transform.rotate(byDegrees: -45)
            transform.translateX(by: -9, yBy: -9)
            transform.concat()

            // Body with a pointed nose, cut through by a round window.
            let body = NSBezierPath()
            body.move(to: NSPoint(x: 9, y: 17.5))
            body.curve(to: NSPoint(x: 11.6, y: 11.5), controlPoint1: NSPoint(x: 10.6, y: 16), controlPoint2: NSPoint(x: 11.6, y: 14))
            body.line(to: NSPoint(x: 11.6, y: 5.5))
            body.line(to: NSPoint(x: 6.4, y: 5.5))
            body.line(to: NSPoint(x: 6.4, y: 11.5))
            body.curve(to: NSPoint(x: 9, y: 17.5), controlPoint1: NSPoint(x: 6.4, y: 14), controlPoint2: NSPoint(x: 7.4, y: 16))
            body.close()
            body.append(NSBezierPath(ovalIn: NSRect(x: 7.6, y: 10.2, width: 2.8, height: 2.8)))
            body.windingRule = .evenOdd
            body.fill()

            // Fins, sitting just off the body so they read separately.
            for side: CGFloat in [-1, 1] {
                let fin = NSBezierPath()
                fin.move(to: NSPoint(x: 9 + side * 3.2, y: 9.5))
                fin.line(to: NSPoint(x: 9 + side * 5.2, y: 6.5))
                fin.line(to: NSPoint(x: 9 + side * 5.2, y: 3.8))
                fin.line(to: NSPoint(x: 9 + side * 3.2, y: 5.5))
                fin.close()
                fin.fill()
            }

            // Exhaust flame.
            let flame = NSBezierPath()
            flame.move(to: NSPoint(x: 7.6, y: 4.6))
            flame.line(to: NSPoint(x: 10.4, y: 4.6))
            flame.curve(to: NSPoint(x: 9, y: 0.5), controlPoint1: NSPoint(x: 10.4, y: 3), controlPoint2: NSPoint(x: 9.6, y: 1.6))
            flame.curve(to: NSPoint(x: 7.6, y: 4.6), controlPoint1: NSPoint(x: 8.4, y: 1.6), controlPoint2: NSPoint(x: 7.6, y: 3))
            flame.close()
            flame.fill()
            return true
        }
        image.isTemplate = true
        image.accessibilityDescription = "Jaycast"
        return image
    }()
}
