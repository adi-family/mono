//! The Live graph page: the canvas the graph will be drawn on, and nothing on it yet.
//!
//! What exists here is the *surface* — a grid you can zoom about the pointer and pan by dragging,
//! sized to the pane and redrawn when it changes. There is no graph, no fetch and no state: the
//! page talks to nothing, which is why it takes no [`State`](crate::state::State).
//!
//! The view is held as a scale and a pan measured from the middle of the stage (see [`Viewport`]),
//! and every pixel on screen is painted by [`draw`] from that one value. Nothing here animates:
//! a redraw happens because the pointer moved, the wheel turned, or the pane changed size.

use leptos::html;
use leptos::prelude::*;
use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::Closure;
use web_sys::{CanvasRenderingContext2d, HtmlCanvasElement, ResizeObserver};

/// How far the view may be taken out and in. Past either end the grid stops saying anything: all
/// wash at one end, one cell filling the pane at the other.
const MIN_SCALE: f64 = 0.1;
const MAX_SCALE: f64 = 8.0;

/// How much zoom one pixel of wheel travel buys, as a rate — the factor is `exp(-delta * this)`,
/// so a fast scroll and a slow one that covers the same distance land on the same scale, and
/// zooming out then back in returns exactly where it started.
const ZOOM_PER_PX: f64 = 0.0015;

/// A wheel event reports its delta in pixels, lines or pages (`deltaMode`); Firefox and some
/// mice send lines. This is the pixel worth of one line — Chrome's own conversion, near enough
/// that a notch feels the same across browsers.
const LINE_PX: f64 = 16.0;

/// World units between the closest grid lines at 100%, and the narrowest that spacing may get on
/// screen before the grid steps up to a coarser one. Below ~14px the lines read as a grey wash
/// rather than a grid.
const CELL: f64 = 24.0;
const MIN_CELL_PX: f64 = 14.0;

/// Every fifth line is the strong one — and five is also the factor the step jumps by when the
/// grid coarsens, so a step change promotes the lines that were already major instead of
/// shifting the pattern sideways.
const MAJOR: f64 = 5.0;

/// The origin dot's radius, in CSS pixels. It does not scale with the view: it is not a thing in
/// the world, it is the mark that says where the world's centre went.
const ORIGIN_R: f64 = 2.5;

/// Where the world sits on screen: how far in, and how far the stage has been dragged from the
/// middle.
///
/// The pan is measured from the *centre* of the stage rather than its top-left corner, which is
/// what makes an untouched view well defined — pan `(0, 0)` is the origin in the middle of the
/// pane, at any size — and keeps what you are looking at in place when the pane changes width,
/// as it does whenever the store rail opens.
#[derive(Clone, Copy, PartialEq)]
struct Viewport {
    scale: f64,
    pan_x: f64,
    pan_y: f64,
}

impl Viewport {
    /// The view **Reset view** returns to, and the one the page opens on.
    const HOME: Self = Self {
        scale: 1.0,
        pan_x: 0.0,
        pan_y: 0.0,
    };

    /// Where a world point lands on screen, in CSS pixels from the stage's top-left corner.
    fn screen(self, w: f64, h: f64, x: f64, y: f64) -> (f64, f64) {
        (
            w / 2.0 + x * self.scale + self.pan_x,
            h / 2.0 + y * self.scale + self.pan_y,
        )
    }

    /// The inverse: what sits under a point on screen.
    fn world(self, w: f64, h: f64, x: f64, y: f64) -> (f64, f64) {
        (
            (x - w / 2.0 - self.pan_x) / self.scale,
            (y - h / 2.0 - self.pan_y) / self.scale,
        )
    }

    /// Zoom by `factor` about the screen point `(x, y)`, keeping whatever is under that point
    /// under it. Solving for the pan afterwards is what makes the pointer the anchor — scaling
    /// alone would zoom about the centre and slide the thing you were looking at away.
    fn zoomed(self, w: f64, h: f64, x: f64, y: f64, factor: f64) -> Self {
        let (wx, wy) = self.world(w, h, x, y);
        let scale = (self.scale * factor).clamp(MIN_SCALE, MAX_SCALE);
        Self {
            scale,
            pan_x: x - w / 2.0 - wx * scale,
            pan_y: y - h / 2.0 - wy * scale,
        }
    }
}

