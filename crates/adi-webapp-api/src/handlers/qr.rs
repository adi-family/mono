//! Drawing a QR code as an SVG, so a phone can be pointed at the control panel.
//!
//! The terminal's answer to the same question lives in `adi-cli/src/qr.rs` (`docs/fleet.md` §8);
//! this is the screen's. They share the encoder and one of its choices, and nothing else — half
//! blocks and ANSI colour pairs are a way of drawing square pixels in a grid of tall cells, and a
//! browser has real pixels.
//!
//! Four things here are load-bearing:
//!
//! * **Error correction `L`**, for the reason the terminal renderer gives: the higher levels buy
//!   recovery from a *damaged* code, and a code on a screen thirty centimetres from a camera is
//!   either in frame or it is not. What they cost is versions, and every version is 4 more modules
//!   across — smaller modules for the camera at the same panel width.
//! * **An opaque white background rect**, not a transparent one. The panel has a dark theme, and a
//!   QR whose light modules are "whatever is behind it" is an *inverted* QR on half the machines
//!   that open it. Some readers cope; a phone at an angle in poor light often does not.
//! * **The quiet zone is drawn, not assumed.** A decoder finds a code by the four-module light
//!   border around it. The `viewBox` includes it, so the border survives whatever width CSS gives
//!   the element — a code sized to a container it fits exactly is a code with no quiet zone.
//! * **No XML prolog and no `width`/`height`.** This is inlined into the page, not served as a
//!   file: a `<?xml …?>` ahead of it is a parse error in HTML, and fixed pixel dimensions would
//!   fight the stylesheet that has to size it for the screen it landed on.
//!
//! One `<path>` of merged horizontal runs rather than a `<rect>` per module: a v15 code is 5929
//! modules, and the run form is about a third of the bytes and a single DOM node.

use std::fmt::Write as _;

use qrcode::{Color, EcLevel, QrCode};

/// Light modules around the code, in modules. Four is the specification's minimum.
const QUIET: usize = 4;

