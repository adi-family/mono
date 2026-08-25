// Generates the ADI app-icon master PNG (1024×1024): the Trefoil mark on a light squircle tile.
// Not part of the app; regenerate with `build.sh --regen-icon`, which then runs it through
// sips + iconutil to produce apps/macos/ADI.icns.
//
// Compiled *with* Sources/Trefoil.swift rather than carrying its own copy of the coordinates —
// which is what this file, ADILogo.swift and the .adi 4XX page each used to do, with a comment
// in every one asking the next person to keep the three in step. That is also why it is
// `@main` instead of a top-level script: swiftc only allows top-level code in main.swift, and
// two files have to be compiled together for the geometry to be shared.
//
//   swiftc -parse-as-library -O Sources/Trefoil.swift icon-gen.swift -o icon-gen
//   ./icon-gen /tmp/icon_1024.png

import AppKit

@main
struct IconGen {
    static let size: CGFloat = 1024
    /// The macOS icon grid: the content square is inset from the full canvas.
    static let inset: CGFloat = 100
    static let cornerRatio: CGFloat = 0.2237
    /// How much of the content square the mark occupies.
    static let markRatio: CGFloat = 0.66

    static let ink = NSColor(srgbRed: 0.078, green: 0.094, blue: 0.114, alpha: 1)   // #14181d
    static let accent = NSColor(srgbRed: 0.980, green: 0.314, blue: 0.098, alpha: 1) // #FA5019

    static func main() {
        let outPath = CommandLine.arguments.count > 1 ? CommandLine.arguments[1] : "icon_1024.png"

        guard let rep = NSBitmapImageRep(
            bitmapDataPlanes: nil, pixelsWide: Int(size), pixelsHigh: Int(size),
            bitsPerSample: 8, samplesPerPixel: 4, hasAlpha: true, isPlanar: false,
            colorSpaceName: .deviceRGB, bytesPerRow: 0, bitsPerPixel: 0
        ) else { fatalError("could not allocate bitmap") }

        NSGraphicsContext.saveGraphicsState()
        NSGraphicsContext.current = NSGraphicsContext(bitmapImageRep: rep)

        // Flip to a top-left origin so Trefoil's coordinates map straight through.
        let flip = NSAffineTransform()
        flip.translateX(by: 0, yBy: size)
        flip.scaleX(by: 1, yBy: -1)
        flip.concat()

        let content = CGRect(x: inset, y: inset, width: size - 2 * inset, height: size - 2 * inset)
        drawTile(in: content)
        drawMark(in: content)

        NSGraphicsContext.restoreGraphicsState()

        guard let png = rep.representation(using: .png, properties: [:]) else {
            fatalError("PNG encode failed")
        }
        do {
            try png.write(to: URL(fileURLWithPath: outPath))
        } catch {
            fatalError("could not write \(outPath): \(error)")
        }
        print("wrote \(outPath)")
    }

    /// The accent, lit — three stops so `#FA5019` itself stays present in the surface.
    private static let accentGradient: NSGradient = {
        let colors = Trefoil.accentStops.map {
            NSColor(srgbRed: CGFloat($0.rgb.0), green: CGFloat($0.rgb.1),
                    blue: CGFloat($0.rgb.2), alpha: 1)
        }
        let locations = Trefoil.accentStops.map { CGFloat($0.location) }
        return NSGradient(colors: colors, atLocations: locations, colorSpace: .sRGB)!
    }()

    /// The specular over the top and the shade under the bottom, laid on whatever the lobe is.
    private static let gloss = NSGradient(
        colors: [NSColor(white: 1, alpha: CGFloat(Trefoil.sheenAlpha)),
                 NSColor(white: 1, alpha: 0),
                 NSColor(white: 0, alpha: 0),
                 NSColor(white: 0, alpha: CGFloat(Trefoil.shadeAlpha))],
        atLocations: [0, CGFloat(Trefoil.sheenEnd), CGFloat(Trefoil.sheenEnd), 1],
        colorSpace: .sRGB)!

    private static func drawTile(in content: CGRect) {
        NSGraphicsContext.saveGraphicsState()
        let radius = content.width * cornerRatio
        NSBezierPath(roundedRect: content, xRadius: radius, yRadius: radius).addClip()
        NSGradient(starting: NSColor(srgbRed: 1, green: 1, blue: 1, alpha: 1),
                   ending: NSColor(srgbRed: 0.93, green: 0.94, blue: 0.95, alpha: 1))?
            .draw(in: content, angle: -90)
        NSGraphicsContext.restoreGraphicsState()
    }

    /// The icon build: cut *and* accented. The two are independent choices — `cut` is the
    /// hairline gaps that keep the lobes apart once this is scaled to a 16pt Dock icon, and
    /// `accented` is what the middle lobe is filled with. An icon needs both.
    private static func drawMark(in content: CGRect) {
        let build = Trefoil.Build.icon
        let extent = content.width * markRatio
        let scale = extent / Trefoil.box
        let origin = CGPoint(x: content.midX - extent / 2, y: content.midY - extent / 2)

        func hexagon(centre: (x: Double, y: Double), radius: Double) -> NSBezierPath {
            let corners = Trefoil.hexagon(centre: centre, radius: radius).map { corner in
                NSPoint(x: origin.x + CGFloat(corner.x) * scale,
                        y: origin.y + CGFloat(corner.y) * scale)
            }
            let path = NSBezierPath()
            path.move(to: corners[0])
            for corner in corners.dropFirst() { path.line(to: corner) }
            path.close()
            return path
        }

        guard let cg = NSGraphicsContext.current?.cgContext else { fatalError("no context") }

        for index in Trefoil.angles.indices {
            let lobe = hexagon(centre: Trefoil.centre(index), radius: Trefoil.lobeRadius)
            let cutters = Trefoil.cutters(of: index, build)

            // A transparency layer, so `.destinationOut` erases only what this lobe painted.
            // Even-odd on a combined path is the obvious shortcut and is wrong: the part of the
            // cutter outside the lobe is covered once and fills, drawing a rim of the wrong
            // colour around the lobe in front. Erasing also means the gap shows the tile's
            // gradient rather than a flat colour seamed over it.
            cg.beginTransparencyLayer(auxiliaryInfo: nil)

            NSGraphicsContext.saveGraphicsState()
            lobe.addClip()
            if build.accented && index == 1 {
                accentGradient.draw(in: lobe.bounds, angle: -90)
            } else {
                // Flat, then lit by the gloss pass. Grading an ink lobe by opacity would make
                // the front one translucent at its foot and let the accent behind show through.
                ink.withAlphaComponent(CGFloat(Trefoil.tone(index, build))).setFill()
                lobe.bounds.fill()
            }
            gloss.draw(in: lobe.bounds, angle: -90)
            NSGraphicsContext.restoreGraphicsState()

            cg.setBlendMode(.destinationOut)
            for cutter in cutters {
                hexagon(centre: cutter, radius: Trefoil.lobeRadius + Trefoil.cutBleed).fill()
            }
            cg.setBlendMode(.normal)
            cg.endTransparencyLayer()
        }
    }
}
