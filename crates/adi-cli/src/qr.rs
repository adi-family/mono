//! Drawing a QR code with terminal characters, so a phone can be pointed at a laptop.
//!
//! There is exactly one caller — `mesh invite` (`docs/fleet.md` §8) — and one shape of payload: a
//! pairing token of a few hundred characters that somebody would otherwise have to retype onto a
//! phone. The whole module is the answer to "how does that string cross the last thirty
//! centimetres".
//!
//! Three choices here are load-bearing, and none of them is obvious from the code:
//!
//! * **Half-blocks, two modules per character cell.** A terminal cell is about twice as tall as it
//!   is wide, so one module per cell would draw a QR stretched 2:1 and no decoder would find its
//!   finder patterns. `▀`/`▄`/`█` put two vertical modules in one cell, which is square again.
//! * **The ANSI colours are not decoration — they are the code.** Half-blocks drawn with no colour
//!   inherit the terminal's own palette, and on a dark theme that is an *inverted* QR: light
//!   modules dark, dark modules light. Some readers cope; a phone camera at an angle in bad light
//!   often does not. So every line is emitted with an explicit black-on-white pair, which is what
//!   `qrencode -t ANSIUTF8` does and for the same reason. `NO_COLOR` is deliberately not honoured:
//!   dropping the colours here does not produce a plainer QR, it produces an unreliable one.
//! * **Error correction `L`.** The higher levels buy recovery from a damaged code, and a code on a
//!   screen thirty centimetres from a camera is not damaged — it is either in frame or it is not.
//!   What they cost is versions, and every version is 4 more modules across: at `M` this token
//!   needs a QR two versions larger, which is a taller block of terminal *and* smaller modules for
//!   the camera. `L` is the right trade for a screen-to-screen scan.
//!
//! The quiet zone is the fourth thing and it is not a choice: a decoder locates a code by the
//! four-module light border around it, and a QR printed flush against a terminal's edge — or
//! against a dark theme — frequently will not be found at all.

use qrcode::{Color, EcLevel, QrCode};

/// Light modules around the code, in modules. Four is the specification's minimum.
const QUIET: usize = 4;

/// Black ink on a white background, held for one line.
const INK: &str = "\x1b[30;107m";
/// Back to the terminal's own colours, so the line after the code is the operator's again.
const RESET: &str = "\x1b[0m";

/// A drawn code, and the window it needs.
///
/// The size travels with the text because nothing downstream can recover it: the lines carry ANSI
/// escapes, so their `len()` is not their width, and a code wider than the window wraps into
/// something no camera will read. An invite comes out at 85 columns, which is wider than the
/// 80-column window a terminal still opens at by default — so this is not a hypothetical.
#[derive(Debug)]
pub(crate) struct Rendered {
    /// The lines, each already coloured and reset, ending in a newline.
    pub text: String,
    /// Terminal columns: one per module, quiet zone included.
    pub columns: usize,
    /// Terminal rows, at two modules to a row.
    pub rows: usize,
}

