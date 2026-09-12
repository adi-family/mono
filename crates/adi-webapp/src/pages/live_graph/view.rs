//! The view transform: how far in the canvas is, and how far it has been dragged.
//!
//! One value, from which every pixel on screen is derived. The gestures produce it, [`paint`](super::paint)
//! consumes it, and nothing else about the page is view state.

/// How far the view may be taken out and in. Past either end there is nothing to see: a graph
/// small enough to be a texture at one end, one box filling the pane at the other.
pub(crate) const MIN_SCALE: f64 = 0.05;
pub(crate) const MAX_SCALE: f64 = 4.0;

/// How much of the stage **Fit** leaves empty around the graph, as a fraction of each side.
const FIT_MARGIN: f64 = 0.06;

/// Where the world sits on screen: how far in, and how far the stage has been dragged from the
/// middle.
///
/// The pan is measured from the *centre* of the stage rather than its top-left corner, which is
/// what makes an untouched view well defined — pan `(0, 0)` is the world origin in the middle of
/// the pane, at any size — and keeps what you are looking at in place when the pane changes width,
/// as it does whenever the store rail opens. The layout centres the graph on that origin for the
/// same reason.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) struct Viewport {
    pub(crate) scale: f64,
    pub(crate) pan_x: f64,
    pub(crate) pan_y: f64,
}

impl Viewport {
    /// The view an empty canvas opens on, and what **Fit** falls back to when there is nothing to
    /// fit.
    pub(crate) const HOME: Self = Self {
        scale: 1.0,
        pan_x: 0.0,
        pan_y: 0.0,
    };

    /// Where a world point lands on screen, in CSS pixels from the stage's top-left corner.
    pub(crate) fn screen(self, w: f64, h: f64, x: f64, y: f64) -> (f64, f64) {
        (
            w / 2.0 + x * self.scale + self.pan_x,
            h / 2.0 + y * self.scale + self.pan_y,
        )
    }

    /// [`fit`](Self::fit), but never taken out past the point where the cards stop saying anything.
    ///
    /// A machine with a thousand conversations does not go on a screen whole *and* readable. Fitting
    /// this operator's takes the scale to 26%, under [`LABEL_SCALE`](super::paint::LABEL_SCALE), and
    /// every label is dropped: a picture of the shape of the work with none of its words, which is
    /// not a thing anybody can read an answer off. Measured on that machine, no wrap setting and no
    /// card cap gets it back — at 40 cards, half of what there is, it is still only 38%.
    ///
    /// So the view opens at the readable floor with the left edge of the picture on screen, which is
    /// where the roots are and where the story starts, and the rest is a drag away. **Fit** still
    /// does what it says and takes the whole graph, labels or no labels.
    pub(crate) fn fit_readable(extent: (f64, f64, f64, f64), w: f64, h: f64, floor: f64) -> Self {
        let fitted = Self::fit(extent, w, h);
        if !floor.is_finite() || fitted.scale >= floor {
            return fitted;
        }
        let (x0, y0, _, y1) = extent;
        Self {
            scale: floor,
            // The left edge of the picture set just inside the stage, rather than its middle.
            pan_x: w.mul_add(FIT_MARGIN - 0.5, -(x0 * floor)),
            pan_y: -y0.midpoint(y1) * floor,
        }
    }

    /// The inverse: what sits under a point on screen.
    pub(crate) fn world(self, w: f64, h: f64, x: f64, y: f64) -> (f64, f64) {
        (
            (x - w / 2.0 - self.pan_x) / self.scale,
            (y - h / 2.0 - self.pan_y) / self.scale,
        )
    }

    /// Zoom by `factor` about the screen point `(x, y)`, keeping whatever is under that point
    /// under it. Solving for the pan afterwards is what makes the pointer the anchor — scaling
    /// alone would zoom about the centre and slide the thing you were looking at away.
    pub(crate) fn zoomed(self, w: f64, h: f64, x: f64, y: f64, factor: f64) -> Self {
        let (wx, wy) = self.world(w, h, x, y);
        let scale = (self.scale * factor).clamp(MIN_SCALE, MAX_SCALE);
        Self {
            scale,
            pan_x: x - w / 2.0 - wx * scale,
            pan_y: y - h / 2.0 - wy * scale,
        }
    }

