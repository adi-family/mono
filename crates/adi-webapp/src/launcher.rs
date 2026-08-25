//! The floating mark, and the menu behind it.
//!
//! The root screens used to wear a bar across the top whose whole job was to hold five
//! controls and a wordmark. A bar is a poor container for that: it costs a strip of every
//! viewport forever, it puts the mark where the sessions rail wants to start, and the
//! controls in it are ones a reader touches a few times a day.
//!
//! So the bar is gone and the mark floats instead — one small island the reader can drag
//! anywhere and that stays where it was put, on this browser, across visits. Clicking it
//! opens the menu; so does `⌘K`. Everything the bar carried is a row in there, alongside
//! whatever else is worth reaching from a keyboard: every dashboard this machine can open,
//! here or on a paired node, the way into the control panel, the theme.
//!
//! **A drag is not a click.** The mark is both a handle and a button, which is only safe
//! because the press has to travel [`DRAG_SLOP`] pixels before it stops being the second
//! one — otherwise every reader who nudged the mark by two pixels while clicking it would
//! find that nothing happened.
//!
//! The two are read from *different events*, though, and deliberately: dragging is pointer
//! events, opening is `click`. A button that opened on `pointerup` would be one no keyboard
//! could press, because `Enter` and `Space` fire `click` and nothing else. So a drag that
//! actually moved marks the click it is about to produce to be thrown away, and every other
//! click — pointer, `Enter`, `Space`, a screen reader's activation — opens the menu.

use leptos::html;
use leptos::{ev, prelude::*};

use adi_ui::Modal;

use crate::icons;
use crate::ui::storage;

/// Where the mark's position is kept between visits: `"<left>,<top>"` in CSS pixels, in this
/// browser's `localStorage`. Per browser rather than per machine on purpose — where a
/// floating control should sit is a fact about the screen it is being read on.
const POS_KEY: &str = "adi-launcher-pos";

/// How far a press has to travel before it counts as a drag rather than a click.
///
/// Four pixels, because a mouse click is rarely perfectly still and a touch never is. Below
/// this the pointer's movement is discarded entirely, so the mark does not creep.
const DRAG_SLOP: f64 = 4.0;

/// How close to the edge of the viewport the mark may be dropped. It is clamped to this on
/// every drag *and* on every resize — a mark parked against the right edge of a wide window
/// would otherwise be outside a narrow one, with no way to get it back.
const EDGE: f64 = 8.0;

/// The mark's own size, used only to keep it on screen — a deliberate over-estimate of the
/// ~51×32 the chip actually measures. Reading the element would be exact, but [`clamp`] also
/// runs at construction, before there is an element to read; erring wide only ever holds the
/// mark a few pixels further inside the edge than it had to be.
const MARK_W: f64 = 56.0;
const MARK_H: f64 = 32.0;

/// The floating mark and everything the menu behind it needs to remember.
///
/// Built once per mounted screen and passed to [`launcher`]. It is `Copy`, so the branch that
/// draws the chat and the branch that draws the wizard can hold the same one — which is the
/// point: the menu does not close, move or forget its position when the screen under it
/// swaps.
#[derive(Clone, Copy)]
pub(crate) struct Launcher {
    /// Whether the menu is open. Public because [`Modal`] closes itself through it, and
    /// because a row that navigates within this document has to shut it on the way out.
    open: RwSignal<bool>,
    /// The mark's top-left in CSS pixels. `None` means "still where it started" — the
    /// bottom-left corner — which is drawn from the bottom rather than the top, so it needs
    /// no measurement before the first paint.
    pos: RwSignal<Option<(f64, f64)>>,
    /// What has been typed into the filter.
    query: RwSignal<String>,
    /// Which row `Enter` would run, as an index into the *filtered* list.
    cursor: RwSignal<usize>,
    /// The press in progress, if any. See [`Drag`].
    drag: RwSignal<Option<Drag>>,
    /// Whether the click that a finished drag is about to produce should be thrown away. See
    /// [`launcher`] for why the two are separate events in the first place.
    swallow: RwSignal<bool>,
}

/// A press on the mark, from `pointerdown` until it is released.
#[derive(Clone, Copy)]
struct Drag {
    /// Where the pointer went down, in client coordinates.
    start: (f64, f64),
    /// Where the mark's top-left was at that moment — read from the element rather than from
    /// [`Launcher::pos`], so a mark still sitting in its default corner drags from where it
    /// visibly is instead of jumping to the origin.
    from: (f64, f64),
    /// Whether the pointer has travelled [`DRAG_SLOP`] yet. Until it has, the release is a
    /// click.
    moved: bool,
}