/// Render `data` as an `<svg>` element, ready to be inlined into the page.
///
/// The result is self-contained and carries no scripts, no external references and no text — a
/// caller can drop it into the document as-is.
///
/// # Errors
/// If `data` is too long to fit in any QR version at this error-correction level — around 2900
/// bytes. An invite is a third of that, so in practice this cannot fire.
pub(super) fn svg(data: &str) -> Result<String, String> {
    let code = QrCode::with_error_correction_level(data, EcLevel::L)
        .map_err(|e| format!("could not draw this token as a QR code: {e}"))?;
    let width = code.width();
    let modules = code.to_colors();
    let side = width + QUIET * 2;

    // Runs of dark modules, one row at a time. `x` walks past the end of each run it emits, so a
    // module is visited once and every run is maximal.
    let mut d = String::new();
    for y in 0..width {
        let mut x = 0;
        while x < width {
            if modules[y * width + x] != Color::Dark {
                x += 1;
                continue;
            }
            let start = x;
            while x < width && modules[y * width + x] == Color::Dark {
                x += 1;
            }
            let (len, px, py) = (x - start, start + QUIET, y + QUIET);
            let _ = write!(d, "M{px} {py}h{len}v1h-{len}z");
        }
    }

    Ok(format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 {side} {side}\" \
         shape-rendering=\"crispEdges\" role=\"img\" aria-label=\"Pairing invite QR code\">\
         <rect width=\"{side}\" height=\"{side}\" fill=\"#ffffff\"/>\
         <path fill=\"#000000\" d=\"{d}\"/></svg>"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One module, in pixels, when the SVG is turned back into an image. A decoder needs enough
    /// pixels to find an edge; eight is comfortable and keeps the test image small.
    const SCALE: usize = 8;

    /// Read an emitted SVG back into a module grid — the inverse of [`svg`], parsed out of the
    /// path data rather than rebuilt from the encoder, so a decoding failure here is a failure of
    /// what was actually *served*.
    ///
    /// Deliberately strict: every command is matched in full, and anything else panics. A parser
    /// that shrugged at an unexpected token would quietly agree with a renderer that had started
    /// emitting nonsense.
    fn to_modules(svg: &str) -> (usize, Vec<bool>) {
        let side: usize = svg
            .split("viewBox=\"0 0 ")
            .nth(1)
            .and_then(|rest| rest.split(' ').next())
            .expect("a viewBox")
            .parse()
            .expect("a square side");
        let d = svg
            .split(" d=\"")
            .nth(1)
            .and_then(|rest| rest.split('"').next())
            .expect("path data");

        let mut grid = vec![false; side * side];
        let mut rest = d;
        while !rest.is_empty() {
            let (x, r) = number(rest.strip_prefix('M').expect("a run starts with M"));
            let (y, r) = number(r.strip_prefix(' ').expect("x and y are separated"));
            let (len, r) = number(r.strip_prefix('h').expect("a run is h<len>"));
            let (back, r) = number(r.strip_prefix("v1h-").expect("a run closes with v1h-<len>"));
            assert_eq!(len, back, "a run that does not close on itself: {d}");
            for i in 0..len {
                grid[y * side + x + i] = true;
            }
            rest = r.strip_prefix('z').expect("a run ends with z");
        }
        (side, grid)
    }

    /// A leading run of digits, and what follows it.
    fn number(s: &str) -> (usize, &str) {
        let end = s.find(|c: char| !c.is_ascii_digit()).unwrap_or(s.len());
        (s[..end].parse().expect("a number"), &s[end..])
    }

    /// The shape of a real invite: the `adi-invite:` prefix and 380 hex characters, which is what
    /// `join::mint_invite` produces for a node carrying a relay ticket.
    fn sample_token() -> String {
        std::iter::once("adi-invite:".to_owned())
            .chain((0..380).map(|i| format!("{:x}", i % 16)))
            .collect()
    }

    /// What the panel inlines is a QR code that decodes back to the token, read by the same
    /// decoder the browser client points its camera through.
    #[test]
    fn the_rendered_svg_decodes_back_to_the_token() {
        let token = sample_token();
        let rendered = svg(&token).expect("an invite fits in a QR code");
        let (side, modules) = to_modules(&rendered);

        let mut pixels = Vec::with_capacity(side * SCALE * side * SCALE);
        for y in 0..side * SCALE {
            for x in 0..side * SCALE {
                pixels.push(u8::from(!modules[(y / SCALE) * side + (x / SCALE)]) * 255);
            }
        }

        let mut decoder = quircs::Quirc::default();
        let codes: Vec<_> = decoder
            .identify(side * SCALE, side * SCALE, &pixels)
            .collect();
        assert_eq!(codes.len(), 1, "expected exactly one code in the render");

        let data = codes[0]
            .as_ref()
            .expect("the code is extractable")
            .decode()
            .expect("the rendered code decodes");
        assert_eq!(String::from_utf8(data.payload).expect("ascii"), token);
    }

    /// The border a decoder locates the code by. Cheap to lose in a refactor and invisible when it
    /// is gone — the code still *looks* right, it just stops being found from a phone.
    #[test]
    fn the_quiet_zone_is_four_light_modules_on_every_side() {
        let (side, modules) = to_modules(&svg("adi-invite:00").expect("a short token fits"));
        for y in 0..side {
            for x in 0..side {
                let border = x < QUIET || y < QUIET || x + QUIET >= side || y + QUIET >= side;
                assert!(
                    !(border && modules[y * side + x]),
                    "a dark module at ({x}, {y}) is inside the quiet zone",
                );
            }
        }
    }

    /// The two properties that make it safe to inline: it is an element and not a document, and it
    /// carries an opaque background rather than borrowing the panel's (which is dark half the
    /// time, and would invert the code).
    #[test]
    fn the_svg_is_inlinable_and_opaque() {
        let rendered = svg(&sample_token()).expect("an invite fits in a QR code");
        assert!(rendered.starts_with("<svg "), "{}", &rendered[..40]);
        assert!(!rendered.contains("<?xml"), "an XML prolog is an HTML parse error");
        // The `<svg>` tag itself carries no pixel size — only a viewBox — so the stylesheet is
        // what decides how big the code is on the screen it landed on. (The background rect's own
        // width/height are in module units and are a different thing.)
        let open = rendered.split('>').next().expect("an opening tag");
        assert!(
            !open.contains("width=") && !open.contains("height="),
            "a fixed size on the root element fights the stylesheet: {open}"
        );
        assert!(
            rendered.contains("<rect width=") && rendered.contains("fill=\"#ffffff\""),
            "the light modules must be painted, not inherited"
        );
        // Nothing that could execute or fetch, because this string is set as inner HTML.
        for hostile in ["<script", "href", "xlink", "<image", "<foreignObject"] {
            assert!(!rendered.contains(hostile), "the SVG carries {hostile:?}");
        }
    }
}
