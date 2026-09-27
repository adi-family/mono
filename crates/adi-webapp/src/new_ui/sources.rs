//! The top bar's devices: every machine paired with this one, how many answer right now — and, on
//! a click, each one dropped down under the count the way a macOS menu-bar extra drops its menu.
//!
//! A row says only what the pairing carries, by the operator's direction, each with its date:
//! **can access you** (it holds grants here), with when it last did (`FleetNode::last_seen`), and
//! **you can access** (this machine reads it; `FleetNode::source`), with when a dial from here last
//! got an answer (`FleetNode::last_reached`). "You" is this machine, the one the panel is open on.
//! Everything else about a device — its reachability, its key, taking either of those away,
//! unpairing — is on its own page ([`super::device`]), which a click on the row opens.

use adi_ui::{Icon, IconSize, Lucide};

use adi_webapp_api::types::{FleetNode, Reach};
use leptos::{ev, html, portal::Portal, prelude::*};

use super::fleet::{Fleet, ago, now_unix};
use super::windows::Desk;

/// The gap between the top bar and the list hanging from it.
const DROP_GAP: f64 = 4.0;

/// The count in the top bar and the list it opens.
#[component]
pub(super) fn Sources(
    fleet: Fleet,
    desk: Desk,
    #[prop(into)] light: Signal<bool>,
) -> impl IntoView {
    let open = RwSignal::new(false);
    // Where the list hangs: `(top, right)` in viewport pixels, from the button's own box.
    let at = RwSignal::new((0.0, 0.0));
    let panel: NodeRef<html::Div> = NodeRef::new();

    // Focus goes into the list, so its Escape is heard there and stopped before the screen's own
    // Escape closes the front window underneath it.
    Effect::new(move |_| {
        if open.get()
            && let Some(el) = panel.get()
        {
            let _ = el.focus();
        }
    });

    let show = move |ev: ev::MouseEvent| {
        if let Some(el) = ev
            .current_target()
            .and_then(|t| wasm_bindgen::JsCast::dyn_into::<web_sys::Element>(t).ok())
        {
            let r = el.get_bounding_client_rect();
            let width = window()
                .inner_width()
                .ok()
                .and_then(|w| w.as_f64())
                .unwrap_or_default();
            at.set((r.bottom() + DROP_GAP, width - r.right()));
        }
        fleet.refresh();
        open.set(true);
    };

    let pick = move |petname: String| {
        open.set(false);
        desk.open_device(petname);
    };

    // `(reachable, devices)`; the first is `None` until the dials are back.
    let counts = move || {
        let devices = fleet.nodes.with(|n| n.as_ref().map(Vec::len))?;
        let up = fleet.reach.with(|r| {
            r.as_ref()
                .map(|r| r.values().filter(|r| **r == Reach::Reachable).count())
        });
        Some((up, devices))
    };

    view! {
        {move || counts().map(|(up, paired)| {
            let label = match up {
                Some(up) => format!("{up} of {paired} devices reachable"),
                None => format!("Checking {paired} devices"),
            };
            view! {
                <button
                    class="adi-new-top__status adi-new-top__sources"
                    class:is-open=move || open.get()
                    type="button"
                    title=label.clone()
                    aria-label=label
                    aria-haspopup="dialog"
                    aria-expanded=move || open.get().to_string()
                    on:click=show
                >
                    <Icon icon=Lucide::Network size=IconSize::Sm/>
                    <span class="adi-new-top__count">
                        {up.map_or_else(|| "–".to_string(), |u| u.to_string())}
                        <span class="adi-new-top__of">"/"{paired}</span>
                    </span>
                </button>
            }
        })}
        <Show when=move || open.get()>
            // Out of the top bar, into the body: the bar's backdrop blur makes it the containing
            // block of anything fixed inside it, so a click-catcher drawn there would cover only
            // the bar, and a second blur nested in the first draws nothing.
            <Portal>
                // `click`, not `pointerdown`: the catcher covers the count too, and closing on
                // the press would hand the click that follows to the count, opening it again.
                <div
                    class="adi-new-drop-catch"
                    on:click=move |_| open.set(false)
                    on:contextmenu=move |ev: ev::MouseEvent| {
                        ev.prevent_default();
                        open.set(false);
                    }
                ></div>
                <div
                    node_ref=panel
                    class="adi-new-drop"
                    class:light=move || light.get()
                    style=move || {
                        let (top, right) = at.get();
                        format!("top: {top}px; right: {right}px")
                    }
                    role="dialog"
                    aria-modal="true"
                    aria-label="Devices"
                    tabindex="-1"
                    on:keydown=move |ev: ev::KeyboardEvent| {
                        if ev.key() == "Escape" {
                            ev.prevent_default();
                            ev.stop_propagation();
                            open.set(false);
                        }
                    }
                >
                    {move || match fleet.nodes.get() {
                        None => view! { <p class="adi-new-drop__empty">"Loading…"</p> }.into_any(),
                        Some(n) if n.is_empty() => view! {
                            <p class="adi-new-drop__empty">"No paired machines yet"</p>
                        }
                        .into_any(),
                        Some(n) => devices(n, pick).into_any(),
                    }}
                </div>
            </Portal>
        </Show>
    }
}

/// Every paired device, by name.
fn devices(
    mut nodes: Vec<FleetNode>,
    pick: impl Fn(String) + Copy + Send + Sync + 'static,
) -> impl IntoView {
    let now = now_unix();
    nodes.sort_by(|a, b| a.petname.cmp(&b.petname));
    view! {
        <section class="adi-new-drop__section">
            <div class="adi-new-drop__head">
                <span class="adi-new-drop__title">"Devices"</span>
            </div>
            <ul class="adi-new-drop__list">
                {nodes.into_iter().map(|n| row(n, now, pick)).collect_view()}
            </ul>
        </section>
    }
}

/// One device: its name, then a line for each way the pairing goes, with its date.
fn row(n: FleetNode, now: u64, pick: impl Fn(String) + Send + Sync + 'static) -> impl IntoView {
    let lines = access(&n, now);
    let label = format!("{} — open device", n.petname);
    let name = n.petname;
    let open = name.clone();
    view! {
        <li class="adi-new-drop__item">
            <button
                class="adi-new-drop__row"
                type="button"
                aria-label=label
                on:click=move |_| pick(open.clone())
            >
                <span class="adi-new-drop__text">
                    <span class="adi-new-drop__name">{name}</span>
                    {lines
                        .into_iter()
                        .map(|(what, when)| view! {
                            <span class="adi-new-drop__access">
                                <span>{what}</span>
                                <span class="adi-new-drop__when">{when}</span>
                            </span>
                        })
                        .collect_view()}
                </span>
                <Icon icon=Lucide::ChevronRight size=IconSize::Sm/>
            </button>
        </li>
    }
}

/// The ways a pairing goes, each with when it last did: `("Can access you", "Last access 3d ago")`.
pub(super) fn access(n: &FleetNode, now: u64) -> Vec<(&'static str, String)> {
    let mut lines = Vec::new();
    if !n.grants.is_empty() {
        lines.push((
            "Can access you",
            format!("Last access {}", ago(n.last_seen, now)),
        ));
    }
    if n.source {
        lines.push((
            "You can access",
            format!("Last connected {}", ago(n.last_reached, now)),
        ));
    }
    lines
}