impl Launcher {
    /// Start one, restoring the mark to wherever it was last dropped.
    pub(crate) fn new() -> Self {
        Self {
            open: RwSignal::new(false),
            pos: RwSignal::new(load_pos()),
            query: RwSignal::new(String::new()),
            cursor: RwSignal::new(0),
            drag: RwSignal::new(None),
            swallow: RwSignal::new(false),
        }
    }

    /// End the press in progress, handing back what it was. `None` when there wasn't one,
    /// which is what a stray release arriving here looks like from the inside.
    fn end_drag(self) -> Option<Drag> {
        let d = self.drag.get_untracked();
        if d.is_some() {
            self.drag.set(None);
        }
        d
    }

    /// Open the menu from a clean slate: no filter, first row selected.
    fn show(self) {
        self.query.set(String::new());
        self.cursor.set(0);
        self.open.set(true);
    }
}

/// One row of the menu.
///
/// Built fresh every time the list is drawn, so a row can say something that is only true
/// right now — which version is published, which dashboards are up.
pub(crate) struct Action {
    /// What the row says. Also what the filter matches on, along with the hint.
    label: String,
    /// The dim note down the right: where the row goes, or what it is.
    hint: String,
    icon: icons::Icon,
    run: Callback<()>,
}

impl Action {
    /// A row that does something on this page.
    pub(crate) fn new(
        label: impl Into<String>,
        hint: impl Into<String>,
        icon: icons::Icon,
        run: impl Fn() + Send + Sync + 'static,
    ) -> Self {
        Self {
            label: label.into(),
            hint: hint.into(),
            icon,
            run: Callback::new(move |()| run()),
        }
    }

    /// A row that leaves for another document. `/extended` and a dashboard are both their own
    /// origins-worth of page, so this is a navigation and not a route change.
    pub(crate) fn link(
        label: impl Into<String>,
        hint: impl Into<String>,
        icon: icons::Icon,
        href: impl Into<String>,
    ) -> Self {
        let href = href.into();
        Self::new(label, hint, icon, move || {
            let _ = window().location().set_href(&href);
        })
    }

    /// A row that opens somewhere in a new tab — a dashboard, which is its own origin and
    /// which the reader is stepping *to* rather than *away* from the chat.
    pub(crate) fn tab(
        label: impl Into<String>,
        hint: impl Into<String>,
        icon: icons::Icon,
        href: impl Into<String>,
    ) -> Self {
        let href = href.into();
        Self::new(label, hint, icon, move || {
            let _ = window().open_with_url_and_target(&href, "_blank");
        })
    }

    /// Whether the typed filter, already lowercased, matches this row.
    fn matches(&self, needle: &str) -> bool {
        needle.is_empty()
            || self.label.to_lowercase().contains(needle)
            || self.hint.to_lowercase().contains(needle)
    }
}

