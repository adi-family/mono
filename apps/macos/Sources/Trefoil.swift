import Foundation

/// The ADI mark: three hexagons at 120°, one in front (`design/DESIGN.md` §10), drawn back to
/// front, weak to strong.
///
/// This is the only definition of the geometry on the Swift side. `ADILogo` draws it in the
/// window and `icon-gen.swift` draws it for the `.icns` and the iOS icon; before this file existed
/// each had its own copy of the coordinates and a comment asking whoever came next to keep them
/// in step. `crates/adi-ui/src/mark.rs` derives the same paths for the web and tests that they
/// agree with the literals in `crates/adi-hive/src/notfound.rs`.
///
/// **The draw order is the design.** An earlier version painted the lobes in the order they are
/// listed but ran the tones the other way, so the lobe visually in front was the faintest — on a
/// white ground that laid a wash of pale grey over the whole mark and the top lobe read grey
/// rather than solid. Back to front is weak to strong, and nothing may reorder it.
///
/// Flat. The mark takes the caller's ink at one of `tones`, or the three flat fills of the
/// coloured build. No gloss, no gradient, no shadow — those were the mark's "lighting" until
/// September 2026, and §10 says it is not a mascot.
enum Trefoil {
    /// The design box every coordinate below is expressed in.
    static let box: Double = 200

    static let lobeRadius: Double = 56
    /// How far each lobe's centre sits from the middle of the box.
    static let centreOffset: Double = 32

    /// Back, middle, front — in paint order. Front is the top lobe.
    static let angles: [Double] = [150, 30, -90]

    /// Opacity of the caller's ink, per lobe, in the same order (§10: 52%, 74%, 100%).
    ///
    /// The range is deliberately shallow at the bottom: the front lobe carries the form at full
    /// strength, so the two behind only have to be *told apart* from each other and from the
    /// ground. 52% is far enough from white to stay visible and far enough from 74% to read as a
    /// separate plane.
    static let tones: [Double] = [0.52, 0.74, 1.0]

    /// The coloured build's fills, back to front: grey, orange, and the ink of wherever it sits
    /// (`nil` here — the caller's ink). Literal on purpose: this is the app icon, and an icon
    /// that changed with the page would be a different icon.
    static let coloredFills: [(r: Double, g: Double, b: Double)?] = [
        (0.549, 0.529, 0.502),   // #8C8780
        (0.910, 0.325, 0.165),   // #E8532A
        nil,
    ]

    /// A lobe in front knocks a slightly larger hexagon out of whatever is behind it, leaving a
    /// hairline of ground between them. Below about 24pt the tones converge and this gap is the
    /// only thing keeping three shapes from reading as one.
    static let cutBleed: Double = 3

    /// Which of the two independent choices a drawing makes.
    ///
    /// They are orthogonal: `cut` is about the gaps between lobes, `colored` is about what the
    /// lobes are filled with. Small sizes want the gaps; the app icon and the landing take the
    /// colour; the app itself is monochrome (§10).
    struct Build {
        /// Hairline gaps between the lobes. On below ~24pt, off above ~64pt.
        var cut: Bool = true
        /// Grey, orange, ink instead of one ink at three tones. Never in the app.
        var colored: Bool = false

        static let icon = Build(cut: true, colored: true)
        static let window = Build(cut: true, colored: false)
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

    /// The lobes that sit in front of `index` and therefore cut into it — empty when the build
    /// has no gaps, and always empty for the front lobe, which nothing covers.
    static func cutters(of index: Int, _ build: Build) -> [(x: Double, y: Double)] {
        guard build.cut else { return [] }
        return ((index + 1)..<angles.count).map { centre($0) }
    }
}
