//! The new UI's windows, and the desk they sit on.
//!
//! Every window has an address. Visiting it opens that window — or brings it to the front — and
//! leaves every other window open where it was, the way a second app on macOS leaves the first
//! one's windows alone. The address bar names the front window; closing a window closes that one
//! and nothing else, and the address moves to whichever window is in front after it (`/` once the
//! desk is clear).
//!
//! Which windows are open, in which order, and where each was dropped, are remembered per device
//! in `localStorage` — the same place as the wallpaper, for the same reason.

use leptos::{ev, html, prelude::*};
use serde::{Deserialize, Serialize};
use wasm_bindgen::JsCast;

use crate::{routing, ui};

/// The open windows, back to front.
const OPEN_KEY: &str = "adi-new-ui-windows";

/// How much of a window must stay on screen, so a drag can never lose it: this much of its
/// width, and all of its title bar.
const KEEP_VISIBLE: f64 = 96.0;
const TITLEBAR: f64 = 40.0;

/// The stacking order the windows start from — under the top bar and the island (35), as a
/// window slides under macOS's menu bar and dock, and under the palette (40).
const Z_BASE: usize = 10;

/// A window the new UI can open.
#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum Win {
    Settings,
    About,
}

impl Win {
    pub(super) const ALL: [Self; 2] = [Self::Settings, Self::About];

    /// The address that opens it.
    pub(super) fn path(self) -> &'static str {
        match self {
            Self::Settings => "/settings",
            Self::About => "/about",
        }
    }

    pub(super) fn title(self) -> &'static str {
        match self {
            Self::Settings => "Settings",
            Self::About => "About adi",
        }
    }

    /// Where its position is kept.
    fn pos_key(self) -> &'static str {
        match self {
            Self::Settings => "adi-new-ui-window-settings",
            Self::About => "adi-new-ui-window-about",
        }
    }

    /// Its width in CSS pixels. About is a narrow card, as macOS draws its own.
    fn width(self) -> u16 {
        match self {
            Self::Settings => 480,
            Self::About => 320,
        }
    }

    fn from_path(path: &str) -> Option<Self> {
        let path = path.trim_end_matches('/');
        Self::ALL.into_iter().find(|w| w.path() == path)
    }
}

/// The open windows and their order.
#[derive(Clone, Copy)]
pub(super) struct Desk {
    /// Back to front: the last one is in front, and is the one the address bar names.
    stack: RwSignal<Vec<Win>>,
}

impl Desk {
    /// The windows this device left open, plus whichever one the address names, in front.
    pub(super) fn load() -> Self {
        let mut stack: Vec<Win> = ui::storage()
            .and_then(|s| s.get_item(OPEN_KEY).ok().flatten())
            .and_then(|json| serde_json::from_str(&json).ok())
            .unwrap_or_default();
        stack.dedup();
        let desk = Self {
            stack: RwSignal::new(stack),
        };
        desk.arrive(&routing::current_path());
        desk
    }

    /// The address changed under us — a load, back, forward. Raise what it names; close nothing,
    /// and write no history, because the browser already has.
    pub(super) fn arrive(self, path: &str) {
        if let Some(w) = Win::from_path(path) {
            self.raise(w);
        }
    }

    /// Open a window, or bring it forward — a link followed, so it is a new history entry.
    pub(super) fn open(self, w: Win) {
        self.raise(w);
        self.show_address(true);
    }

    /// Bring a window forward because it was clicked. Not a place visited, so the address is
    /// replaced rather than pushed: back does not step through every click between windows.
    pub(super) fn focus(self, w: Win) {
        if self.stack.with_untracked(|s| s.last() != Some(&w)) {
            self.raise(w);
            self.show_address(false);
        }
    }

    /// Close one window and only that one.
    pub(super) fn close(self, w: Win) {
        self.stack.update(|s| s.retain(|x| *x != w));
        self.save();
        self.show_address(false);
    }

    /// Close whichever window is in front — what Escape does.
    pub(super) fn close_front(self) {
        if let Some(w) = self.stack.with_untracked(|s| s.last().copied()) {
            self.close(w);
        }
    }

    pub(super) fn is_open(self, w: Win) -> bool {
        self.stack.with(|s| s.contains(&w))
    }

    pub(super) fn is_front(self, w: Win) -> bool {
        self.stack.with(|s| s.last() == Some(&w))
    }

    fn z(self, w: Win) -> usize {
        Z_BASE
            + self
                .stack
                .with(|s| s.iter().position(|x| *x == w).unwrap_or(0))
    }

    fn raise(self, w: Win) {
        self.stack.update(|s| {
            s.retain(|x| *x != w);
            s.push(w);
        });
        self.save();
    }

    fn save(self) {
        if let (Some(s), Ok(json)) = (
            ui::storage(),
            serde_json::to_string(&self.stack.get_untracked()),
        ) {
            let _ = s.set_item(OPEN_KEY, &json);
        }
    }

    /// Point the address bar at the front window, or at `/` when there is none.
    fn show_address(self, push: bool) {
        let to = self
            .stack
            .with_untracked(|s| s.last().map_or("/", |w| w.path()));
        if routing::current_path() != to {
            if push {
                routing::push_state(to);
            } else {
                routing::replace_state(to);
            }
        }
    }
}

