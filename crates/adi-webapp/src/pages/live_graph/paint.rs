//! Putting the graph on the canvas: the floor it sits on, the edges, and the boxes.
//!
//! Two coordinate systems, on purpose. The grid is drawn in **screen** pixels so a hairline can be
//! snapped to the device grid and stay one pixel at any zoom; the graph is drawn in **world** units
//! with the viewport as the canvas transform, so a node's own geometry never has to know how far in
//! the view is. Line widths inside that second pass are divided by the scale for the same reason a
//! border is 1px whatever a page's zoom: a hairline is a hairline.
//!
//! Every colour and every type size is read off the element as a CSS custom property — the canvas
//! has no stylesheet to inherit one from, and the alternative is restating values that live in
//! `design/tokens.css`.

use leptos::prelude::window;
use wasm_bindgen::JsCast;
use web_sys::{CanvasRenderingContext2d, HtmlCanvasElement};

use super::model::{Graph, Kind, Mark, NODE_H};
use super::view::Viewport;

/// World units between the closest grid lines at 100%, and the narrowest that spacing may get on
/// screen before the grid steps up to a coarser one. Below ~14px the lines read as a grey wash
/// rather than a grid.
const CELL: f64 = 24.0;
const MIN_CELL_PX: f64 = 14.0;

/// Every fifth line is the strong one — and five is also the factor the step jumps by when the grid
/// coarsens, so a step change promotes the lines that were already major instead of shifting the
/// pattern sideways.
const MAJOR: f64 = 5.0;

/// Below this scale a label is a smear rather than a word, so the boxes are drawn without one and
/// the shape of the graph is what is left. Zooming in is what brings the names back.
///
/// `pub(super)` because it is also the bar a laid-out graph is held to: [`model`](super::model)
/// asserts that fitting a real machine's graph lands above it, which is the difference between an
/// overview and a pattern of grey boxes.
pub(super) const LABEL_SCALE: f64 = 0.34;

/// …and below this one an arrowhead is a dot on every edge, which reads as noise.
const ARROW_SCALE: f64 = 0.2;

/// The radius of the dot a node wears, in world units — 6px across at 100%, the size §3 gives a
/// status dot.
const MARK_R: f64 = 3.0;

/// Space inside a box, and the width an arrowhead takes back from its target.
const PAD: f64 = 12.0;
const ARROW: f64 = 8.0;

/// The tokens this module draws with, read once per paint.
///
/// An empty string is what `getComputedStyle` answers before the stylesheet has resolved, and
/// canvas treats an invalid colour as "keep the last one" — which would paint the graph in
/// whatever was set before, or in black. So a missing token stops the paint instead (see
/// [`Palette::read`]), and the next one — data arriving, a pointer moving — draws it properly.
struct Palette {
    line: String,
    line_strong: String,
    ink: String,
    ink_2: String,
    ink_3: String,
    raise: String,
    active: String,
    hover: String,
    accent: String,
    warn: String,
    err: String,
    code: String,
    sans: String,
    mono: String,
    fs_ui_sm: f64,
    fs_small: f64,
    fs_label: f64,
}

impl Palette {
    fn read(el: &HtmlCanvasElement) -> Option<Self> {
        let style = window().get_computed_style(el).ok().flatten()?;
        let get = |name: &str| -> Option<String> {
            let v = style.get_property_value(name).ok()?.trim().to_string();
            (!v.is_empty()).then_some(v)
        };
        let px = |name: &str| -> Option<f64> {
            get(name)?.trim_end_matches("px").trim().parse::<f64>().ok()
        };
        Some(Self {
            line: get("--line")?,
            line_strong: get("--line-strong")?,
            ink: get("--ink")?,
            ink_2: get("--ink-2")?,
            ink_3: get("--ink-3")?,
            raise: get("--bg-raise")?,
            active: get("--bg-active")?,
            hover: get("--bg-hover")?,
            accent: get("--accent")?,
            warn: get("--warn")?,
            err: get("--err")?,
            code: get("--code")?,
            sans: get("--sans")?,
            mono: get("--mono")?,
            fs_ui_sm: px("--fs-ui-sm")?,
            fs_small: px("--fs-small")?,
            fs_label: px("--fs-label")?,
        })
    }

