import Foundation
import SwiftUI

/// The ADI mark in the window: three hexagons at 120°, back to front, weak to strong.
///
/// Geometry comes from [`Trefoil`] rather than living here — this file used to carry its own
/// copy of the coordinates, as did `icon-gen.swift` and the `.adi` 4XX page, each with a comment
/// asking the next person to keep the three in sync.
///
/// The window is a dark surface we control, so it takes the accented build: ink for the front
/// and back lobes, the accent for the middle one. Ink is `.primary`, so the mark follows the
/// system appearance the way the rest of the panel does.
struct ADILogo: View {
    var size: CGFloat = 60
    var build: Trefoil.Build = .window

    @Environment(\.colorScheme) private var scheme

    /// `#FA5019`. The one accent, shared with the disk image and the web pages.
    static let accent = Color(red: 0.980, green: 0.314, blue: 0.098)

    /// Explicit, opaque ink rather than `.primary`.
    ///
    /// `.primary` is `labelColor`, which is only **85% opaque** — so the front lobe let the
    /// accent behind it bleed through, and the hairline gaps showed the mid lobe instead of the
    /// ground. Both are the same bug, and neither is visible until the lobes overlap something
    /// coloured. These are the design tokens' own ink values, so the mark also stops depending
    /// on whatever the system decides a label should look like.
    private var ink: Color {
        scheme == .dark
            ? Color(red: 0.929, green: 0.906, blue: 0.855)   // #EDE7DA
            : Color(red: 0.090, green: 0.078, blue: 0.059)   // #17140F
    }

    var body: some View {
        Canvas { context, canvas in
            let scale = min(canvas.width, canvas.height) / Trefoil.box
            func point(_ p: (x: Double, y: Double)) -> CGPoint {
                CGPoint(x: p.x * scale, y: p.y * scale)
            }

            for index in Trefoil.angles.indices {
                let lobe = path(centre: Trefoil.centre(index), radius: Trefoil.lobeRadius, map: point)
                let span = Trefoil.span(index)
                let top = CGPoint(x: 0, y: span.top * scale)
                let bottom = CGPoint(x: 0, y: span.bottom * scale)
                let isAccent = build.accented && index == 1

                // A lobe in front leaves a hairline of ground in the one behind it, and that has
                // to be a genuine *subtraction*. Adding the cutter to the path and filling
                // even-odd looks equivalent and is not: the part of the cutter that falls outside
                // this lobe is covered once, so it fills — which painted an accent rim right
                // around the front lobe. Erasing inside a layer clips the damage to the lobe.
                context.drawLayer { layer in
                    layer.fill(lobe, with: .linearGradient(base(index, isAccent),
                                                           startPoint: top, endPoint: bottom))
                    layer.fill(lobe, with: .linearGradient(Self.gloss,
                                                           startPoint: top, endPoint: bottom))
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
        .accessibilityLabel("ADI")
    }

    /// The lobe's own colour, graded from its top edge to its bottom one.
    private func base(_ index: Int, _ isAccent: Bool) -> Gradient {
        if isAccent {
            return Gradient(stops: Trefoil.accentStops.map { stop in
                .init(color: Color(red: stop.rgb.0, green: stop.rgb.1, blue: stop.rgb.2),
                      location: stop.location)
            })
        }
        // Flat, deliberately. Grading an ink lobe by *opacity* is the obvious way to light it
        // and it is wrong: the front lobe ends up 72% transparent at its foot and the accent
        // behind shows straight through. All the lighting comes from the gloss overlay, which
        // is white and black over the top rather than a change in the lobe's own alpha.
        return Gradient(colors: [ink.opacity(Trefoil.tone(index, build))])
    }

    /// The specular over the top and the shade under the bottom, laid on whatever the lobe is.
    /// One gradient rather than two fills — the middle pair of stops is the hand-off.
    private static let gloss = Gradient(stops: [
        .init(color: .white.opacity(Trefoil.sheenAlpha), location: 0),
        .init(color: .white.opacity(0), location: Trefoil.sheenEnd),
        .init(color: .black.opacity(0), location: Trefoil.sheenEnd),
        .init(color: .black.opacity(Trefoil.shadeAlpha), location: 1),
    ])

    private func path(centre: (x: Double, y: Double), radius: Double,
                      map: ((x: Double, y: Double)) -> CGPoint) -> Path {
        var path = Path()
        path.addLines(Trefoil.hexagon(centre: centre, radius: radius).map(map))
        path.closeSubpath()
        return path
    }
}
