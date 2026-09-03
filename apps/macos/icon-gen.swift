// Generates the ADI app-icon master PNG (1024×1024): the mark, in its coloured build, on a flat
// paper tile (design/DESIGN.md §10 — the coloured version is for the app icon and the landing).
// Not part of the app; regenerate with `build.sh --regen-icon`, which then runs it through
// sips + iconutil to produce apps/macos/ADI.icns. `--ios` emits the iOS master instead, which
// `apps/ios/build.sh --regen-icon` writes into the app's asset catalogue.
//
// Compiled *with* Sources/Trefoil.swift rather than carrying its own copy of the coordinates —
// which is what this file, ADILogo.swift and the .adi 4XX page each used to do, with a comment
// in every one asking the next person to keep the three in step. That is also why it is
// `@main` instead of a top-level script: swiftc only allows top-level code in main.swift, and
// two files have to be compiled together for the geometry to be shared.
//
// The same reasoning is why iOS is a *flag here* rather than a second generator under apps/ios:
// the two platforms differ only in their canvas, and a copied drawMark is exactly the drift this
// file was written to end. It had already happened once — the icon shipped in AdiFleet until
// 2026-08-31 was a wireframe hexagon from before the Trefoil existed, because nothing regenerated
// it when the mark changed.
//
//   swiftc -parse-as-library -O Sources/Trefoil.swift icon-gen.swift -o icon-gen
//   ./icon-gen /tmp/icon_1024.png            # macOS master
//   ./icon-gen --ios /tmp/icon_ios_1024.png  # iOS master

import AppKit

@main
struct IconGen {
    static let size: CGFloat = 1024

    /// What the canvas looks like on one platform. Only the ground differs between the two —
    /// the mark itself is `drawMark`, unchanged, on whatever square it is handed.
    struct Canvas {
        /// How far the tile is inset from the full canvas.
        let inset: CGFloat
        /// Corner radius as a fraction of the tile's width.
        let cornerRatio: CGFloat
        /// How much of the *tile* the mark occupies.
        let markRatio: CGFloat
        /// Whether the PNG keeps an alpha channel.
        let hasAlpha: Bool

        /// The macOS icon grid: a rounded tile floating inside a larger, transparent canvas.
        /// The inset is the shadow gutter every macOS icon leaves for itself.
        static let macOS = Canvas(inset: 100, cornerRatio: 0.2237, markRatio: 0.66, hasAlpha: true)

        /// iOS is the opposite arrangement: the artwork is full-bleed and the *system* applies
        /// the corner mask, so drawing our own rounded corners would round an already-rounded
        /// icon and drawing our own inset would leave a visible margin inside the mask. Alpha is
        /// off because App Store Connect rejects an icon that has any (ITMS-90717).
        ///
        /// `markRatio` is smaller than the macOS 0.66 to land at the same *optical* size: there
        /// it is 66% of an 824pt tile (≈544pt of 1024), here the tile is the whole 1024, and
        /// 0.55 puts the mark back at ≈563pt — a shade larger, which is right once the corner
        /// mask eats the four corners.
        static let iOS = Canvas(inset: 0, cornerRatio: 0, markRatio: 0.55, hasAlpha: false)
    }

    /// The tile's ink — `--ink` on paper, from design/tokens.css's light set.
    static let ink = NSColor(srgbRed: 0.090, green: 0.082, blue: 0.059, alpha: 1)   // #17150F
    /// The tile itself: paper, flat. A gradient here would be the one thing §10 forbids.
    static let paper = NSColor(srgbRed: 0.984, green: 0.980, blue: 0.973, alpha: 1)  // #FBFAF8

    static func main() {
        var args = Array(CommandLine.arguments.dropFirst())
        let canvas: Canvas = args.first == "--ios" ? .iOS : .macOS
        if args.first == "--ios" { args.removeFirst() }
        let outPath = args.first ?? "icon_1024.png"

        // Built as a CGBitmapContext rather than from an NSBitmapImageRep because an *opaque*
        // rep (samplesPerPixel 3) is not a format NSGraphicsContext will back — it hands back a
        // context with no `cgContext`, and the first thing drawMark does is ask for one.
        // `noneSkipLast` is the supported opaque layout, and it is what keeps alpha out of the
        // iOS PNG. The erase pass still works on it: `.destinationOut` runs inside a
        // transparency layer, which has its own alpha and composites onto this opaque ground.
        guard let cg = CGContext(
            data: nil, width: Int(size), height: Int(size), bitsPerComponent: 8, bytesPerRow: 0,
            space: CGColorSpaceCreateDeviceRGB(),
            bitmapInfo: (canvas.hasAlpha ? CGImageAlphaInfo.premultipliedLast
                                         : CGImageAlphaInfo.noneSkipLast).rawValue
        ) else { fatalError("could not allocate bitmap") }

        NSGraphicsContext.saveGraphicsState()
        // `flipped: false` keeps the bottom-left origin the explicit flip below expects.
        NSGraphicsContext.current = NSGraphicsContext(cgContext: cg, flipped: false)

        // Flip to a top-left origin so Trefoil's coordinates map straight through.
        let flip = NSAffineTransform()
        flip.translateX(by: 0, yBy: size)
        flip.scaleX(by: 1, yBy: -1)
        flip.concat()

        let content = CGRect(x: canvas.inset, y: canvas.inset,
                             width: size - 2 * canvas.inset, height: size - 2 * canvas.inset)
        drawTile(in: content, canvas)
        drawMark(in: content, canvas)

        NSGraphicsContext.restoreGraphicsState()

        guard let image = cg.makeImage() else { fatalError("could not snapshot the context") }
        guard let png = NSBitmapImageRep(cgImage: image).representation(using: .png, properties: [:]) else {
            fatalError("PNG encode failed")
        }
        do {
            try png.write(to: URL(fileURLWithPath: outPath))
        } catch {
            fatalError("could not write \(outPath): \(error)")
        }
        print("wrote \(outPath)")
    }

    private static func drawTile(in content: CGRect, _ canvas: Canvas) {
        NSGraphicsContext.saveGraphicsState()
        // A zero corner ratio means the tile *is* the canvas (iOS), where clipping to a rounded
        // path would carve corners the system is about to carve again.
        if canvas.cornerRatio > 0 {
            let radius = content.width * canvas.cornerRatio
            NSBezierPath(roundedRect: content, xRadius: radius, yRadius: radius).addClip()
        }
        paper.setFill()
        content.fill()
        NSGraphicsContext.restoreGraphicsState()
    }

    /// The icon build: cut *and* coloured. The two are independent choices — `cut` is the
    /// hairline gaps that keep the lobes apart once this is scaled to a 16pt Dock icon, and
    /// `colored` is the grey / orange / ink fills. An icon needs both.
    private static func drawMark(in content: CGRect, _ canvas: Canvas) {
        let build = Trefoil.Build.icon
        let extent = content.width * canvas.markRatio
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

            // Flat fills, back to front: grey, the accent, ink (Trefoil.coloredFills).
            let fill: NSColor
            if build.colored, let rgb = Trefoil.coloredFills[index] {
                fill = NSColor(srgbRed: rgb.r, green: rgb.g, blue: rgb.b, alpha: 1)
            } else {
                fill = ink.withAlphaComponent(CGFloat(Trefoil.tones[index]))
            }
            fill.setFill()
            lobe.fill()

            cg.setBlendMode(.destinationOut)
            for cutter in cutters {
                hexagon(centre: cutter, radius: Trefoil.lobeRadius + Trefoil.cutBleed).fill()
            }
            cg.setBlendMode(.normal)
            cg.endTransparencyLayer()
        }
    }
}