    /// The colour a node's dot is drawn in, or `None` for a node with nothing to say.
    fn mark(&self, mark: Mark) -> Option<&str> {
        match mark {
            Mark::None => None,
            Mark::Running => Some(&self.accent),
            Mark::Waiting => Some(&self.warn),
            Mark::Failed => Some(&self.err),
        }
    }
}

/// How one kind of node is drawn: its fill, its border, its text, and how round it is.
struct Style<'a> {
    fill: &'a str,
    border: &'a str,
    text: &'a str,
    size: f64,
    weight: &'a str,
    /// The family the label is set in — sans for everything a person named, mono for the one kind
    /// of node whose label is a string out of a config file (§3).
    family: &'a str,
    radius: f64,
}

impl Palette {
    fn style(&self, kind: Kind, hovered: bool) -> Style<'_> {
        let border = if hovered { &self.ink_3 } else { &self.line_strong };
        match kind {
            // The spine of the picture: the strongest box and the only one in primary ink.
            Kind::Agent => Style {
                fill: if hovered { &self.active } else { &self.raise },
                border,
                text: &self.ink,
                size: self.fs_ui_sm,
                weight: "500",
                family: &self.sans,
                radius: 6.0,
            },
            Kind::Chat => Style {
                fill: if hovered { &self.active } else { &self.hover },
                border: if hovered { &self.ink_3 } else { &self.line },
                text: &self.ink_2,
                size: self.fs_small,
                weight: "400",
                family: &self.sans,
                radius: 6.0,
            },
            // Where work came from, and the tools it may reach: pills, because neither is a step
            // in the flow — one is before it and the other is past the end of it.
            Kind::Origin => Style {
                fill: if hovered { &self.active } else { &self.raise },
                border,
                text: &self.ink_2,
                size: self.fs_small,
                weight: "500",
                family: &self.sans,
                radius: NODE_H / 2.0,
            },
            // A tool's label is its id as the agent's definition spells it — a string out of a
            // config file, which is the one thing mono is for. Kept at the smallest step so it
            // stays quiet despite the brighter ink mono carries.
            Kind::Tool => Style {
                fill: if hovered { &self.active } else { &self.hover },
                border: if hovered { &self.ink_3 } else { &self.line },
                text: &self.code,
                size: self.fs_label,
                weight: "400",
                family: &self.mono,
                radius: NODE_H / 2.0,
            },
        }
    }
}

/// Paint one frame: the whole canvas, from the graph and the viewport alone.
///
/// There is no partial redraw and no retained scene — a frame costs a few hundred rounded
/// rectangles, and a canvas that is rebuilt from its inputs cannot drift out of step with them.
// Every cast below is a pixel count or a grid-line index bounded by the size of the stage: the
// bitmap is a few thousand pixels a side, and `gap` is never under `MIN_CELL_PX`, so there are
// fewer line indices than there are pixels.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss
)]
pub(crate) fn draw(canvas: &HtmlCanvasElement, g: &Graph, v: Viewport, hover: Option<usize>) {
    let Some(ctx) = context(canvas) else { return };
    let rect = canvas.get_bounding_client_rect();
    let (w, h) = (rect.width(), rect.height());
    if w < 1.0 || h < 1.0 {
        return;
    }
    // The bitmap is sized in device pixels and the context scaled back to CSS pixels, or every line
    // is resampled and the whole picture comes out soft on a retina screen. Assigning either
    // dimension clears the canvas, so it is done only when the size actually changed.
    let dpr = window().device_pixel_ratio().max(1.0);
    let (bw, bh) = ((w * dpr).round() as u32, (h * dpr).round() as u32);
    if canvas.width() != bw {
        canvas.set_width(bw);
    }
    if canvas.height() != bh {
        canvas.set_height(bh);
    }
    let Some(ink) = Palette::read(canvas) else { return };

    let _ = ctx.set_transform(dpr, 0.0, 0.0, dpr, 0.0, 0.0);
    ctx.clear_rect(0.0, 0.0, w, h);
    grid(&ctx, &ink, v, w, h, dpr);

    // From here on the units are the graph's own. Everything drawn in screen pixels inside this
    // pass — a hairline, a dot — is divided by the scale to come out that size on the glass.
    let (ox, oy) = v.screen(w, h, 0.0, 0.0);
    let _ = ctx.set_transform(dpr * v.scale, 0.0, 0.0, dpr * v.scale, ox * dpr, oy * dpr);
    let (wx0, wy0) = v.world(w, h, 0.0, 0.0);
    let (wx1, wy1) = v.world(w, h, w, h);
    edges(&ctx, &ink, g, v, hover);
    nodes(&ctx, &ink, g, v, hover, (wx0, wy0, wx1, wy1));
}

