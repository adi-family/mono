//! The mark — **Trefoil**: three hexagons at 120°, painted back to front, weak to strong.
//!
//! This is the web's copy of the geometry that `apps/macos/Sources/Trefoil.swift` holds for the
//! Swift renderers and `crates/adi-hive/src/notfound.rs` holds as SVG literals for the pages the
//! front door serves. It is generated from the numbers rather than written out, and
//! `the_lobes_match_the_spec` re-derives the literals the other two agree on and fails if this
//! one drifts from them.
//!
//! Three things about the drawing are load-bearing, and every one of them is a mistake somebody
//! already made:
//!
//! **Paint order is the design.** Back to front runs weak to strong (52% / 74% / 100%). Run the
//! tones the other way and the lobe visually in front is the faintest, which on white lays a
//! wash of pale grey over the whole mark.
//!
//! **The tone is on the lobe's fill, never on the group.** Fading the group fades the gloss with
//! it, and — worse, on the front lobe — turns the shape itself translucent so whatever is behind
//! shows through its foot. The one build that *does* set group opacity is [`MarkVariant::Glass`],
//! where lobes mixing is the point.
//!
//! **A gap between lobes is a real subtraction.** [`MarkVariant::Cut`] masks each lobe with the
//! ones in front of it, so the hairline shows the ground. Even-odd on a combined path is the
//! obvious shortcut and draws a rim of the wrong colour around the lobe in front instead.

use std::fmt::Write as _;

use leptos::prelude::*;

use crate::merge;

/// The design box every coordinate below is expressed in.
const BOX: f64 = 200.0;
const LOBE_RADIUS: f64 = 56.0;
/// How far each lobe's centre sits from the middle of the box.
const CENTRE_OFFSET: f64 = 32.0;
/// Back, middle, front — in paint order. Front is the top lobe.
const ANGLES: [f64; 3] = [150.0, 30.0, -90.0];
/// Two lobes sit low and one sits high, so the three centres average to the middle of the box
/// while the *drawn* shape does not: it spans y 12…172, which is 8 units high. Applied here,
/// once, rather than re-derived by every consumer.
const VERTICAL_NUDGE: f64 = 8.0;
/// A lobe in front knocks a slightly larger hexagon out of what is behind it, which is what
/// leaves a hairline of ground between them. Below about 24px the tones converge and this gap is
/// the only thing keeping three shapes from reading as one.
const CUT_BLEED: f64 = 3.0;
/// The ink each lobe is filled at, in paint order.
///
/// Shallow at the bottom on purpose: the front lobe carries the form at full strength, so the two
/// behind only have to be told apart from each other and from the ground.
const TONES: [f64; 3] = [0.52, 0.74, 1.0];
/// The same, for [`MarkVariant::Glass`], where the numbers are group opacities and the lobes mix.
const GLASS_TONES: [f64; 3] = [0.42, 0.60, 0.88];

/// The centre of the drawn shape, as a percentage of the box — **not** the middle of the
/// viewBox, which sits the vertical nudge above it.
///
/// Only rotation cares, and it cares completely: turned about this point the three lobes map onto
/// each other exactly, so a third of a turn lands the mark back on itself. Turned about the
/// middle of the viewBox the nudge orbits and it does not.
pub const SPIN_ORIGIN: &str = "50% 54%";

/// Which drawing of the mark this is. One drawing cannot serve a 16px favicon and a 168px error
/// page, so there are three, and the size decides.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MarkVariant {
    /// Hairline gaps between the lobes. The default, and what anything icon-sized wants.
    #[default]
    Cut,
    /// No gaps — the lobes are told apart by their tones alone, which is enough above ~64px.
    Solid,
    /// The lobes mix rather than stack: richer above ~96px, muddy below it.
    Glass,
}

/// The centre of lobe `index`, in box coordinates, with y running downward.
fn centre(index: usize) -> (f64, f64) {
    let radians = ANGLES[index].to_radians();
    (
        BOX / 2.0 + CENTRE_OFFSET * radians.cos(),
        BOX / 2.0 + CENTRE_OFFSET * radians.sin() + VERTICAL_NUDGE,
    )
}

