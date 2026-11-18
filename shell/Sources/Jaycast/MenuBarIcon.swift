import AppKit

enum MenuBarIcon {
    /// A backpack drawn as a template image (tinted by the menu bar like a symbol): a domed pack with a
    /// grab loop on top and a front pocket with a zip across it, the pocket and zip cut out at the same weight.
    static let backpack: NSImage = {
        let image = NSImage(size: NSSize(width: 19, height: 19), flipped: true) { _ in
            guard let ctx = NSGraphicsContext.current?.cgContext else { return false }
            // Drawn on a 64-unit grid with the origin top left.
            ctx.scaleBy(x: 19 / 64, y: 19 / 64)
            // Scaled up around the drawing's centre so the loop and base reach nearly to the edges of the box.
            ctx.translateBy(x: 32, y: 31.65)
            ctx.scaleBy(x: 1.12, y: 1.12)
            ctx.translateBy(x: -32, y: -31.65)
            ctx.beginTransparencyLayer(auxiliaryInfo: nil)

            // Body: rounder corners at the top than the bottom.
            let body = CGRect(x: 13, y: 13, width: 38, height: 46)
            let top: CGFloat = 15, bottom: CGFloat = 8
            let pack = CGMutablePath()
            pack.move(to: CGPoint(x: body.minX, y: body.midY))
            pack.addArc(tangent1End: CGPoint(x: body.minX, y: body.minY), tangent2End: CGPoint(x: body.midX, y: body.minY), radius: top)
            pack.addArc(tangent1End: CGPoint(x: body.maxX, y: body.minY), tangent2End: CGPoint(x: body.maxX, y: body.midY), radius: top)
            pack.addArc(tangent1End: CGPoint(x: body.maxX, y: body.maxY), tangent2End: CGPoint(x: body.midX, y: body.maxY), radius: bottom)
            pack.addArc(tangent1End: CGPoint(x: body.minX, y: body.maxY), tangent2End: CGPoint(x: body.minX, y: body.midY), radius: bottom)
            pack.closeSubpath()

            // Grab loop: an arch whose legs drop into the top of the pack.
            let loop = CGMutablePath()
            loop.move(to: CGPoint(x: 26, y: 15))
            loop.addArc(tangent1End: CGPoint(x: 26, y: 6), tangent2End: CGPoint(x: 32, y: 6), radius: 5)
            loop.addArc(tangent1End: CGPoint(x: 38, y: 6), tangent2End: CGPoint(x: 38, y: 15), radius: 5)
            loop.addLine(to: CGPoint(x: 38, y: 15))
            ctx.setLineWidth(3.4)
            ctx.setLineJoin(.round)
            ctx.addPath(loop)
            ctx.strokePath()

            // Cut a thin gap where the loop meets the pack, then fill the pack over the ends of the loop.
            ctx.setBlendMode(.clear)
            ctx.setLineWidth(3.2)
            ctx.addPath(pack)
            ctx.strokePath()
            ctx.setBlendMode(.normal)
            ctx.addPath(pack)
            ctx.fillPath()

            // Pocket outline and zip, cut out of the body at the same line weight.
            ctx.setBlendMode(.clear)
            ctx.setLineWidth(2.6)
            ctx.addPath(CGPath(roundedRect: CGRect(x: 18, y: 34.5, width: 28, height: 19.5), cornerWidth: 5.5, cornerHeight: 5.5, transform: nil))
            ctx.strokePath()
            ctx.setLineCap(.round)
            ctx.move(to: CGPoint(x: 22.5, y: 41.5))
            ctx.addLine(to: CGPoint(x: 41.5, y: 41.5))
            ctx.strokePath()

            ctx.endTransparencyLayer()
            return true
        }
        image.isTemplate = true
        image.accessibilityDescription = "Jaycast"
        return image
    }()
}