/// A window's frame: a macOS title bar with its traffic lights, dragged to move it, over a body
/// that scrolls on its own so the title bar — and the way out in it — never scrolls away.
#[component]
pub(super) fn Frame(
    win: Win,
    desk: Desk,
    /// Draw in the light token set, to sit on a light wallpaper.
    #[prop(into)]
    light: Signal<bool>,
    children: Children,
) -> impl IntoView {
    // `None` until it is first dragged: the stylesheet centres it, which stays centred through
    // a resize in a way a stored pixel position would not.
    let pos = RwSignal::new(load_pos(win));
    // Where in the window the pointer took hold, while a drag is on.
    let grip = StoredValue::new(None::<(f64, f64)>);
    let frame: NodeRef<html::Div> = NodeRef::new();
    // Where it sat in the stack when it opened. A window opened over others steps down and right
    // by that many places, as macOS cascades new windows, so none opens exactly over another and
    // hides its title bar. Read once: a window must not jump each time the order changes.
    let cascade = desk
        .stack
        .with_untracked(|s| s.iter().position(|x| *x == win).unwrap_or(0));

    let on_down = move |ev: ev::PointerEvent| {
        // The traffic lights sit in the title bar; pressing one is a click, not a drag.
        let on_button = ev
            .target()
            .and_then(|t| t.dyn_into::<web_sys::Element>().ok())
            .and_then(|t| t.closest("button").ok().flatten())
            .is_some();
        if on_button || ev.button() != 0 {
            return;
        }
        let (Some(frame), Some(bar)) = (
            frame.get(),
            ev.current_target()
                .and_then(|t| t.dyn_into::<web_sys::Element>().ok()),
        ) else {
            return;
        };
        let r = frame.get_bounding_client_rect();
        grip.set_value(Some((
            f64::from(ev.client_x()) - r.left(),
            f64::from(ev.client_y()) - r.top(),
        )));
        pos.set(Some((r.left(), r.top())));
        // Captured, so a fast drag that outruns the bar keeps moving the window.
        let _ = bar.set_pointer_capture(ev.pointer_id());
        ev.prevent_default();
    };
    let on_move = move |ev: ev::PointerEvent| {
        let (Some((gx, gy)), Some(frame)) = (grip.get_value(), frame.get()) else {
            return;
        };
        let x = f64::from(ev.client_x()) - gx;
        let y = f64::from(ev.client_y()) - gy;
        pos.set(Some(clamp(x, y, frame.offset_width().into())));
    };
    let on_up = move |_: ev::PointerEvent| {
        if grip.get_value().is_some() {
            grip.set_value(None);
            if let (Some(s), Some((x, y))) = (ui::storage(), pos.get_untracked()) {
                let _ = s.set_item(win.pos_key(), &format!("{x:.0},{y:.0}"));
            }
        }
    };

    view! {
        <div
            node_ref=frame
            class="adi-new-win"
            class:light=move || light.get()
            class:is-placed=move || pos.get().is_some()
            class:is-front=move || desk.is_front(win)
            style=move || {
                let at = pos
                    .get()
                    .map(|(x, y)| format!("left: {x:.0}px; top: {y:.0}px; "))
                    .unwrap_or_default();
                format!(
                    "{at}z-index: {}; --cascade: {cascade}; --win-w: {}px",
                    desk.z(win),
                    win.width(),
                )
            }
            role="dialog"
            aria-label=win.title()
            // Any press inside a window brings it forward, as a click on a macOS window does.
            on:pointerdown=move |_| desk.focus(win)
        >
            <header
                class="adi-new-win__bar"
                on:pointerdown=on_down
                on:pointermove=on_move
                on:pointerup=on_up
                on:pointercancel=on_up
            >
                // macOS's three lights. Only close does anything yet — nothing here minimises or
                // zooms — so the other two are drawn disabled, the way macOS draws them on a
                // window that cannot.
                <div class="adi-new-win__lights">
                    <button
                        class="adi-new-win__light adi-new-win__light--close"
                        type="button"
                        aria-label=format!("Close {}", win.title().to_lowercase())
                        title="Close"
                        on:click=move |_| desk.close(win)
                    ></button>
                    <span class="adi-new-win__light" aria-hidden="true"></span>
                    <span class="adi-new-win__light" aria-hidden="true"></span>
                </div>
                <h1 class="adi-new-win__title">{win.title()}</h1>
            </header>
            <div class="adi-new-win__body">{children()}</div>
        </div>
    }
}

/// Keep a window of this width with enough of it on screen to take hold of again.
fn clamp(x: f64, y: f64, width: f64) -> (f64, f64) {
    let (vw, vh) = viewport();
    (
        x.clamp(KEEP_VISIBLE - width, (vw - KEEP_VISIBLE).max(0.0)),
        y.clamp(0.0, (vh - TITLEBAR).max(0.0)),
    )
}

fn viewport() -> (f64, f64) {
    let w = window();
    let px = |v: Result<wasm_bindgen::JsValue, _>| v.ok().and_then(|v| v.as_f64()).unwrap_or(0.0);
    (px(w.inner_width()), px(w.inner_height()))
}

/// The saved position, pulled back on screen — the window may have been left on a larger one.
fn load_pos(win: Win) -> Option<(f64, f64)> {
    let saved = ui::storage()?.get_item(win.pos_key()).ok()??;
    let (x, y) = saved.split_once(',')?;
    let (x, y) = (x.parse().ok()?, y.parse().ok()?);
    // The width is not known before the window is drawn; the narrowest it is drawn at stands in.
    Some(clamp(x, y, KEEP_VISIBLE * 2.0))
}
