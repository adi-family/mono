//! The adi mark — **Trefoil**: three hexagons at 120°, painted back to front, weak to strong.
//!
//! This is a fourth renderer of one drawing, and it is a fourth because nothing this client can
//! depend on already draws it. `adi-ui`'s `Mark` is the obvious reuse, except that the whole
//! component library is styled with Tailwind utilities resolved against the `adi-css` design
//! tokens — a stylesheet this page does not have and would have to ship a copy of, to draw one
//! logo. `adi-webapp/assets/mark.svg` is a file rather than markup, so it lands as a second
//! network request in a client whose premise is that it fetches nothing from anywhere.
//!
//! So the geometry is restated, and [`the_lobes_match_the_spec`](tests::the_lobes_match_the_spec)
//! re-derives every path below from the five numbers all four copies are built from — the same
//! guard `adi-hive/src/notfound.rs` puts on its copy, and the reason a drift here is a failing
//! test rather than two logos that quietly stop matching.
//!
//! Two things about the drawing are load-bearing, and both are mistakes somebody already made
//! (`adi-ui/src/mark.rs` has the long form):
//!
//! * **The lobe in front is the strongest.** Run the tones the other way and the shape nearest the
//!   reader is the faintest, which lays a wash of pale ink over the whole mark.
//! * **A gap between lobes is a real subtraction.** Each lobe is masked by the ones in front of
//!   it, so the hairline shows the ground. Even-odd on one combined path is the obvious shortcut
//!   and draws a rim of the wrong colour around the lobe in front instead.

use leptos::prelude::*;

/// Everything inside the `<svg>`, as markup.
///
/// Set with `inner_html` rather than written as elements in the `view!`, which is what `adi-ui`
/// does with its copy and for the same reason: `linearGradient` and `maskUnits` are camelCase in
/// SVG, and markup is the one form in which they cannot be normalised into something a browser
/// ignores.
///
/// The ink is `currentColor` at 52% and 100%, so the mark takes the colour of whatever it sits in
/// and follows both themes without a second drawing. The middle lobe is the one named colour —
/// `#FA5019`, the same accent the Dock icon, the disk image and the front door's error pages use.
const MARKUP: &str = concat!(
    r##"<defs>"##,
    // Specular then shade, handing over at the same offset so there is no band between them. Laid
    // *over* the lobe and never graded into its own alpha: grading the alpha turns the front lobe
    // translucent at its foot and lets whatever is behind show through.
    r##"<linearGradient id="adi-mark-gloss" x1="0" y1="0" x2="0" y2="1">"##,
    r##"<stop offset="0" stop-color="#ffffff" stop-opacity=".38"/>"##,
    r##"<stop offset=".55" stop-color="#ffffff" stop-opacity="0"/>"##,
    r##"<stop offset=".55" stop-color="#000000" stop-opacity="0"/>"##,
    r##"<stop offset="1" stop-color="#000000" stop-opacity=".28"/>"##,
    r##"</linearGradient>"##,
    r##"<linearGradient id="adi-mark-accent" x1="0" y1="0" x2="0" y2="1">"##,
    r##"<stop offset="0" stop-color="#FF8A4A"/>"##,
    r##"<stop offset=".55" stop-color="#FA5019"/>"##,
    r##"<stop offset="1" stop-color="#D8380A"/>"##,
    r##"</linearGradient>"##,
    // White keeps, black erases: each mask holds the lobes that sit in front of the one it cuts,
    // grown by the bleed that leaves the hairline.
    r##"<mask id="adi-mark-cut-0" maskUnits="userSpaceOnUse" x="0" y="0" width="200" height="200">"##,
    r##"<rect width="200" height="200" fill="#fff"/>"##,
    r##"<path fill="#000" d="M127.71 65.00 L178.81 94.50 L178.81 153.50 L127.71 183.00 L76.62 153.50 L76.62 94.50 Z"/>"##,
    r##"<path fill="#000" d="M100.00 17.00 L151.10 46.50 L151.10 105.50 L100.00 135.00 L48.90 105.50 L48.90 46.50 Z"/>"##,
    r##"</mask>"##,
    r##"<mask id="adi-mark-cut-1" maskUnits="userSpaceOnUse" x="0" y="0" width="200" height="200">"##,
    r##"<rect width="200" height="200" fill="#fff"/>"##,
    r##"<path fill="#000" d="M100.00 17.00 L151.10 46.50 L151.10 105.50 L100.00 135.00 L48.90 105.50 L48.90 46.50 Z"/>"##,
    r##"</mask>"##,
    r##"</defs>"##,
    r##"<g mask="url(#adi-mark-cut-0)">"##,
    r##"<path fill="currentColor" fill-opacity="0.52" d="M72.29 68.00 L120.78 96.00 L120.78 152.00 L72.29 180.00 L23.79 152.00 L23.79 96.00 Z"/>"##,
    r##"<path fill="url(#adi-mark-gloss)" d="M72.29 68.00 L120.78 96.00 L120.78 152.00 L72.29 180.00 L23.79 152.00 L23.79 96.00 Z"/>"##,
    r##"</g>"##,
    r##"<g mask="url(#adi-mark-cut-1)">"##,
    r##"<path fill="url(#adi-mark-accent)" d="M127.71 68.00 L176.21 96.00 L176.21 152.00 L127.71 180.00 L79.22 152.00 L79.22 96.00 Z"/>"##,
    r##"<path fill="url(#adi-mark-gloss)" d="M127.71 68.00 L176.21 96.00 L176.21 152.00 L127.71 180.00 L79.22 152.00 L79.22 96.00 Z"/>"##,
    r##"</g>"##,
    r##"<g>"##,
    r##"<path fill="currentColor" d="M100.00 20.00 L148.50 48.00 L148.50 104.00 L100.00 132.00 L51.50 104.00 L51.50 48.00 Z"/>"##,
    r##"<path fill="url(#adi-mark-gloss)" d="M100.00 20.00 L148.50 48.00 L148.50 104.00 L100.00 132.00 L51.50 104.00 L51.50 48.00 Z"/>"##,
    r##"</g>"##,
);

