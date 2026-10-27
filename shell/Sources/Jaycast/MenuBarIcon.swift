import AppKit

enum MenuBarIcon {
    /// A multitool drawn as a template image (tinted by the menu bar like a symbol):
    /// a filled body on the left with a can opener and a knife swung out from one hinge at its base.
    static let multitool: NSImage = {
        let image = NSImage(size: NSSize(width: 18, height: 18), flipped: true) { _ in
            guard let ctx = NSGraphicsContext.current?.cgContext else { return false }
            // Drawn on a 64-unit grid with the origin top left.
            ctx.scaleBy(x: 18 / 64, y: 18 / 64)
            ctx.beginTransparencyLayer(auxiliaryInfo: nil)
            ctx.setLineWidth(4.5)
            ctx.setLineCap(.round)

            // Each tool is drawn along its own axis (x out from the hinge, y across its width), then swung into place.
            let hinge = CGPoint(x: 20, y: 49)
            func swing(_ path: CGPath, degrees: CGFloat, flipped: Bool) -> CGPath {
                var lay = CGAffineTransform(translationX: hinge.x, y: hinge.y).rotated(by: degrees * .pi / 180)
                if flipped { lay = lay.scaledBy(x: 1, y: -1) }
                return path.copy(using: &lay)!
            }

            // Knife, straight out at 90°: spine along the top to a sharp tip, edge underneath.
            let knife = CGMutablePath()
            knife.move(to: CGPoint(x: 4, y: 0))
            knife.addLine(to: CGPoint(x: 42, y: 0))
            knife.addQuadCurve(to: CGPoint(x: 33, y: 8), control: CGPoint(x: 39, y: 9))
            knife.addLine(to: CGPoint(x: 4, y: 10))
            ctx.setLineJoin(.miter)
            ctx.addPath(swing(knife, degrees: 0, flipped: false))
            ctx.strokePath()

            // Can opener, up at 45°: a shaft ending in a hooked point with a scooped notch under it.
            let opener = CGMutablePath()
            opener.move(to: CGPoint(x: 4, y: 0))
            opener.addLine(to: CGPoint(x: 39, y: 0))
            opener.addQuadCurve(to: CGPoint(x: 44, y: 13), control: CGPoint(x: 48, y: 3))
            opener.addQuadCurve(to: CGPoint(x: 32, y: 13), control: CGPoint(x: 37, y: 5))
            opener.addLine(to: CGPoint(x: 4, y: 13))
            ctx.setLineJoin(.round)
            ctx.addPath(swing(opener, degrees: -45, flipped: true))
            ctx.strokePath()

            // Clear a gap around the body so the tools tuck behind it, then fill the body.
            let body = CGRect(x: 4, y: 3, width: 16, height: 58)
            ctx.setBlendMode(.clear)
            ctx.addPath(CGPath(roundedRect: body.insetBy(dx: -3, dy: -3), cornerWidth: 11, cornerHeight: 11, transform: nil))
            ctx.fillPath()
            ctx.setBlendMode(.normal)
            ctx.addPath(CGPath(roundedRect: body, cornerWidth: 8, cornerHeight: 8, transform: nil))
            ctx.fillPath()

            ctx.endTransparencyLayer()
            return true
        }
        image.isTemplate = true
        image.accessibilityDescription = "Jaycast"
        return image
    }()
}