fn context(canvas: &HtmlCanvasElement) -> Option<CanvasRenderingContext2d> {
    canvas
        .get_context("2d")
        .ok()
        .flatten()
        .and_then(|c| c.dyn_into::<CanvasRenderingContext2d>().ok())
}

/// The floor: a grid that coarsens as the view goes out, so the lines never merge into a wash.
#[allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]
fn grid(ctx: &CanvasRenderingContext2d, ink: &Palette, v: Viewport, w: f64, h: f64, dpr: f64) {
    let mut step = CELL;
    while step * v.scale < MIN_CELL_PX {
        step *= MAJOR;
    }
    while step * v.scale >= MIN_CELL_PX * MAJOR {
        step /= MAJOR;
    }
    let gap = step * v.scale;
    let (ox, oy) = v.screen(w, h, 0.0, 0.0);
    let major = MAJOR as i64;
    // A line is drawn on a whole device pixel: rounded to the device grid, then offset by half its
    // own width. Without it a hairline straddles two rows of pixels and renders as two dim ones —
    // which on a full-screen grid reads as a moiré rather than as blur.
    let snap = |p: f64| ((p * dpr).round() + dpr / 2.0) / dpr;

    ctx.set_line_width(1.0);
    for pass in [false, true] {
        ctx.set_stroke_style_str(if pass { &ink.line_strong } else { &ink.line });
        ctx.begin_path();
        for i in ((-ox / gap).ceil() as i64)..=(((w - ox) / gap).floor() as i64) {
            if (i.rem_euclid(major) == 0) != pass {
                continue;
            }
            let x = snap(ox + i as f64 * gap);
            ctx.move_to(x, 0.0);
            ctx.line_to(x, h);
        }
        for i in ((-oy / gap).ceil() as i64)..=(((h - oy) / gap).floor() as i64) {
            if (i.rem_euclid(major) == 0) != pass {
                continue;
            }
            let y = snap(oy + i as f64 * gap);
            ctx.move_to(0.0, y);
            ctx.line_to(w, y);
        }
        ctx.stroke();
    }
}

