//! The Live graph page (`/extended/live-graph`): what set what off on this machine, on one canvas.
//!
//! The picture is a chain — you, the conversation you opened, the agent that ran it, the
//! conversations that agent started, the agents *those* belong to, and on. [`model`] builds it from
//! the two listings the panel already carries, so the page costs no endpoint of its own; [`paint`]
//! puts it on the canvas; [`view`] is the one value that says how far in the canvas is.
//!
//! Nothing here is a timeline. Left to right is *causation*, not time: a column further right was
//! set off by the one before it, and two boxes in the same column have nothing in common but their
//! distance from where the work came from.
//!
//! The graph is rebuilt only when the data or the toggles change (it is a [`Memo`]); panning,
//! zooming and hovering repaint from the same laid-out nodes.

mod model;
mod paint;
mod view;

use adi_webapp_api::types::AllAgentRuns;
use leptos::html;
use leptos::prelude::*;
use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::Closure;
use web_sys::ResizeObserver;

use crate::routing::{Route, go_global};
use crate::state::{AgentsForm, AgentsWatch, State, read_error};
use model::{Action, Graph, Options};
use view::Viewport;

/// How much zoom one pixel of wheel travel buys, as a rate — the factor is `exp(-delta * this)`, so
/// a fast scroll and a slow one that cover the same distance land on the same scale, and zooming
/// out then back in returns exactly where it started.
const ZOOM_PER_PX: f64 = 0.0015;

/// A wheel event reports its delta in pixels, lines or pages (`deltaMode`); Firefox and some mice
/// send lines. This is the pixel worth of one line — Chrome's own conversion, near enough that a
/// notch feels the same across browsers.
const LINE_PX: f64 = 16.0;

/// How far the pointer may travel between going down and coming up and still be a click. Below
/// this, a press that opens a conversation; above it, a pan that happened to start on a box.
const CLICK_SLOP: f64 = 4.0;

/// The page's own state: the view, what is drawn, and what the pointer is on.
///
/// Held by the shell rather than made in [`live_graph_view`], like every other page's view state —
/// that function re-runs on a route change, and signals made inside it would put the view back at
/// the top every time somebody looked at something else.
#[derive(Clone, Copy)]
pub(crate) struct GraphView {
    view: RwSignal<Viewport>,
    chats: RwSignal<bool>,
    tools: RwSignal<bool>,
    idle: RwSignal<bool>,
    /// The node under the pointer, as an index into the current graph.
    hover: RwSignal<Option<usize>>,
    /// Whether the view has been fitted to the graph now on screen. Set when a fit happens and
    /// cleared whenever the toggles change what is drawn: a new picture deserves a look at all of
    /// it, but data arriving — a conversation starting, a run ending — must never yank the view
    /// out from under somebody who has panned somewhere.
    fitted: RwSignal<bool>,
}

impl GraphView {
    pub(crate) fn new() -> Self {
        let o = Options::default();
        Self {
            view: RwSignal::new(Viewport::HOME),
            chats: RwSignal::new(o.chats),
            tools: RwSignal::new(o.tools),
            idle: RwSignal::new(o.idle),
            hover: RwSignal::new(None),
            fitted: RwSignal::new(false),
        }
    }

    /// Draw something else. Every toggle goes through here, because every one of them changes the
    /// shape of the graph enough that the view is worth refitting.
    fn toggle(self, which: RwSignal<bool>, on: bool) {
        which.set(on);
        self.hover.set(None);
        self.fitted.set(false);
    }
}