    /// The view that puts all of `extent` — `(x0, y0, x1, y1)` in world units — on a stage of
    /// `w` × `h`.
    ///
    /// Never magnifies: a graph of three boxes is shown at its own size in the middle of the pane,
    /// not blown up until the boxes are the size of windows.
    pub(crate) fn fit(extent: (f64, f64, f64, f64), w: f64, h: f64) -> Self {
        let (x0, y0, x1, y1) = extent;
        let (gw, gh) = (x1 - x0, y1 - y0);
        if !(gw.is_finite() && gh.is_finite()) || gw <= 0.0 || gh <= 0.0 || w <= 0.0 || h <= 0.0 {
            return Self::HOME;
        }
        let scale = (w * (1.0 - FIT_MARGIN * 2.0) / gw)
            .min(h * (1.0 - FIT_MARGIN * 2.0) / gh)
            .clamp(MIN_SCALE, 1.0);
        // The graph's own middle, brought to the middle of the stage.
        let (cx, cy) = (x0.midpoint(x1), y0.midpoint(y1));
        Self {
            scale,
            pan_x: -cx * scale,
            pan_y: -cy * scale,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The pointer is the anchor: whatever sits under it before a zoom sits under it after, at any
    /// scale and from any pan. This is the property the whole gesture is judged by, and the one the
    /// pan solve in [`Viewport::zoomed`] exists for.
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

    /// The scale stops at both ends, and a zoom that is already clamped does not drift — the wheel
    /// keeps turning long after the limit is reached.
    #[test]
    fn the_scale_stays_within_its_limits() {
        let (w, h) = (800.0, 600.0);
        let mut v = Viewport::HOME;
        for _ in 0..200 {
            v = v.zoomed(w, h, 400.0, 300.0, 0.5);
        }
        assert!((v.scale - MIN_SCALE).abs() < 1e-9, "scale fell to {}", v.scale);
        for _ in 0..200 {
            v = v.zoomed(w, h, 400.0, 300.0, 2.0);
        }
        assert!((v.scale - MAX_SCALE).abs() < 1e-9, "scale rose to {}", v.scale);
    }

    /// Screen and world are inverses of each other. A pan that is read back wrong is a canvas that
    /// drifts under the cursor, which is the hardest kind of bug to see in a screenshot.
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

    /// A fitted view has the whole graph on the stage, with its middle in the middle.
    #[test]
    fn fit_puts_the_whole_graph_on_the_stage() {
        let (w, h) = (1000.0, 600.0);
        let extent = (-4000.0, -1500.0, 1000.0, 2500.0);
        let v = Viewport::fit(extent, w, h);
        let (x0, y0) = v.screen(w, h, extent.0, extent.1);
        let (x1, y1) = v.screen(w, h, extent.2, extent.3);
        assert!(x0 >= 0.0 && y0 >= 0.0 && x1 <= w && y1 <= h, "{x0},{y0} {x1},{y1}");
        assert!(((x0 + x1) / 2.0 - w / 2.0).abs() < 1e-6);
        assert!(((y0 + y1) / 2.0 - h / 2.0).abs() < 1e-6);
    }

    /// A graph too big to be read whole opens at the readable floor showing where it starts, rather
    /// than at a scale that drops every label and leaves a field of blank boxes.
    #[test]
    fn a_fit_nobody_asked_for_stops_where_the_labels_would_go() {
        let (w, h) = (1160.0, 690.0);
        let floor = 0.34;
        let big = (-2000.0, -1300.0, 2000.0, 1300.0);
        assert!(Viewport::fit(big, w, h).scale < floor, "fixture is not big enough to matter");

        let v = Viewport::fit_readable(big, w, h, floor);
        assert!((v.scale - floor).abs() < 1e-9, "opened at {}", v.scale);
        // The left edge of the picture sits just inside the stage — the roots are there.
        let (left, _) = v.screen(w, h, big.0, 0.0);
        assert!((left - w * FIT_MARGIN).abs() < 1e-6, "left edge lands at {left}");
        // …and the middle row is still the middle of the stage, so it opens on the picture.
        let (_, mid) = v.screen(w, h, 0.0, big.1.midpoint(big.3));
        assert!((mid - h / 2.0).abs() < 1e-6, "vertical middle lands at {mid}");

        // A graph that already fits is not touched at all: this only ever pulls a view back in.
        let small = (-100.0, -50.0, 100.0, 50.0);
        assert_eq!(Viewport::fit_readable(small, w, h, floor), Viewport::fit(small, w, h));
    }

    /// Fit is not magnification: a small graph is shown at its own size, not blown up to fill a
    /// display.
    #[test]
    fn fit_never_zooms_past_a_hundred_percent() {
        let v = Viewport::fit((-100.0, -20.0, 100.0, 20.0), 1600.0, 900.0);
        assert!((v.scale - 1.0).abs() < 1e-9, "fit zoomed to {}", v.scale);
    }

    /// Nothing to fit — an empty graph, a stage with no width yet — is the home view rather than a
    /// division by zero.
    #[test]
    fn fitting_nothing_is_the_home_view() {
        assert_eq!(Viewport::fit((0.0, 0.0, 0.0, 0.0), 800.0, 600.0), Viewport::HOME);
        assert_eq!(
            Viewport::fit((-10.0, -10.0, 10.0, 10.0), 0.0, 0.0),
            Viewport::HOME
        );
    }
}