/// The observer watching the stage for a size change, and the closure it calls — held together
/// because the one must be disconnected before the other is dropped.
///
/// Stored *locally*: both are JS handles, which are neither `Send` nor `Sync`, and the ordinary
/// [`StoredValue`] demands both (the same reason [`crate::voice`]'s session is).
type Resize = StoredValue<Option<(ResizeObserver, Closure<dyn FnMut()>)>, LocalStorage>;

/// The Live graph page: a head with the zoom readout and the way back to the middle, and under
/// it the canvas itself.
pub(crate) fn live_graph_view() -> AnyView {
    let view = RwSignal::new(Viewport::HOME);
    let canvas: NodeRef<html::Canvas> = NodeRef::new();
    // The last pointer position of a drag, and `None` between drags — so the move handler knows
    // whether it is panning without asking the event, which reports no button on a trackpad.
    let drag = RwSignal::new(None::<(f64, f64)>);
    // Kept apart from `drag` so the cursor's class changes twice per drag rather than on every
    // move the pointer makes.
    let panning = RwSignal::new(false);
    let watch: Resize = StoredValue::new_local(None);

    // Repaint whenever the view changes, and once when the canvas itself lands.
    Effect::new(move |_| {
        let Some(c) = canvas.get() else { return };
        draw(&c, view.get());
        if watch.with_value(Option::is_some) {
            return;
        }
        // A `resize` listener on the window would miss most of what resizes this pane: the
        // explorer and the store rail change its width with the window standing still.
        let el = c.clone();
        let cb = Closure::<dyn FnMut()>::new(move || draw(&el, view.get_untracked()));
        if let Ok(ro) = ResizeObserver::new(cb.as_ref().unchecked_ref()) {
            ro.observe(&c);
            watch.set_value(Some((ro, cb)));
        }
    });
    on_cleanup(move || {
        // Disconnected before the closure it calls is dropped, never the other way round.
        watch.update_value(|w| {
            if let Some((ro, _)) = w.take() {
                ro.disconnect();
            }
        });
    });

    view! {
        <section class="adi-panel adi-panel--fill adi-graph">
            <header class="adi-bar adi-graph__head">
                <h1 class="adi-bar__title">"Live graph"</h1>
                <span class="adi-graph__hint">"Scroll to zoom, drag to pan"</span>
                <span class="adi-spacer"></span>
                <span class="adi-graph__zoom adi-tabnums">
                    {move || format!("{:.0}%", view.get().scale * 100.0)}
                </span>
                <button class="adi-btn adi-btn--sm" type="button"
                    prop:disabled=move || view.get() == Viewport::HOME
                    on:click=move |_| view.set(Viewport::HOME)>"Reset view"</button>
            </header>

            <div class="adi-graph__stage"
                class:adi-graph__stage--panning=move || panning.get()>
                <canvas class="adi-graph__canvas" node_ref=canvas
                    aria-label="The graph canvas — empty for now"

                    on:wheel=move |ev: web_sys::WheelEvent| {
                        // The wheel *is* the zoom here, so neither the pane's scroll nor the
                        // browser's own ctrl+wheel zoom may have it. A trackpad pinch arrives as
                        // a wheel with ctrlKey set, which is why both take this one path.
                        ev.prevent_default();
                        let Some(c) = canvas.get_untracked() else { return };
                        let r = c.get_bounding_client_rect();
                        let (cx, cy) = pointer(ev.client_x(), ev.client_y());
                        let (x, y) = (cx - r.left(), cy - r.top());
                        let dy = wheel_px(&ev, r.height());
                        view.update(|v| {
                            *v = v.zoomed(r.width(), r.height(), x, y, (-dy * ZOOM_PER_PX).exp());
                        });
                    }

                    on:pointerdown=move |ev: web_sys::PointerEvent| {
                        if ev.button() != 0 {
                            return;
                        }
                        // Captured, so a drag that leaves the canvas — or the window — still
                        // reports its end here instead of leaving the page stuck mid-pan.
                        if let Some(c) = canvas.get_untracked() {
                            let _ = c.set_pointer_capture(ev.pointer_id());
                        }
                        drag.set(Some(pointer(ev.client_x(), ev.client_y())));
                        panning.set(true);
                    }

                    on:pointermove=move |ev: web_sys::PointerEvent| {
                        let Some((px, py)) = drag.get_untracked() else { return };
                        let (x, y) = pointer(ev.client_x(), ev.client_y());
                        drag.set(Some((x, y)));
                        // A pan moves the world under the pointer by exactly what the pointer
                        // moved, at any scale — the drag is in screen pixels and so is the pan.
                        view.update(|v| {
                            v.pan_x += x - px;
                            v.pan_y += y - py;
                        });
                    }

                    on:pointerup=move |_| { drag.set(None); panning.set(false); }
                    on:pointercancel=move |_| { drag.set(None); panning.set(false); }
                ></canvas>
            </div>
        </section>
    }
    .into_any()
}