/// The Live graph page: a head with what is drawn and the controls over it, and the canvas.
// One canvas with six handlers on it. They are listed here rather than lifted into functions
// because each is three lines that all read the same four signals, and passing those around would
// be more code saying less.
#[allow(clippy::too_many_lines)]
pub(crate) fn live_graph_view(
    state: State,
    gv: GraphView,
    form: AgentsForm,
    watch: AgentsWatch,
    route: RwSignal<Route>,
) -> AnyView {
    let canvas: NodeRef<html::Canvas> = NodeRef::new();
    // The last pointer position of a drag, and `None` between drags — so the move handler knows
    // whether it is panning without asking the event, which reports no button on a trackpad.
    let drag = RwSignal::new(None::<(f64, f64)>);
    let panning = RwSignal::new(false);
    // Whether this press has travelled far enough to be a drag. A press that has not is a click,
    // and a click on a box opens what it stands for.
    let dragged = RwSignal::new(false);
    let watcher: Resize = StoredValue::new_local(None);

    // The graph itself. A memo, so it is rebuilt when the listings or the toggles change and not
    // once per frame — every pan and every hover paints the same laid-out nodes.
    let graph = Memo::new(move |_| {
        let options = Options {
            chats: gv.chats.get(),
            tools: gv.tools.get(),
            idle: gv.idle.get(),
        };
        state.agents.with(|agents| {
            state.all_chats.with(|chats| {
                let none = AllAgentRuns {
                    agents: Vec::new(),
                    total: 0,
                };
                model::build(
                    agents.as_ref().map_or(&[][..], |a| a.agents.as_slice()),
                    chats.as_ref().unwrap_or(&none),
                    options,
                )
            })
        })
    });

    // Fit the first graph that arrives, and any graph the toggles ask for. Kept apart from the
    // paint effect below because this one *writes* the view the other one reads.
    Effect::new(move |_| {
        if gv.fitted.get() {
            return;
        }
        let Some(c) = canvas.get() else { return };
        let extent = graph.with(|gr| (!gr.is_empty()).then_some(gr.extent));
        let Some(extent) = extent else { return };
        let rect = c.get_bounding_client_rect();
        gv.view.set(Viewport::fit(extent, rect.width(), rect.height()));
        gv.fitted.set(true);
    });

    // Repaint on every change of what is drawn or how it is looked at, and once when the canvas
    // itself lands.
    Effect::new(move |_| {
        let Some(c) = canvas.get() else { return };
        let (v, hover) = (gv.view.get(), gv.hover.get());
        graph.with(|gr| paint::draw(&c, gr, v, hover));
        if watcher.with_value(Option::is_some) {
            return;
        }
        // A `resize` listener on the window would miss most of what resizes this pane: the explorer
        // and the store rail change its width with the window standing still.
        let el = c.clone();
        let cb = Closure::<dyn FnMut()>::new(move || {
            let (v, hover) = (gv.view.get_untracked(), gv.hover.get_untracked());
            graph.with_untracked(|gr| paint::draw(&el, gr, v, hover));
        });
        if let Ok(ro) = ResizeObserver::new(cb.as_ref().unchecked_ref()) {
            ro.observe(&c);
            watcher.set_value(Some((ro, cb)));
        }
    });
    on_cleanup(move || {
        // Disconnected before the closure it calls is dropped, never the other way round.
        watcher.update_value(|w| {
            if let Some((ro, _)) = w.take() {
                ro.disconnect();
            }
        });
    });

    // Where a pointer event is, in the graph's own units, and which node is under it.
    let under = move |x: f64, y: f64| -> Option<usize> {
        let c = canvas.get_untracked()?;
        let r = c.get_bounding_client_rect();
        let (wx, wy) = gv.view.get_untracked().world(
            r.width(),
            r.height(),
            x - r.left(),
            y - r.top(),
        );
        graph.with_untracked(|gr| gr.at(wx, wy))
    };

    // Whether what the pointer is on goes anywhere, which is the only thing the cursor promises.
    let actionable = move || {
        gv.hover.get().is_some_and(|i| {
            graph.with(|gr| gr.nodes.get(i).is_some_and(|n| n.action != Action::None))
        })
    };

    let open = move |i: usize| {
        let action = graph.with_untracked(|gr| gr.nodes.get(i).map(|n| n.action.clone()));
        match action {
            Some(Action::Agent(name)) => {
                let defined = state.agents.with_untracked(|a| {
                    a.as_ref()
                        .and_then(|s| s.agents.iter().find(|x| x.name == name).cloned())
                });
                // An agent the history names but that no longer exists has no definition to open,
                // and the Agents list is where you would go to see that it is gone.
                match defined.as_ref() {
                    Some(dto) => super::agents::open_agent_editor(state, route, form, Some(dto)),
                    None => go_global(state, route, Route::Agents),
                }
            }
            Some(Action::Chat {
                agent,
                run_id,
                interactive,
            }) => {
                // The Agents page is where a conversation opened from a listing lands, so opening
                // one from here means the same thing it means there.
                go_global(state, route, Route::Agents);
                super::agents::open_conversation(watch, agent, run_id, interactive);
            }
            Some(Action::None) | None => {}
        }
    };

    view! {
        <section class="adi-panel adi-panel--fill adi-graph">
            <header class="adi-bar adi-graph__head">
                <h1 class="adi-bar__title">"Live graph"</h1>
                // One line that is either what the whole picture is, or — while the pointer is on
                // something — what that one thing is. A tooltip would say the same words in a box
                // that covers the graph they are about.
                <span class="adi-graph__note">{move || {
                    let hovered = gv.hover.get().and_then(|i| {
                        graph.with(|gr| gr.nodes.get(i).map(|n| n.meta.clone()))
                    });
                    hovered.unwrap_or_else(|| graph.with(Graph::summary))
                }}</span>
                <span class="adi-spacer"></span>
                <span class="adi-graph__toggles">
                    {toggle("Chats", gv.chats, move |on| gv.toggle(gv.chats, on),
                        "Draw each agent's newest conversations, not only the agents")}
                    {toggle("Tools", gv.tools, move |on| gv.toggle(gv.tools, on),
                        "Draw the tools each agent may run, in a column of their own")}
                    {toggle("Idle agents", gv.idle, move |on| gv.toggle(gv.idle, on),
                        "Keep agents that have never run — they connect to nothing")}
                </span>
                <span class="adi-graph__zoom adi-tabnums">
                    {move || format!("{:.0}%", gv.view.get().scale * 100.0)}
                </span>
                <button class="adi-btn adi-btn--sm" type="button"
                    title="Put the whole graph on screen"
                    on:click=move |_| gv.fitted.set(false)>"Fit"</button>
            </header>

            <div class="adi-graph__stage"
                // The gestures, where somebody would hover to ask what this is. Not a line in the
                // head: the head's one line is already saying what is on the canvas, and that is
                // the more useful of the two once you are looking at it.
                title="Scroll to zoom, drag to pan \u{2014} click a box to open it"
                class:adi-graph__stage--panning=move || panning.get()
                class:adi-graph__stage--link=actionable>
                <canvas class="adi-graph__canvas" node_ref=canvas
                    aria-label="Every agent on this machine and what set it off"

                    on:wheel=move |ev: web_sys::WheelEvent| {
                        // The wheel *is* the zoom here, so neither the pane's scroll nor the
                        // browser's own ctrl+wheel zoom may have it. A trackpad pinch arrives as a
                        // wheel with ctrlKey set, which is why both take this one path.
                        ev.prevent_default();
                        let Some(c) = canvas.get_untracked() else { return };
                        let r = c.get_bounding_client_rect();
                        let (x, y) = (
                            f64::from(ev.client_x()) - r.left(),
                            f64::from(ev.client_y()) - r.top(),
                        );
                        let dy = wheel_px(&ev, r.height());
                        gv.view.update(|v| {
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
                        drag.set(Some(pointer(&ev)));
                        dragged.set(false);
                        panning.set(true);
                    }

                    on:pointermove=move |ev: web_sys::PointerEvent| {
                        let (x, y) = pointer(&ev);
                        let Some((px, py)) = drag.get_untracked() else {
                            // Not panning: the pointer is only asking what it is over.
                            let hit = under(x, y);
                            if gv.hover.get_untracked() != hit {
                                gv.hover.set(hit);
                            }
                            return;
                        };
                        drag.set(Some((x, y)));
                        if (x - px).hypot(y - py) > 0.0 {
                            dragged.set(dragged.get_untracked()
                                || (x - px).abs() + (y - py).abs() > CLICK_SLOP);
                        }
                        // A pan moves the world under the pointer by exactly what the pointer
                        // moved, at any scale — the drag is in screen pixels and so is the pan.
                        gv.view.update(|v| {
                            v.pan_x += x - px;
                            v.pan_y += y - py;
                        });
                    }

                    on:pointerup=move |ev: web_sys::PointerEvent| {
                        let was_drag = dragged.get_untracked();
                        drag.set(None);
                        panning.set(false);
                        if was_drag {
                            return;
                        }
                        let (x, y) = pointer(&ev);
                        if let Some(i) = under(x, y) {
                            open(i);
                        }
                    }

                    on:pointercancel=move |_| { drag.set(None); panning.set(false); }
                    on:pointerleave=move |_| { gv.hover.set(None); }
                ></canvas>

                {move || empty_view(state, graph)}
            </div>
        </section>
    }
    .into_any()
}

/// One of the head's switches: what to draw, and the sentence saying what turning it on does.
fn toggle(
    label: &'static str,
    on: RwSignal<bool>,
    set: impl Fn(bool) + 'static,
    title: &'static str,
) -> impl IntoView {
    view! {
        <label class="adi-graph__toggle" title=title>
            <input type="checkbox" class="adi-check"
                prop:checked=move || on.get()
                on:change=move |ev| set(event_target_checked(&ev)) />
            <span>{label}</span>
        </label>
    }
}

/// What to say over a canvas with nothing on it. A read that failed and a read that has not
/// answered yet look identical from here, so the failure is named rather than left as a page that
/// says "loading" for ever.
fn empty_view(state: State, graph: Memo<Graph>) -> Option<AnyView> {
    if !graph.with(Graph::is_empty) {
        return None;
    }
    let waiting = state.all_chats.with(Option::is_none);
    let text = match (read_error(state, "/api/agents/runs/all"), waiting) {
        (Some(why), _) => format!("Couldn't load this: {why}"),
        (None, true) => "Loading\u{2026}".to_string(),
        (None, false) => {
            "Nothing has run on this machine yet. Start a chat and it appears here.".to_string()
        }
    };
    Some(view! { <div class="adi-graph__empty">{text}</div> }.into_any())
}

/// A pointer position as the arithmetic wants it. The DOM reports whole pixels here; everything
/// downstream is in fractions of one, because the scale is.
fn pointer(ev: &web_sys::PointerEvent) -> (f64, f64) {
    (f64::from(ev.client_x()), f64::from(ev.client_y()))
}

/// One wheel event's travel in pixels, whatever unit it chose to report in.
fn wheel_px(ev: &web_sys::WheelEvent, stage_h: f64) -> f64 {
    match ev.delta_mode() {
        web_sys::WheelEvent::DOM_DELTA_LINE => ev.delta_y() * LINE_PX,
        web_sys::WheelEvent::DOM_DELTA_PAGE => ev.delta_y() * stage_h,
        _ => ev.delta_y(),
    }
}

/// The observer watching the stage for a size change, and the closure it calls — held together
/// because the one must be disconnected before the other is dropped.
///
/// Stored *locally*: both are JS handles, which are neither `Send` nor `Sync`, and the ordinary
/// [`StoredValue`] demands both (the same reason [`crate::voice`]'s session is).
type Resize = StoredValue<Option<(ResizeObserver, Closure<dyn FnMut()>)>, LocalStorage>;