/// The edges, drawn under the boxes so a curve that passes behind one is hidden by it rather than
/// crossing its label.
///
/// Two passes: everything, then the hovered node's own edges again in a colour that reads over the
/// rest. Drawing the highlight second is what keeps it on top of a hundred quiet curves.
fn edges(
    ctx: &CanvasRenderingContext2d,
    ink: &Palette,
    g: &Graph,
    v: Viewport,
    hover: Option<usize>,
) {
    ctx.set_line_width(1.0 / v.scale);
    for pass in [false, true] {
        let colour = if pass { &ink.ink_3 } else { &ink.line };
        if pass && hover.is_none() {
            continue;
        }
        ctx.set_stroke_style_str(colour);
        ctx.set_fill_style_str(colour);
        for e in &g.edges {
            let touched = hover.is_some_and(|i| e.from == i || e.to == i);
            if touched != pass {
                continue;
            }
            let (from, to) = (&g.nodes[e.from], &g.nodes[e.to]);
            let x0 = from.x + from.w() / 2.0;
            let x1 = to.x - to.w() / 2.0 - ARROW;
            let (y0, y1) = (from.y, to.y);
            // How far the curve leaves each end horizontally. An edge that runs backwards — an
            // agent that started work in a column left of its own — bulges further, so that it
            // reads as going back rather than as a straight line through everything between.
            let reach = if x1 > x0 {
                ((x1 - x0) * 0.5).max(30.0)
            } else {
                (x0 - x1).mul_add(0.25, 60.0)
            };
            ctx.begin_path();
            ctx.move_to(x0, y0);
            ctx.bezier_curve_to(x0 + reach, y0, x1 - reach, y1, x1, y1);
            ctx.stroke();
            if v.scale >= ARROW_SCALE {
                // A head at the target end. Without it the two ends of an edge look the same, and
                // which way the work went is the whole question this page answers.
                ctx.begin_path();
                ctx.move_to(x1 + ARROW, y1);
                ctx.line_to(x1, y1 - ARROW * 0.45);
                ctx.line_to(x1, y1 + ARROW * 0.45);
                ctx.close_path();
                ctx.fill();
            }
        }
    }
}

/// The boxes, and what is written in them.
fn nodes(
    ctx: &CanvasRenderingContext2d,
    ink: &Palette,
    g: &Graph,
    v: Viewport,
    hover: Option<usize>,
    visible: (f64, f64, f64, f64),
) {
    let labels = v.scale >= LABEL_SCALE;
    ctx.set_text_baseline("middle");
    ctx.set_text_align("left");
    ctx.set_line_width(1.0 / v.scale);
    for (i, n) in g.nodes.iter().enumerate() {
        // A graph is mostly off screen at any useful zoom; a node that cannot be seen is not
        // measured, cut to fit, or drawn.
        let (w2, h2) = (n.w() / 2.0, NODE_H / 2.0);
        if n.x + w2 < visible.0 || n.x - w2 > visible.2 || n.y + h2 < visible.1 || n.y - h2 > visible.3
        {
            continue;
        }
        let s = ink.style(n.kind, hover == Some(i));
        ctx.set_fill_style_str(s.fill);
        ctx.set_stroke_style_str(s.border);
        ctx.begin_path();
        let _ = ctx.round_rect_with_f64(n.x - w2, n.y - h2, n.w(), NODE_H, s.radius);
        ctx.fill();
        ctx.stroke();

        let mut left = n.x - w2 + PAD;
        if let Some(colour) = ink.mark(n.mark) {
            ctx.set_fill_style_str(colour);
            ctx.begin_path();
            let _ = ctx.arc(left + MARK_R, n.y, MARK_R, 0.0, std::f64::consts::TAU);
            ctx.fill();
            left += MARK_R * 2.0 + 6.0;
        }
        if labels {
            ctx.set_font(&format!("{} {}px {}", s.weight, s.size, s.family));
            ctx.set_fill_style_str(s.text);
            let room = n.x + w2 - PAD - left;
            let _ = ctx.fill_text(&cut(ctx, &n.label, room), left, n.y + 0.5);
        }
    }
}

/// `label`, or as much of it as fits in `room` with an ellipsis in place of the rest.
///
/// Measured rather than counted: the labels are agent names, tool ids and the first line of
/// somebody's message, and a character budget cuts "adi-ui" and "MMMMMM" in the same place.
fn cut(ctx: &CanvasRenderingContext2d, label: &str, room: f64) -> String {
    let width = |s: &str| ctx.measure_text(s).map_or(0.0, |m| m.width());
    if room <= 0.0 {
        return String::new();
    }
    if width(label) <= room {
        return label.to_string();
    }
    let mut out = String::new();
    for ch in label.chars() {
        let mut next = out.clone();
        next.push(ch);
        if width(&format!("{next}\u{2026}")) > room {
            break;
        }
        out = next;
    }
    out.push('\u{2026}');
    out
}