/// A pointer position as the arithmetic wants it. The DOM reports whole pixels here; everything
/// downstream is in fractions of one, because the scale is.
fn pointer(x: i32, y: i32) -> (f64, f64) {
    (f64::from(x), f64::from(y))
}

/// One wheel event's travel in pixels, whatever unit it chose to report in.
fn wheel_px(ev: &web_sys::WheelEvent, stage_h: f64) -> f64 {
    match ev.delta_mode() {
        web_sys::WheelEvent::DOM_DELTA_LINE => ev.delta_y() * LINE_PX,
        web_sys::WheelEvent::DOM_DELTA_PAGE => ev.delta_y() * stage_h,
        _ => ev.delta_y(),
    }
}

/// Paint the grid for one viewport.
///
/// Every draw is complete: the bitmap is cleared and redrawn from `v` alone, so there is no
/// accumulated state to get out of step with the signal.
// Every cast below is a pixel count or a line index bounded by the size of a viewport: the
// bitmap is at most a few thousand pixels a side, and `gap` is never under `MIN_CELL_PX`, so
// there are fewer line indices than there are pixels.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss
)]
fn draw(canvas: &HtmlCanvasElement, v: Viewport) {
    let Some(ctx) = canvas
        .get_context("2d")
        .ok()
        .flatten()
        .and_then(|c| c.dyn_into::<CanvasRenderingContext2d>().ok())
    else {
        return;
    };
    let rect = canvas.get_bounding_client_rect();
    let (w, h) = (rect.width(), rect.height());
    if w < 1.0 || h < 1.0 {
        return;
    }

    // The bitmap is sized in device pixels and the context scaled back to CSS pixels, or every
    // line is resampled and the grid comes out soft on a retina screen. Assigning either
    // dimension clears the canvas, so it is done only when the size actually changed — and the
    // transform is set after, because the assignment resets that too.
    let dpr = window().device_pixel_ratio().max(1.0);
    let (bw, bh) = ((w * dpr).round() as u32, (h * dpr).round() as u32);
    if canvas.width() != bw {
        canvas.set_width(bw);
    }
    if canvas.height() != bh {
        canvas.set_height(bh);
    }
    let _ = ctx.set_transform(dpr, 0.0, 0.0, dpr, 0.0, 0.0);
    ctx.clear_rect(0.0, 0.0, w, h);

    // The tokens, read off the element rather than written here (DESIGN.md: never restate a
    // value). An empty answer means the stylesheet has not resolved yet; drawing that as a
    // colour would put a black grid on the page, so the pass is simply skipped.
    let line = token(canvas, "--line");
    let strong = token(canvas, "--line-strong");
    if line.is_empty() || strong.is_empty() {
        return;
    }

    // Coarsen the step until the lines are far enough apart to read as a grid. `MAJOR` is the
    // factor, so whichever step we land on, its every-fifth line was a line of the step before.
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
    // A line is drawn on a whole device pixel: rounded to the device grid, then offset by half
    // its own width. Without it a hairline straddles two rows of pixels and renders as two dim
    // ones — which on a full-screen grid reads as a moiré rather than as blur.
    let snap = |p: f64| ((p * dpr).round() + dpr / 2.0) / dpr;

    ctx.set_line_width(1.0);
    for pass in [false, true] {
        ctx.set_stroke_style_str(if pass { &strong } else { &line });
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

    // The origin, so panning has something to have moved away from and **Reset view** has
    // somewhere visible to come back to.
    if (0.0..=w).contains(&ox) && (0.0..=h).contains(&oy) {
        ctx.set_fill_style_str(&token(canvas, "--ink-3"));
        ctx.begin_path();
        let _ = ctx.arc(ox, oy, ORIGIN_R, 0.0, std::f64::consts::TAU);
        ctx.fill();
    }
}

/// The computed value of a CSS custom property on this element — the way a token reaches the
/// canvas, which has no stylesheet of its own to inherit one from.
fn token(canvas: &HtmlCanvasElement, name: &str) -> String {
    window()
        .get_computed_style(canvas)
        .ok()
        .flatten()
        .and_then(|s| s.get_property_value(name).ok())
        .map(|v| v.trim().to_string())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The pointer is the anchor: whatever sits under it before a zoom sits under it after, at
    /// any scale and from any pan. This is the property the whole gesture is judged by, and the
    /// one the pan solve in [`Viewport::zoomed`] exists for.
    #[test]
    fn zooming_keeps_the_point_under_the_pointer() {
        let (w, h) = (1200.0, 800.0);
        let start = Viewport {
            scale: 0.7,
            pan_x: -140.0,
            pan_y: 96.0,
        };
        for (x, y) in [(0.0, 0.0), (640.0, 410.0), (1200.0, 800.0)] {
            for factor in [0.5, 0.9, 1.0, 1.4, 3.0] {
                let before = start.world(w, h, x, y);
                let after = start.zoomed(w, h, x, y, factor).world(w, h, x, y);
                assert!(
                    (before.0 - after.0).abs() < 1e-9 && (before.1 - after.1).abs() < 1e-9,
                    "zooming by {factor} at ({x}, {y}) moved {before:?} to {after:?}"
                );
            }
        }
    }

    /// The scale stops at both ends, and a zoom that is already clamped does not drift — the
    /// wheel keeps turning long after the limit is reached.
    #[test]
    fn the_scale_stays_within_its_limits() {
        let (w, h) = (800.0, 600.0);
        let mut v = Viewport::HOME;
        for _ in 0..200 {
            v = v.zoomed(w, h, 400.0, 300.0, 0.5);
        }
        assert!(
            (v.scale - MIN_SCALE).abs() < 1e-9,
            "scale fell to {}",
            v.scale
        );
        for _ in 0..200 {
            v = v.zoomed(w, h, 400.0, 300.0, 2.0);
        }
        assert!(
            (v.scale - MAX_SCALE).abs() < 1e-9,
            "scale rose to {}",
            v.scale
        );
    }

    /// Screen and world are inverses of each other. A pan that is read back wrong is a canvas
    /// that drifts under the cursor, which is the hardest kind of bug to see in a screenshot.
    #[test]
    fn screen_and_world_round_trip() {
        let v = Viewport {
            scale: 2.5,
            pan_x: 33.0,
            pan_y: -71.0,
        };
        let (w, h) = (1024.0, 768.0);
        let (sx, sy) = v.screen(w, h, -18.0, 42.0);
        let (wx, wy) = v.world(w, h, sx, sy);
        assert!((wx + 18.0).abs() < 1e-9 && (wy - 42.0).abs() < 1e-9);
    }
}