/// Render `data` as a block of lines that a camera can read, each already coloured and reset.
///
/// # Errors
/// If `data` is too long to fit in any QR version at this error-correction level — around 2900
/// bytes. An invite is a third of that, so in practice this cannot fire.
pub(crate) fn terminal(data: &str) -> Result<Rendered, String> {
    let code = QrCode::with_error_correction_level(data, EcLevel::L)
        .map_err(|e| format!("could not draw this token as a QR code: {e}"))?;
    let width = code.width();
    let modules = code.to_colors();
    let side = width + QUIET * 2;

    // Reads the padded grid, so the quiet zone needs no separate loop: everything outside the code
    // proper is light, which is exactly what a quiet zone is.
    let dark = |x: usize, y: usize| -> bool {
        let (Some(cx), Some(cy)) = (x.checked_sub(QUIET), y.checked_sub(QUIET)) else {
            return false;
        };
        cx < width && cy < width && modules[cy * width + cx] == Color::Dark
    };

    let mut out = String::with_capacity(side * side / 2);
    for row in (0..side).step_by(2) {
        out.push_str(INK);
        for x in 0..side {
            // `side` is always odd — every QR version has an odd module count and the quiet zone
            // adds eight — so the last row has no lower half. `dark` answers false off the grid,
            // which draws it as the light border it should be.
            out.push(match (dark(x, row), dark(x, row + 1)) {
                (false, false) => ' ',
                (true, false) => '▀',
                (false, true) => '▄',
                (true, true) => '█',
            });
        }
        out.push_str(RESET);
        out.push('\n');
    }
    Ok(Rendered {
        text: out,
        columns: side,
        rows: side.div_ceil(2),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One module, in pixels, when the rendered text is turned back into an image. A decoder needs
    /// enough pixels to find an edge; eight is comfortable and keeps the test image small.
    const SCALE: usize = 8;

    /// Read a rendered block back into a module grid — the inverse of [`terminal`], character by
    /// character, so a decoding failure is a failure of what was *printed* and not of a bitmap
    /// built some other way.
    fn to_modules(rendered: &str) -> (usize, Vec<bool>) {
        let mut rows: Vec<Vec<bool>> = Vec::new();
        for line in rendered.lines() {
            let cells = line
                .strip_prefix(INK)
                .and_then(|l| l.strip_suffix(RESET))
                .expect("every line is coloured and reset");
            let mut upper = Vec::new();
            let mut lower = Vec::new();
            for ch in cells.chars() {
                let (u, l) = match ch {
                    ' ' => (false, false),
                    '▀' => (true, false),
                    '▄' => (false, true),
                    '█' => (true, true),
                    other => panic!("unexpected character {other:?} in a rendered QR"),
                };
                upper.push(u);
                lower.push(l);
            }
            rows.push(upper);
            rows.push(lower);
        }
        let width = rows[0].len();
        assert!(rows.iter().all(|r| r.len() == width), "ragged rows");
        (width, rows.concat())
    }

    /// The characters `mesh invite` prints are a QR code that decodes to the token, read by the
    /// same decoder the browser client points its camera through.
    #[test]
    fn what_is_printed_decodes_back_to_the_token() {
        // The shape of a real invite: the `adi-invite:` prefix and 380 hex characters, which is
        // the length `join::mint_invite` produces for a node carrying a relay ticket.
        let token: String = std::iter::once("adi-invite:".to_owned())
            .chain((0..380).map(|i| format!("{:x}", i % 16)))
            .collect();

        let rendered = terminal(&token).expect("an invite fits in a QR code");
        let (width, modules) = to_modules(&rendered.text);
        let height = modules.len() / width;
        assert_eq!((width, rendered.rows), (rendered.columns, height / 2));

        let mut pixels = Vec::with_capacity(width * SCALE * height * SCALE);
        for y in 0..height * SCALE {
            for x in 0..width * SCALE {
                pixels.push(u8::from(!modules[(y / SCALE) * width + (x / SCALE)]) * 255);
            }
        }

        let mut decoder = quircs::Quirc::default();
        let codes: Vec<_> = decoder
            .identify(width * SCALE, height * SCALE, &pixels)
            .collect();
        assert_eq!(codes.len(), 1, "expected exactly one code in the render");

        let data = codes[0]
            .as_ref()
            .expect("the code is extractable")
            .decode()
            .expect("the printed code decodes");
        assert_eq!(String::from_utf8(data.payload).expect("ascii"), token);
    }

    /// The border a decoder locates the code by. Cheap to lose in a refactor and invisible when it
    /// is gone — the code still *looks* right, it just stops being found from a phone.
    #[test]
    fn the_quiet_zone_is_four_light_modules_on_every_side() {
        let rendered = terminal("adi-invite:00").expect("a short token fits");
        let (width, modules) = to_modules(&rendered.text);
        let height = modules.len() / width;

        // The grid is square, so `width` bounds both axes. `height` is one greater — the last
        // character row carries a lower half the grid does not have — and that phantom row is
        // border too, which is what makes it safe to print.
        for y in 0..height {
            for x in 0..width {
                let border = x < QUIET || y < QUIET || x + QUIET >= width || y + QUIET >= width;
                assert!(
                    !(border && modules[y * width + x]),
                    "a dark module at ({x}, {y}) is inside the quiet zone",
                );
            }
        }
    }
}
