import Foundation

/// The ADI mark: three hexagons at 120°, drawn back to front, weak to strong.
///
/// This is the only definition of the geometry on the Swift side. `ADILogo` draws it in the
/// window and `icon-gen.swift` draws it for the `.icns`; before this file existed each had its
/// own copy of the coordinates and a comment asking whoever came next to keep them in step.
///
/// **The draw order is the design.** An earlier version painted the lobes in the order they are
/// listed but ran the tones the other way, so the lobe visually in front was the faintest — on a
/// white ground that laid a wash of pale grey over the whole mark and the top lobe read grey
/// rather than solid. Back to front is weak to strong, and nothing may reorder it.
///
/// The mark never names its own colour: every lobe is the caller's ink at one of `tones`, so the
/// same geometry works on any ground. See the `Build` cases for the two axes that vary.
enum Trefoil {
    /// The design box every coordinate below is expressed in.
    static let box: Double = 200

    static let lobeRadius: Double = 56
    /// How far each lobe's centre sits from the middle of the box.
    static let centreOffset: Double = 32

    /// Back, middle, front — in paint order. Front is the top lobe.
    static let angles: [Double] = [150, 30, -90]

    /// Opacity of the caller's ink, per lobe, in the same order.
    ///
    /// The range is deliberately shallow at the bottom: the front lobe carries the form at full
    /// strength, so the two behind only have to be *told apart* from each other and from the
    /// ground. 52% is far enough from white to stay visible and far enough from 74% to read as a
    /// separate plane.
    static let tones: [Double] = [0.52, 0.74, 1.0]

    /// A lobe in front knocks a slightly larger hexagon out of whatever is behind it, leaving a
    /// hairline of ground between them. Below about 24pt the tones converge and this gap is the
    /// only thing keeping three shapes from reading as one.
    static let cutBleed: Double = 3

    /// Which of the two independent choices a drawing makes.
    ///
    /// They are orthogonal: `cut` is about the gaps between lobes, `accented` is about what the
    /// middle lobe is filled with. Small sizes want the gaps; a surface whose ground we control
    /// can afford the accent; the marketing lockup wants neither and lets the lobes mix.
    struct Build {
        /// Hairline gaps between the lobes. On below ~24pt, off above ~64pt.
        var cut: Bool = true
        /// The middle lobe becomes the accent instead of ink.
        ///
        /// Never enable this on an accent-coloured ground — the lobe disappears into it.
        var accented: Bool = false
        /// Lobes mix rather than stack: richer above ~96pt, muddy below it.
        var translucent: Bool = false

        static let icon = Build(cut: true, accented: true)
        static let window = Build(cut: false, accented: true)
        static let large = Build(cut: false, accented: false, translucent: true)
    }

    /// Two lobes sit low and one sits high, so the three lobe centres average to the middle of
    /// the box while the *drawn* shape does not: it spans y 12…172, which is 8 units high. Every
    /// consumer would otherwise have to re-derive this, so it is applied here, once.
    static let verticalNudge: Double = 8

    /// The centre of lobe `index`, in box coordinates, with y running downward.
    static func centre(_ index: Int) -> (x: Double, y: Double) {
        let radians = angles[index] * .pi / 180
        return (box / 2 + centreOffset * cos(radians),
                box / 2 + centreOffset * sin(radians) + verticalNudge)
    }

    /// A pointy-top hexagon's six corners, clockwise from the top.
    static func hexagon(centre: (x: Double, y: Double), radius: Double) -> [(x: Double, y: Double)] {
        (0..<6).map { corner in
            let radians = (Double(corner) * 60 - 90) * .pi / 180
            return (centre.x + radius * cos(radians), centre.y + radius * sin(radians))
        }
    }

    /// The opacity lobe `index` is painted at under `build`.
    static func tone(_ index: Int, _ build: Build) -> Double {
        build.translucent ? [0.42, 0.60, 0.88][index] : tones[index]
    }

    // MARK: gloss
    //
    // The lobes are lit, not filled flat: a specular wash over the upper part of each and a
    // shade under the lower, clipped to the lobe so the gloss never leaks into the hairline gaps.
    //
    // The lighting is white and black laid *over* the lobe, never a ramp in the lobe's own
    // alpha. Grading the alpha is the obvious way to do it and it is wrong — the front lobe
    // turns translucent at its foot and whatever is behind it shows through.
    //
    // Kept as numbers here rather than as three hand-tuned gradients, because the SVG on the
    // `.adi` pages has to arrive at the same surface as the Swift renderers do.

    /// White over the top of the lobe: the specular.
    static let sheenAlpha: Double = 0.38
    /// Where the specular has faded to nothing, as a fraction of the lobe's height.
    static let sheenEnd: Double = 0.55
    /// Black under the bottom of the lobe, so the gloss reads as a curved surface rather than a
    /// light leak.
    static let shadeAlpha: Double = 0.28 

    /// The accent, lit. Three stops rather than two: the middle one keeps `#FA5019` itself
    /// present in the surface instead of leaving only the two ends of a ramp.
    static let accentStops: [(location: Double, rgb: (Double, Double, Double))] = [
        (0.00, (1.000, 0.541, 0.290)),   // #FF8A4A
        (0.55, (0.980, 0.314, 0.098)),   // #FA5019
        (1.00, (0.847, 0.220, 0.039)),   // #D8380A
    ]

    /// The vertical extent of lobe `index`, in box coordinates — where its gradients start and
    /// stop.
    static func span(_ index: Int) -> (top: Double, bottom: Double) {
        let c = centre(index)
        return (c.y - lobeRadius, c.y + lobeRadius)
    }

    /// The lobes that sit in front of `index` and therefore cut into it — empty when the build
    /// has no gaps, and always empty for the front lobe, which nothing covers.
    static func cutters(of index: Int, _ build: Build) -> [(x: Double, y: Double)] {
        guard build.cut else { return [] }
        return ((index + 1)..<angles.count).map { centre($0) }
    }
}