/// A pointy-top hexagon, clockwise from the top, as an SVG path.
fn hexagon((cx, cy): (f64, f64), radius: f64) -> String {
    let corners: Vec<String> = (0..6)
        .map(|corner: i32| {
            let radians = (f64::from(corner) * 60.0 - 90.0).to_radians();
            format!(
                "{:.2} {:.2}",
                cx + radius * radians.cos(),
                cy + radius * radians.sin()
            )
        })
        .collect();
    format!("M{} Z", corners.join(" L"))
}

/// Lobe `index` as an SVG path, in the mark's own 200×200 box.
///
/// Public because the mark is drawn outside this crate too — the control panel's pre-wasm splash
/// and its icon files are markup a browser reads before any of this code runs — and the test that
/// pins those copies to this one needs the same numbers this component draws from.
#[must_use]
pub fn lobe_path(index: usize) -> String {
    hexagon(centre(index), LOBE_RADIUS)
}

/// The specular over the top of a lobe and the shade under its bottom, handing over at the same
/// stop so there is no band between them.
///
/// Laid *over* the lobe, never as a ramp in the lobe's own alpha. Left in `objectBoundingBox`
/// units, which is what lets one gradient serve all three lobes: each `<path>` maps it to its own
/// box.
const GLOSS: &str = concat!(
    r##"<linearGradient id="adi-mark-gloss" x1="0" y1="0" x2="0" y2="1">"##,
    r##"<stop offset="0" stop-color="#ffffff" stop-opacity=".38"/>"##,
    r##"<stop offset=".55" stop-color="#ffffff" stop-opacity="0"/>"##,
    r##"<stop offset=".55" stop-color="#000000" stop-opacity="0"/>"##,
    r##"<stop offset="1" stop-color="#000000" stop-opacity=".28"/>"##,
    r##"</linearGradient>"##,
);

/// The accent, lit. Three stops rather than two, so `#FA5019` itself stays present in the surface
/// instead of only the two ends of a ramp.
///
/// Written out rather than mixed from `--accent`: this is the one place the mark names a colour,
/// and it names the same one on every ground and in both themes. An app icon that changed with
/// the page theme would be a different app icon.
const ACCENT: &str = concat!(
    r##"<linearGradient id="adi-mark-accent" x1="0" y1="0" x2="0" y2="1">"##,
    r##"<stop offset="0" stop-color="#FF8A4A"/>"##,
    r##"<stop offset=".55" stop-color="#FA5019"/>"##,
    r##"<stop offset="1" stop-color="#D8380A"/>"##,
    r##"</linearGradient>"##,
);

