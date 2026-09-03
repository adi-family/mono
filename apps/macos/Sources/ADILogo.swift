import SwiftUI

/// The ADI mark in the window: three hexagons, one in front, drawn from [`Trefoil`].
///
/// Monochrome, in the ink it is given — `currentColor` at 52%, 74% and 100%, the way every other
/// copy of the mark in the product draws it (`design/DESIGN.md` §10). The coloured build exists
/// for the app icon, which `icon-gen.swift` draws; in the window an orange lobe would be a second
/// orange on every screen.
///
/// Opaque ink rather than `.primary`: `.primary` is `labelColor`, which is only 85% opaque, so
/// the front lobe would let the lobe behind it bleed through and the hairline gaps would show the
/// mid lobe instead of the ground.
struct ADILogo: View {
    var size: CGFloat = 18
    var build: Trefoil.Build = .window
    var ink: Color = ADI.ink

    var body: some View {
        Canvas { context, canvas in
            let scale = min(canvas.width, canvas.height) / Trefoil.box
            func point(_ p: (x: Double, y: Double)) -> CGPoint {
                CGPoint(x: p.x * scale, y: p.y * scale)
            }

            for index in Trefoil.angles.indices {
                let lobe = path(centre: Trefoil.centre(index), radius: Trefoil.lobeRadius, map: point)

                // A lobe in front leaves a hairline of ground in the one behind it, and that has
                // to be a genuine *subtraction*. Adding the cutter to the path and filling
                // even-odd looks equivalent and is not: the part of the cutter that falls outside
                // this lobe is covered once, so it fills — which painted a rim of the wrong colour
                // around the front lobe. Erasing inside a layer clips the damage to the lobe.
                context.drawLayer { layer in
                    layer.fill(lobe, with: .color(fill(index)))
                    layer.blendMode = .destinationOut
                    for cutter in Trefoil.cutters(of: index, build) {
                        layer.fill(path(centre: cutter,
                                        radius: Trefoil.lobeRadius + Trefoil.cutBleed,
                                        map: point),
                                   with: .color(.black))
                    }
                }
            }
        }
        .frame(width: size, height: size)
        // Every place it is drawn pairs it with the wordmark, and a screen reader announcing
        // "adi" twice is worse than not drawing it at all.
        .accessibilityHidden(true)
    }

    private func fill(_ index: Int) -> Color {
        if build.colored, let rgb = Trefoil.coloredFills[index] {
            return Color(red: rgb.r, green: rgb.g, blue: rgb.b)
        }
        return ink.opacity(Trefoil.tones[index])
    }

    private func path(centre: (x: Double, y: Double), radius: Double,
                      map: ((x: Double, y: Double)) -> CGPoint) -> Path {
        var path = Path()
        path.addLines(Trefoil.hexagon(centre: centre, radius: radius).map(map))
        path.closeSubpath()
        return path
    }
}