/// The mark and the menu, as one thing to drop into a screen.
///
/// `actions` is called every time the list is drawn rather than taken once, so the rows track
/// whatever signals they read — a dashboard that comes up mid-session appears in the menu
/// without anything having to invalidate it.
pub(crate) fn launcher(
    l: Launcher,
    actions: impl Fn() -> Vec<Action> + Copy + Send + Sync + 'static,
) -> impl IntoView {
    let mark: NodeRef<html::Button> = NodeRef::new();

    // `⌘K` from anywhere on the page. `code`, not `key`: with a modifier held, a layout that
    // puts something else on that key reports *that*, and the menu must not depend on the
    // keyboard. Toggles rather than opens, so the same press that opened it closes it.
    let keys = window_event_listener(ev::keydown, move |ev| {
        if ev.code() != "KeyK" || !(ev.meta_key() || ev.ctrl_key()) || ev.alt_key() || ev.shift_key()
        {
            return;
        }
        ev.prevent_default();
        if l.open.get_untracked() {
            l.open.set(false);
        } else {
            l.show();
        }
    });
    on_cleanup(move || keys.remove());

    // A window that got smaller must not take the mark with it. Clamping only what was
    // actually placed leaves a mark still in its default corner alone, which is already
    // anchored to that corner and follows the resize on its own.
    let resize = window_event_listener(ev::resize, move |_| {
        if let Some(p) = l.pos.get_untracked() {
            l.pos.set(Some(clamp(p)));
        }
    });
    on_cleanup(move || resize.remove());

    view! {
        <button
            node_ref=mark
            type="button"
            // `touch-none` so a drag on a touchscreen moves the mark instead of scrolling the
            // chat underneath it.
            class="adi-ui-type island fixed z-40 flex h-8 cursor-grab touch-none select-none \
                   items-center bg-card px-2.5 font-mono text-sub font-semibold \
                   tracking-[-0.03em] text-ink active:cursor-grabbing \
                   focus-visible:outline-2 focus-visible:outline-offset-2 \
                   focus-visible:outline-accent"
            style=move || match l.pos.get() {
                Some((x, y)) => format!("left:{x}px;top:{y}px"),
                // The corner it starts in, stated as a corner rather than as coordinates —
                // no measurement, and it survives a resize by itself.
                None => format!("left:{EDGE}px;bottom:{EDGE}px"),
            }
            title=move || format!("Menu — {}K, or drag to move", mod_glyph())
            aria-label="Open the menu"
            aria-haspopup="dialog"
            aria-keyshortcuts="Meta+K Control+K"
            on:pointerdown=move |ev: web_sys::PointerEvent| {
                let Some(el) = mark.get_untracked() else { return };
                // Capture, so the rest of the drag arrives here even when the pointer
                // outruns a 62px target — which, on a fast throw, it always does.
                let _ = el.set_pointer_capture(ev.pointer_id());
                let r = el.get_bounding_client_rect();
                // A fresh press owns its own click. Clearing here is also what stops a drag
                // that somehow ended without one from eating the *next* press.
                l.swallow.set(false);
                l.drag.set(Some(Drag {
                    start: (f64::from(ev.client_x()), f64::from(ev.client_y())),
                    from: (r.left(), r.top()),
                    moved: false,
                }));
            }
            on:pointermove=move |ev: web_sys::PointerEvent| {
                let Some(d) = l.drag.get_untracked() else { return };
                let dx = f64::from(ev.client_x()) - d.start.0;
                let dy = f64::from(ev.client_y()) - d.start.1;
                if !d.moved && dx.hypot(dy) < DRAG_SLOP {
                    return;
                }
                l.drag.set(Some(Drag { moved: true, ..d }));
                l.pos.set(Some(clamp((d.from.0 + dx, d.from.1 + dy))));
            }
            on:pointerup=move |ev: web_sys::PointerEvent| {
                let Some(d) = l.end_drag() else { return };
                if let Some(el) = mark.get_untracked() {
                    let _ = el.release_pointer_capture(ev.pointer_id());
                }
                if d.moved {
                    save_pos(l.pos.get_untracked());
                    l.swallow.set(true);
                }
            }
            // A press that ends by being cancelled (a system gesture, a lost pointer) still
            // has to put back what it moved, or the mark keeps following the next one.
            on:pointercancel=move |_| {
                if l.end_drag().is_some_and(|d| d.moved) {
                    save_pos(l.pos.get_untracked());
                    l.swallow.set(true);
                }
            }
            // Opening is on `click` and not on the release above, because `click` is also
            // what `Enter` and `Space` fire on a focused button: hang the menu off the
            // pointer and it becomes a control no keyboard can reach.
            on:click=move |_| {
                if l.swallow.get_untracked() {
                    l.swallow.set(false);
                } else {
                    l.show();
                }
            }
        >
            "adi"<span class="text-accent">"."</span>
        </button>

        {menu(l, actions)}
    }
}

/// The menu itself: a filter, and the rows that survive it.
fn menu(
    l: Launcher,
    actions: impl Fn() -> Vec<Action> + Copy + Send + Sync + 'static,
) -> impl IntoView {
    let field: NodeRef<html::Input> = NodeRef::new();
    // Focus the filter as it opens, so `⌘K` and then typing is one movement. On the ref
    // landing rather than on `open`, because the node does not exist until the dialog is up.
    Effect::new(move |_| {
        if l.open.get()
            && let Some(el) = field.get()
        {
            let _ = el.focus();
        }
    });

    // What Enter would run, and what the arrows move over: the filtered list, with the cursor
    // pulled back into it. Kept in one place so the row rendering and the key handling can
    // never disagree about which row is selected.
    let shown = move || {
        let needle = l.query.get().to_lowercase();
        let rows: Vec<Action> = actions()
            .into_iter()
            .filter(|a| a.matches(&needle))
            .collect();
        let at = l.cursor.get().min(rows.len().saturating_sub(1));
        (rows, at)
    };

    view! {
        <Modal open=l.open title="Menu" width="max-w-lg">
            <div class="flex flex-col gap-3">
                <input
                    node_ref=field
                    class="h-8 w-full rounded-sm border border-frame bg-canvas px-2.5 \
                           text-row text-ink placeholder:text-placeholder \
                           focus-visible:border-accent focus-visible:outline-none"
                    type="text"
                    placeholder="Search actions…"
                    autocomplete="off"
                    prop:value=move || l.query.get()
                    on:input=move |ev| {
                        l.query.set(event_target_value(&ev));
                        // A new filter is a new list; leaving the cursor where it was would
                        // aim Enter at whatever happens to have shuffled under it.
                        l.cursor.set(0);
                    }
                    on:keydown=move |ev: ev::KeyboardEvent| {
                        let (rows, at) = shown();
                        match ev.key().as_str() {
                            "ArrowDown" => {
                                ev.prevent_default();
                                if !rows.is_empty() {
                                    l.cursor.set((at + 1) % rows.len());
                                }
                            }
                            "ArrowUp" => {
                                ev.prevent_default();
                                if !rows.is_empty() {
                                    l.cursor.set((at + rows.len() - 1) % rows.len());
                                }
                            }
                            "Enter" => {
                                ev.prevent_default();
                                if let Some(row) = rows.get(at) {
                                    l.open.set(false);
                                    row.run.run(());
                                }
                            }
                            _ => {}
                        }
                    }
                />
                <div class="-mx-1 flex max-h-[52vh] flex-col gap-px overflow-y-auto px-1">
                    {move || {
                        let (rows, at) = shown();
                        if rows.is_empty() {
                            return view! {
                                <p class="px-2 py-6 text-center text-mini text-fainter">
                                    "Nothing matches."
                                </p>
                            }
                            .into_any();
                        }
                        rows.into_iter()
                            .enumerate()
                            .map(|(i, row)| action_row(l, i, i == at, row))
                            .collect::<Vec<_>>()
                            .into_any()
                    }}
                </div>
            </div>
        </Modal>
    }
}