/// The mark, at whatever size the box around it gives it.
///
/// Hidden from assistive technology: every place it is drawn pairs it with the wordmark beside it,
/// and a screen reader announcing "adi" twice is worse than not drawing it at all.
#[component]
pub fn Mark() -> impl IntoView {
    view! {
        <svg class="mark" viewBox="0 0 200 200" fill="none" aria-hidden="true" inner_html=MARKUP></svg>
    }
}

#[cfg(test)]
mod tests {
    use super::MARKUP;

    /// The design box, the lobe, and where the three sit in it. Every path in [`MARKUP`] is
    /// derived from these, and so is every other copy of the mark in this tree.
    const BOX: f64 = 200.0;
    const RADIUS: f64 = 56.0;
    const OFFSET: f64 = 32.0;
    const NUDGE: f64 = 8.0;
    /// A lobe in front knocks a slightly larger hexagon out of what is behind it, and that is what
    /// leaves a hairline of ground between them.
    const CUT_BLEED: f64 = 3.0;
    /// Back, middle, front — paint order, and the order the paths are declared in.
    const ANGLES: [f64; 3] = [150.0, 30.0, -90.0];

    fn hexagon(angle: f64, radius: f64) -> String {
        let (cx, cy) = (
            BOX / 2.0 + OFFSET * angle.to_radians().cos(),
            BOX / 2.0 + OFFSET * angle.to_radians().sin() + NUDGE,
        );
        let corners: Vec<String> = (0..6)
            .map(|i| {
                let a = (f64::from(i) * 60.0 - 90.0).to_radians();
                format!("{:.2} {:.2}", cx + radius * a.cos(), cy + radius * a.sin())
            })
            .collect();
        format!("M{} Z", corners.join(" L"))
    }

    /// The guard `adi-hive/src/notfound.rs` and `adi-webapp/src/pwa.rs` put on their own copies:
    /// re-derive the path data, so this drawing cannot drift from the other three silently.
    #[test]
    fn the_lobes_match_the_spec() {
        for angle in ANGLES {
            let lobe = hexagon(angle, RADIUS);
            assert!(MARKUP.contains(&lobe), "the lobe at {angle}°: {lobe}");
        }
        // Only the two lobes that sit in front of something are ever cut out of anything.
        for angle in [ANGLES[1], ANGLES[2]] {
            let cutter = hexagon(angle, RADIUS + CUT_BLEED);
            assert!(MARKUP.contains(&cutter), "the cutter at {angle}°: {cutter}");
        }
    }

    /// An earlier mark had the tones the other way round and laid a wash of the palest shape over
    /// everything — invisible on a dark ground, and ruinous on a light one.
    #[test]
    fn the_front_lobe_is_the_strongest() {
        let faded = MARKUP.find("fill-opacity=\"0.52\"").expect("the back lobe");
        let full = MARKUP
            .rfind("<path fill=\"currentColor\" d=")
            .expect("the front lobe");
        assert!(faded < full, "the lobes must be declared back to front");
    }
}