/// The mask that cuts the lobes in front of `index` out of it. White keeps, black erases.
fn cut_mask(index: usize) -> String {
    let mut mask = format!(
        r#"<mask id="adi-mark-cut-{index}" maskUnits="userSpaceOnUse" x="0" y="0" width="200" height="200">"#
    );
    mask.push_str(r##"<rect width="200" height="200" fill="#fff"/>"##);
    for front in index + 1..ANGLES.len() {
        let cutter = hexagon(centre(front), LOBE_RADIUS + CUT_BLEED);
        let _ = write!(mask, r##"<path fill="#000" d="{cutter}"/>"##);
    }
    mask.push_str("</mask>");
    mask
}

/// One lobe: its fill, then the gloss over it, both inside whatever mask cuts it.
fn lobe(index: usize, variant: MarkVariant, accent: bool) -> String {
    let path = lobe_path(index);
    let name = ["back", "mid", "front"][index];
    // Only the lobes with something in front of them are cut; the front lobe never is.
    let mask = if variant == MarkVariant::Cut && index + 1 < ANGLES.len() {
        format!(r#" mask="url(#adi-mark-cut-{index})""#)
    } else {
        String::new()
    };
    let (group_opacity, fill) = match (variant, accent && index == 1) {
        // The accent lobe is the gradient at full strength: a tone under it would wash out the
        // one colour in the mark.
        (_, true) => (String::new(), r#"fill="url(#adi-mark-accent)""#.to_string()),
        (MarkVariant::Glass, false) => (
            format!(r#" opacity="{}""#, GLASS_TONES[index]),
            r#"fill="currentColor""#.to_string(),
        ),
        (_, false) => (
            String::new(),
            format!(r#"fill="currentColor" fill-opacity="{}""#, TONES[index]),
        ),
    };

    let mut group = format!(
        r#"<g class="adi-mark__lobe adi-mark__lobe--{name}"{group_opacity}{mask}>"#
    );
    let _ = write!(group, r#"<path {fill} d="{path}"/>"#);
    let _ = write!(group, r#"<path fill="url(#adi-mark-gloss)" d="{path}"/>"#);
    group.push_str("</g>");
    group
}

/// Everything inside the `<svg>`, as markup.
fn markup(variant: MarkVariant, accent: bool) -> String {
    let mut defs = String::from(GLOSS);
    if accent {
        defs.push_str(ACCENT);
    }
    if variant == MarkVariant::Cut {
        for index in 0..ANGLES.len() - 1 {
            defs.push_str(&cut_mask(index));
        }
    }
    let lobes: String = (0..ANGLES.len())
        .map(|index| lobe(index, variant, accent))
        .collect();
    // The lobes are wrapped rather than dropped straight in, so CSS animating the mark has one
    // element to turn: turning each lobe on its own turns it about its own centre.
    format!(r#"<defs>{defs}</defs><g class="adi-mark__lobes">{lobes}</g>"#)
}

/// The mark, at whatever size `class` gives it.
///
/// It never names its own ink — every lobe is `currentColor` at one of the tones, which is what
/// lets the same drawing sit on white, on black, on an image, or inside a control and pick up
/// that control's state.
///
/// Hidden from assistive technology, because every place it is used pairs it with the wordmark,
/// and a screen reader announcing "adi" twice is worse than not drawing it at all.
///
/// ```ignore
/// <Mark accent=true class="size-4.5"/>
/// ```
#[component]
pub fn Mark(
    #[prop(optional)] variant: MarkVariant,
    /// The middle lobe takes the accent instead of ink. Never on an accent-coloured ground —
    /// the lobe disappears into it.
    #[prop(optional)]
    accent: bool,
    #[prop(optional, into)] class: String,
) -> impl IntoView {
    view! {
        <svg
            // `overflow-visible` because a caller that animates the lobes moves them past the
            // viewBox, and an svg viewport clips by default.
            class=merge("adi-mark shrink-0 overflow-visible", class)
            viewBox="0 0 200 200"
            fill="none"
            aria-hidden="true"
            inner_html=markup(variant, accent)
        ></svg>
    }
}

#[cfg(test)]
mod tests {
    use super::{MarkVariant, lobe_path, markup};

    /// The three lobes, exactly as `crates/adi-hive/src/notfound.rs` writes them and as
    /// `apps/macos/Sources/Trefoil.swift` computes them. This is the whole point of generating
    /// the module: three languages draw this mark and none of them may drift.
    const SPEC: [&str; 3] = [
        "M72.29 68.00 L120.78 96.00 L120.78 152.00 L72.29 180.00 L23.79 152.00 L23.79 96.00 Z",
        "M127.71 68.00 L176.21 96.00 L176.21 152.00 L127.71 180.00 L79.22 152.00 L79.22 96.00 Z",
        "M100.00 20.00 L148.50 48.00 L148.50 104.00 L100.00 132.00 L51.50 104.00 L51.50 48.00 Z",
    ];

    #[test]
    fn the_lobes_match_the_spec() {
        for (index, expected) in SPEC.iter().enumerate() {
            assert_eq!(&lobe_path(index), expected, "lobe {index} has drifted");
        }
    }

    /// Back to front is weak to strong, and nothing may reorder it — an earlier mark ran the
    /// tones the other way and laid a wash of the palest shape over everything.
    #[test]
    fn the_front_lobe_is_the_strongest() {
        let drawn = markup(MarkVariant::Solid, false);
        let at = |needle: &str| drawn.find(needle).unwrap_or_else(|| panic!("no {needle}"));
        assert!(at("0.52") < at("0.74"), "lobes must be drawn back to front");
        assert!(at("0.74") < at(r#"fill-opacity="1""#));
    }

    /// The front lobe has nothing in front of it, so nothing cuts it — and the builds without
    /// gaps keep every lobe and lose only the hairlines.
    #[test]
    fn only_the_covered_lobes_are_cut() {
        let cut = markup(MarkVariant::Cut, true);
        assert!(cut.contains("adi-mark-cut-0") && cut.contains("adi-mark-cut-1"));
        assert!(!cut.contains("adi-mark-cut-2"));
        assert!(!markup(MarkVariant::Solid, true).contains("mask"));
    }
}