/// One row, drawn.
fn action_row(l: Launcher, i: usize, selected: bool, row: Action) -> AnyView {
    let run = row.run;
    view! {
        <button
            type="button"
            class="flex w-full shrink-0 cursor-pointer items-center gap-2.5 rounded-sm px-2 \
                   py-1.5 text-left text-row hover:bg-selected"
            class:bg-selected=selected
            // The pointer moving over a row is the reader saying which one they mean, so it
            // takes the cursor with it — otherwise Enter and the pointer aim at two
            // different rows at once.
            on:pointerenter=move |_| l.cursor.set(i)
            on:click=move |_| {
                l.open.set(false);
                run.run(());
            }
        >
            <svg
                class="size-3.5 shrink-0 text-meta"
                viewBox="0 0 16 16"
                fill="none"
                stroke="currentColor"
                stroke-width="1.5"
                stroke-linecap="round"
                stroke-linejoin="round"
                aria-hidden="true"
                inner_html=row.icon.path()
            ></svg>
            <span class="min-w-0 flex-1 truncate text-ink">{row.label}</span>
            <span class="shrink-0 truncate text-mini text-fainter">{row.hint}</span>
        </button>
    }
    .into_any()
}

/// The modifier this platform writes its shortcuts with.
///
/// A guess from the user agent, which is the only thing a browser still offers — and a wrong
/// one costs a hint that names the wrong key, not a shortcut that fails: [`launcher`] answers
/// to `⌘K` and `Ctrl+K` on every platform either way.
pub(crate) fn mod_glyph() -> &'static str {
    let mac = window()
        .navigator()
        .user_agent()
        .is_ok_and(|ua| ua.contains("Mac"));
    if mac { "\u{2318}" } else { "Ctrl+" }
}

/// Hold a point far enough inside the viewport that the mark stays reachable.
fn clamp((x, y): (f64, f64)) -> (f64, f64) {
    let w = window().inner_width().ok().and_then(|v| v.as_f64());
    let h = window().inner_height().ok().and_then(|v| v.as_f64());
    // `max(EDGE)` on the ceiling as well as the floor: on a viewport narrower than the mark
    // the two bounds cross, and without it the clamp would pin the mark off the left edge.
    let cx = w.map_or(x, |w| x.clamp(EDGE, (w - MARK_W - EDGE).max(EDGE)));
    let cy = h.map_or(y, |h| y.clamp(EDGE, (h - MARK_H - EDGE).max(EDGE)));
    (cx, cy)
}

/// Where the mark was left, if anywhere. A stored pair is clamped on the way in — the window
/// it was saved from may have been a different size, or a different screen.
fn load_pos() -> Option<(f64, f64)> {
    let raw = storage()?.get_item(POS_KEY).ok().flatten()?;
    let (x, y) = raw.split_once(',')?;
    Some(clamp((x.trim().parse().ok()?, y.trim().parse().ok()?)))
}

fn save_pos(pos: Option<(f64, f64)>) {
    if let (Some(s), Some((x, y))) = (storage(), pos) {
        let _ = s.set_item(POS_KEY, &format!("{x},{y}"));
    }
}
