import AppKit

enum MenuBarIcon {
    /// A turbocharger drawn as a template image (tinted by the menu bar like a symbol): a round
    /// housing whose top edge runs straight out into an outlet pipe with a rounded end, and the inlet
    /// cut out of the housing around a hub dot.
    static let turbo: NSImage = {
        let image = NSImage(size: NSSize(width: 18, height: 18), flipped: true) { _ in
            guard let ctx = NSGraphicsContext.current?.cgContext else { return false }
            // Drawn on a 64-unit grid with the origin top left.
            ctx.scaleBy(x: 18 / 64, y: 18 / 64)
            // Scaled up around the grid's centre so the 53-unit-wide ink nearly fills the box.
            ctx.translateBy(x: 32, y: 32)
            ctx.scaleBy(x: 1.12, y: 1.12)
            ctx.translateBy(x: -32, y: -32)
            // The ink spans x 4.5–57.5 and y 14.5–59.5; move its centre (31, 37) to the grid's (32, 32).
            ctx.translateBy(x: 1, y: -5)
            ctx.beginTransparencyLayer(auxiliaryInfo: nil)

            // Housing and outlet: the top of the circle carries on as the pipe's top edge, and the
            // pipe's bottom edge meets the circle where it is a pipe's width below the top.
            let centre = CGPoint(x: 27, y: 37)
            let radius: CGFloat = 21, pipe: CGFloat = 13, end: CGFloat = 56
            let top = centre.y - radius
            let join = CGPoint(x: centre.x + (radius * radius - (radius - pipe) * (radius - pipe)).squareRoot(), y: top + pipe)
            let housing = CGMutablePath()
            housing.move(to: CGPoint(x: centre.x, y: top))
            housing.addLine(to: CGPoint(x: end, y: top))
            housing.addLine(to: CGPoint(x: end, y: join.y))
            housing.addLine(to: join)
            // The long way round, under the inlet, back up to the top.
            housing.addArc(center: centre, radius: radius,
                           startAngle: atan2(join.y - centre.y, join.x - centre.x), endAngle: -.pi / 2, clockwise: false)
            housing.closeSubpath()
            // Filled and stroked so the pipe's end and the joins come out rounded.
            ctx.setLineWidth(3)
            ctx.setLineJoin(.round)
            ctx.addPath(housing)
            ctx.drawPath(using: .fillStroke)

            // Inlet cut out, then the hub put back in its middle.
            ctx.setBlendMode(.clear)
            ctx.fillEllipse(in: CGRect(x: centre.x - 9, y: centre.y - 9, width: 18, height: 18))
            ctx.setBlendMode(.normal)
            ctx.fillEllipse(in: CGRect(x: centre.x - 3.4, y: centre.y - 3.4, width: 6.8, height: 6.8))

            ctx.endTransparencyLayer()
            return true
        }
        image.isTemplate = true
        image.accessibilityDescription = "Turbo"
        return image
    }()
}
