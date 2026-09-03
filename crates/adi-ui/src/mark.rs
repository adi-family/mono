//! The mark — three hexagons, one in front (`design/DESIGN.md` §10).
//!
//! This is the web's copy of the geometry that `apps/macos/Sources/Trefoil.swift` holds for the
//! Swift renderers, `crates/adi-mesh-client/src/mark.rs` holds for the mesh client and
//! `crates/adi-hive/src/notfound.rs` holds as SVG literals for the pages the front door serves.
//! It is generated from the numbers rather than written out, and `the_lobes_match_the_spec`
//! re-derives the literals the other copies agree on and fails if this one drifts.
//!
//! Monochrome by default: `currentColor`, with the two back shapes at 52% and 74%. The coloured
//! build — grey, orange, ink — exists for the app icon and the landing, and nowhere in the app.
//! No gloss, no gradient, no shadow, no animation: it is not a mascot.
//!
//! Two things about the drawing are load-bearing, and both are mistakes somebody already made:
//!
//! **Paint order is the design.** Back to front runs weak to strong. Run the tones the other way
//! and the lobe visually in front is the faintest, which on white lays a wash of pale grey over
//! the whole mark.
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
/// The ink each lobe is filled at, in paint order (§10: 52%, 74%, 100%).
const TONES: [&str; 3] = ["0.52", "0.74", "1"];
/// The coloured build's fills, in paint order: grey, orange, and the ink of wherever it sits.
/// Literal on purpose — this is an icon, and an icon that changed with the page would be a
/// different icon. The landing's light orange is the one exception, and the landing draws its
/// own copy.
const COLORED: [&str; 3] = ["#8C8780", "#E8532A", "currentColor"];

/// The centre of the drawn shape, as a percentage of the box — **not** the middle of the
/// viewBox, which sits the vertical nudge above it. What a `transform-origin` for the whole mark
/// has to be, on the one surface (the app icon in motion) that is allowed to turn it.
pub const SPIN_ORIGIN: &str = "50% 54%";

/// Which drawing of the mark this is. One drawing cannot serve a 16px favicon and a 104px
/// splash, so there are two, and the size decides.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MarkVariant {
    /// Hairline gaps between the lobes. The default, and what anything icon-sized wants.
    #[default]
    Cut,
    /// No gaps — the lobes are told apart by their tones alone, which is enough above ~64px.
    Solid,
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

/// One lobe: its fill, inside whatever mask cuts it.
fn lobe(index: usize, variant: MarkVariant, colored: bool) -> String {
    let path = lobe_path(index);
    let name = ["back", "mid", "front"][index];
    // Only the lobes with something in front of them are cut; the front lobe never is.
    let mask = if variant == MarkVariant::Cut && index + 1 < ANGLES.len() {
        format!(r#" mask="url(#adi-mark-cut-{index})""#)
    } else {
        String::new()
    };
    let fill = if colored {
        format!(r#"fill="{}""#, COLORED[index])
    } else {
        format!(r#"fill="currentColor" fill-opacity="{}""#, TONES[index])
    };
    format!(
        r#"<g class="adi-mark__lobe adi-mark__lobe--{name}"{mask}><path {fill} d="{path}"/></g>"#
    )
}

/// Everything inside the `<svg>`, as markup.
fn markup(variant: MarkVariant, colored: bool) -> String {
    let mut defs = String::new();
    if variant == MarkVariant::Cut {
        for index in 0..ANGLES.len() - 1 {
            defs.push_str(&cut_mask(index));
        }
    }
    let lobes: String = (0..ANGLES.len())
        .map(|index| lobe(index, variant, colored))
        .collect();
    format!(r#"<defs>{defs}</defs><g class="adi-mark__lobes">{lobes}</g>"#)
}

/// The mark, at whatever size `class` gives it. 18px beside the wordmark in a bar; never under
/// 16.
///
/// It never names its own ink — every lobe is `currentColor` at one of the tones, which is what
/// lets the same drawing sit on the page, in a bar, or inside a control and pick up that
/// control's colour.
///
/// Hidden from assistive technology, because every place it is used pairs it with the wordmark,
/// and a screen reader announcing "adi" twice is worse than not drawing it at all.
///
/// ```ignore
/// <Mark class="size-4.5"/>
/// ```
#[component]
pub fn Mark(
    #[prop(optional)] variant: MarkVariant,
    /// The coloured build — grey, orange, ink. For the app icon and the landing only; in the
    /// app the mark is monochrome (§10), and an orange lobe on every screen is a second orange.
    #[prop(optional)]
    accent: bool,
    #[prop(optional, into)] class: String,
) -> impl IntoView {
    view! {
        <svg
            class=merge("adi-mark shrink-0", class)
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
    /// `apps/macos/Sources/Trefoil.swift` computes them. Three languages draw this mark and none
    /// of them may drift.
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

    /// Back to front is weak to strong, and nothing may reorder it.
    #[test]
    fn the_front_lobe_is_the_strongest() {
        let drawn = markup(MarkVariant::Solid, false);
        let at = |needle: &str| drawn.find(needle).unwrap_or_else(|| panic!("no {needle}"));
        assert!(at("0.52") < at("0.74"), "lobes must be drawn back to front");
        assert!(at("0.74") < at(r#"fill-opacity="1""#));
    }

    /// The front lobe has nothing in front of it, so nothing cuts it — and the solid build keeps
    /// every lobe and loses only the hairlines.
    #[test]
    fn only_the_covered_lobes_are_cut() {
        let cut = markup(MarkVariant::Cut, true);
        assert!(cut.contains("adi-mark-cut-0") && cut.contains("adi-mark-cut-1"));
        assert!(!cut.contains("adi-mark-cut-2"));
        assert!(!markup(MarkVariant::Solid, true).contains("mask"));
    }

    /// §10: no gradient, no gloss, and the app's build names no colour of its own.
    #[test]
    fn the_mark_is_flat() {
        let mono = markup(MarkVariant::Cut, false);
        assert!(!mono.contains("Gradient"), "{mono}");
        // The masks are black and white; nothing else in the app's build may name a colour.
        for named in ["#E8532A", "#FA5019", "#8C8780"] {
            assert!(!mono.contains(named), "{named} in the monochrome mark");
        }
        let colored = markup(MarkVariant::Cut, true);
        assert!(colored.contains("#E8532A") && !colored.contains("Gradient"));
    }
}
