//! The top bar's paired machines, in both directions, dropped down under a count the way a macOS
//! menu-bar extra drops its menu.
//!
//! * **Sources** — the machines this one reads from: every paired node, and whether a dial to it
//!   connects right now (`GET /api/fleet/reach`). The count in the bar is these.
//! * **Viewers** — the machines that may read this one: every node granted something here, and
//!   whether it has lately (`FleetNode::active`, a request in within the last minute).
//!
//! Either row disconnects on a right click, or from the `⋯` it shows on hover — and what that
//! cuts is the row's own direction, by the operator's decision. A **viewer** loses its grants
//! here, so it can no longer read this machine but stays paired and stays a source. A **source**
//! is unpaired outright, since this machine reading it needs nothing but the pairing. Both ask
//! first: a pairing comes back only with a new invite, and grants only from the fleet settings.
//!
//! Two lists because they are two questions. A node can be reachable and never have called in, or
//! have called in a minute ago and be asleep now; one "active" for both said neither.

use adi_ui::{Icon, IconSize, Lucide};
use std::collections::HashMap;

use adi_webapp_api::types::{FleetNode, Reach};
use leptos::{ev, html, portal::Portal, prelude::*};

use crate::fetch;

/// How often the paired machines are counted and dialled again. A machine coming up is not urgent
/// news, and every round is a mesh dial per node. Opening the list asks at once.
const TICK_MS: u32 = 30_000;

/// The gap between the top bar and the list hanging from it.
const DROP_GAP: f64 = 4.0;

/// The width of a row's menu — `.adi-new-menu` in `_new_ui.scss` — so one opened near the right
/// edge is pulled back onto the screen.
const MENU_W: f64 = 264.0;

/// A row's menu: which node, where it was opened, and — for a viewer — the grants disconnecting
/// it takes away. `None` is a source, which is disconnected by unpairing.
#[derive(Clone)]
struct Target {
    node: String,
    grants: Option<Vec<String>>,
    x: f64,
    y: f64,
}

/// The count in the top bar and the list it opens.
#[component]
pub(super) fn Sources(#[prop(into)] light: Signal<bool>) -> impl IntoView {
    // `None` until the fleet first answers, so the bar never claims "0 of 0" it has not been told.
    let nodes = RwSignal::new(None::<Vec<FleetNode>>);
    // `None` while the first dials are out: a node not yet dialled is not yet unreachable.
    let reach = RwSignal::new(None::<HashMap<String, Reach>>);
    let open = RwSignal::new(false);
    // Where the list hangs: `(top, right)` in viewport pixels, from the button's own box.
    let at = RwSignal::new((0.0, 0.0));
    let panel: NodeRef<html::Div> = NodeRef::new();
    let menu = RwSignal::new(None::<Target>);
    let menu_el: NodeRef<html::Div> = NodeRef::new();
    // The menu's second step — "are you sure" — and what the unpair call last said, if it failed.
    let confirming = RwSignal::new(false);
    let failed = RwSignal::new(None::<String>);
    let busy = RwSignal::new(false);

    let load = move || {
        leptos::task::spawn_local(async move {
            // A failed recount keeps the last answer: stale by a tick beats blank.
            if let Ok(fleet) = fetch::fleet().await {
                nodes.set(Some(fleet.nodes));
            }
        });
        // Apart from the fleet, which is local and quick: this one waits on every node's dial.
        leptos::task::spawn_local(async move {
            if let Ok(r) = fetch::fleet_reach().await {
                reach.set(Some(r.nodes.into_iter().map(|n| (n.node, n.reach)).collect()));
            }
        });
    };
    load();
    let tick = set_interval_with_handle(load, std::time::Duration::from_millis(TICK_MS.into()));
    on_cleanup(move || {
        if let Ok(t) = tick {
            t.clear();
        }
    });

    // Focus goes into the list, so its Escape is heard there and stopped before the screen's own
    // Escape closes the front window underneath it.
    // The same for a row's menu while it is up; and back to the list when it goes, so the next
    // Escape closes the list rather than nothing.
    Effect::new(move |_| {
        if !open.get() {
            return;
        }
        if menu.with(Option::is_some) {
            if let Some(el) = menu_el.get() {
                let _ = el.focus();
            }
        } else if let Some(el) = panel.get() {
            let _ = el.focus();
        }
    });

    let close_menu = move || {
        menu.set(None);
        confirming.set(false);
        failed.set(None);
    };

    let disconnect = move |node: String, grants: Option<Vec<String>>| {
        busy.set(true);
        leptos::task::spawn_local(async move {
            let done = match grants {
                Some(grants) => revoke_all(&node, grants).await,
                None => fetch::fleet_unpair(node.clone()).await.map(|fleet| {
                    reach.update(|r| {
                        if let Some(r) = r {
                            r.remove(&node);
                        }
                    });
                    fleet
                }),
            };
            match done {
                Ok(fleet) => {
                    nodes.set(Some(fleet.nodes));
                    close_menu();
                }
                Err(e) => failed.set(Some(e)),
            }
            busy.set(false);
        });
    };

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
        load();
        open.set(true);
    };

    // `(reachable, paired)`; the first is `None` until the dials are back.
    let counts = move || {
        let paired = nodes.with(|n| n.as_ref().map(Vec::len))?;
        let up = reach.with(|r| {
            r.as_ref()
                .map(|r| r.values().filter(|r| **r == Reach::Reachable).count())
        });
        Some((up, paired))
    };

    let menu_view = move || {
        let Target { node, grants, x, y } = menu.get()?;
        let width = window()
            .inner_width()
            .ok()
            .and_then(|w| w.as_f64())
            .unwrap_or_default();
        let left = x.min(width - MENU_W - 8.0).max(8.0);
        let style = format!("left: {left}px; top: {y}px");
        let body = if confirming.get() {
            let (name, taking) = (node.clone(), grants.clone());
            let question = match &grants {
                Some(grants) => view! {
                    <p class="adi-new-menu__text">
                        "Stop "<strong>{node.clone()}</strong>" viewing this machine? It stays "
                        "paired, and this machine can still read it. Removes "
                        {grants.iter().map(|g| view! { <code>{g.clone()}</code>" " }).collect_view()}
                    </p>
                }
                .into_any(),
                None => view! {
                    <p class="adi-new-menu__text">
                        "Disconnect "<strong>{node.clone()}</strong>"? This unpairs it: neither "
                        "machine can reach the other until you pair them again."
                    </p>
                }
                .into_any(),
            };
            view! {
                {question}
                {move || failed.get().map(|e| view! { <p class="adi-new-menu__error">{e}</p> })}
                <div class="adi-new-menu__actions">
                    <button class="adi-new-menu__btn" type="button" on:click=move |_| close_menu()>
                        "Cancel"
                    </button>
                    <button
                        class="adi-new-menu__btn adi-new-menu__btn--danger"
                        type="button"
                        disabled=move || busy.get()
                        on:click=move |_| disconnect(name.clone(), taking.clone())
                    >
                        "Disconnect"
                    </button>
                </div>
            }
            .into_any()
        } else {
            view! {
                <button
                    class="adi-new-menu__item"
                    type="button"
                    role="menuitem"
                    on:click=move |_| confirming.set(true)
                >
                    <Icon icon=Lucide::Unplug size=IconSize::Md/>
                    "Disconnect…"
                </button>
            }
            .into_any()
        };
        Some(view! {
            <div
                class="adi-new-menu-catch"
                on:click=move |_| close_menu()
                on:contextmenu=move |ev: ev::MouseEvent| {
                    ev.prevent_default();
                    close_menu();
                }
            ></div>
            <div
                node_ref=menu_el
                class="adi-new-drop adi-new-menu"
                class:light=move || light.get()
                style=style
                role="menu"
                aria-label=format!("{node} actions")
                tabindex="-1"
                on:keydown=move |ev: ev::KeyboardEvent| {
                    if ev.key() == "Escape" {
                        ev.prevent_default();
                        ev.stop_propagation();
                        close_menu();
                    }
                }
            >
                {body}
            </div>
        })
    };

    view! {
        {move || counts().map(|(up, paired)| {
            let label = match up {
                Some(up) => format!("{up} of {paired} sources reachable"),
                None => format!("Checking {paired} sources"),
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
                    aria-label="Paired machines"
                    tabindex="-1"
                    on:keydown=move |ev: ev::KeyboardEvent| {
                        if ev.key() == "Escape" {
                            ev.prevent_default();
                            ev.stop_propagation();
                            open.set(false);
                        }
                    }
                >
                    {move || match nodes.get() {
                        None => view! { <p class="adi-new-drop__empty">"Loading…"</p> }.into_any(),
                        Some(n) if n.is_empty() => view! {
                            <p class="adi-new-drop__empty">"No paired machines yet"</p>
                        }
                        .into_any(),
                        Some(n) => {
                            let r = reach.get();
                            view! { {sources(&n, r.as_ref(), menu)} {viewers(&n, menu)} }
                                .into_any()
                        }
                    }}
                </div>
                {menu_view}
            </Portal>
        </Show>
    }
}

/// One row: a dot for its state, the machine's name, and the state in words.
struct Row {
    name: String,
    /// The node's own name for itself, when it differs — what it is recognised by over there.
    called: Option<String>,
    /// `on`, `warn`, `err` or `off` — the dot's colour.
    tone: &'static str,
    state: String,
    /// A viewer's grants here, which its Disconnect revokes; `None` on a source.
    grants: Option<Vec<String>>,
}

impl Row {
    fn new(n: &FleetNode, tone: &'static str, state: String) -> Self {
        Self {
            name: n.petname.clone(),
            called: (n.nickname != n.petname).then(|| format!("Calls itself {}", n.nickname)),
            tone,
            state,
            grants: None,
        }
    }
}

/// A titled section: its head, a count beside it, and its rows.
fn section(
    title: &'static str,
    meta: String,
    rows: Vec<Row>,
    menu: RwSignal<Option<Target>>,
) -> impl IntoView {
    view! {
        <section class="adi-new-drop__section">
            <div class="adi-new-drop__head">
                <span class="adi-new-drop__title">{title}</span>
                <span class="adi-new-drop__meta">{meta}</span>
            </div>
            <ul class="adi-new-drop__list">
                {rows
                    .into_iter()
                    .map(|r| {
                        let target = {
                            let (node, grants) = (r.name.clone(), r.grants.clone());
                            move |x, y| Target { node: node.clone(), grants: grants.clone(), x, y }
                        };
                        let on_more = target.clone();
                        view! {
                            <li
                                class="adi-new-drop__row"
                                title=r.called
                                on:contextmenu=move |ev: ev::MouseEvent| {
                                    ev.prevent_default();
                                    menu.set(Some(target(
                                        f64::from(ev.client_x()),
                                        f64::from(ev.client_y()),
                                    )));
                                }
                            >
                                <span class="adi-new-drop__dot" data-tone=r.tone></span>
                                <span class="adi-new-drop__name">{r.name.clone()}</span>
                                <span class="adi-new-drop__seen">{r.state}</span>
                                <button
                                    class="adi-new-drop__more"
                                    type="button"
                                    aria-label=format!("{} actions", r.name)
                                    aria-haspopup="menu"
                                    on:click=move |ev: ev::MouseEvent| {
                                        let Some(el) = ev.current_target().and_then(|t| {
                                            wasm_bindgen::JsCast::dyn_into::<web_sys::Element>(t)
                                                .ok()
                                        }) else {
                                            return;
                                        };
                                        let b = el.get_bounding_client_rect();
                                        menu.set(Some(on_more(b.left(), b.bottom() + DROP_GAP)));
                                    }
                                >
                                    <Icon icon=Lucide::Ellipsis size=IconSize::Sm/>
                                </button>
                            </li>
                        }
                    })
                    .collect_view()}
            </ul>
        </section>
    }
}

/// Every paired node, as something this machine dials: reachable first, then by name.
fn sources(
    nodes: &[FleetNode],
    reach: Option<&HashMap<String, Reach>>,
    menu: RwSignal<Option<Target>>,
) -> impl IntoView {
    let mut rows: Vec<(u8, Row)> = nodes
        .iter()
        .map(|n| {
            let (rank, tone, state) = match reach.and_then(|r| r.get(&n.petname)) {
                Some(Reach::Reachable) => (0, "on", "Reachable"),
                Some(Reach::Refused) => (1, "warn", "Refuses this machine"),
                Some(Reach::Unreachable) => (2, "err", "Unreachable"),
                Some(Reach::MeshOff) => (2, "err", "Mesh is off"),
                None => (3, "off", "Checking…"),
            };
            (rank, Row::new(n, tone, state.into()))
        })
        .collect();
    rows.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.name.cmp(&b.1.name)));
    let meta = match reach {
        Some(r) => format!(
            "{} of {} reachable",
            r.values().filter(|r| **r == Reach::Reachable).count(),
            nodes.len()
        ),
        None => "Checking…".into(),
    };
    section("Sources", meta, rows.into_iter().map(|(_, r)| r).collect(), menu)
}

/// The nodes granted something here — the ones that can read this machine — by how lately they
/// did: connected now, then most recently seen, then never.
fn viewers(nodes: &[FleetNode], menu: RwSignal<Option<Target>>) -> impl IntoView {
    let mut granted: Vec<&FleetNode> = nodes.iter().filter(|n| !n.grants.is_empty()).collect();
    if granted.is_empty() {
        return None;
    }
    granted.sort_by(|a, b| {
        b.active
            .cmp(&a.active)
            .then(b.last_seen.cmp(&a.last_seen))
            .then_with(|| a.petname.cmp(&b.petname))
    });
    let now = now_unix();
    let connected = granted.iter().filter(|n| n.active).count();
    let rows = granted
        .into_iter()
        .map(|n| {
            let tone = if n.active { "on" } else { "off" };
            Row {
                grants: Some(n.grants.clone()),
                ..Row::new(n, tone, seen(n.active, n.last_seen, now))
            }
        })
        .collect();
    Some(section("Viewers", format!("{connected} connected"), rows, menu))
}

/// Take every grant a node holds here, one call each; the fleet as the last one left it.
async fn revoke_all(
    node: &str,
    grants: Vec<String>,
) -> Result<adi_webapp_api::types::FleetState, String> {
    let mut fleet = None;
    for grant in grants {
        fleet = Some(fetch::fleet_revoke(node.to_string(), grant).await?);
    }
    match fleet {
        Some(fleet) => Ok(fleet),
        None => fetch::fleet().await,
    }
}

/// A viewer's state as a person reads it: `Connected now`, `Seen 3d ago`, `Never connected`.
fn seen(active: bool, last_seen: Option<u64>, now: u64) -> String {
    match (active, last_seen) {
        (true, _) => "Connected now".into(),
        (false, None) => "Never connected".into(),
        (false, Some(at)) => match now.saturating_sub(at) {
            0..=59 => "Seen just now".into(),
            s if s < 3_600 => format!("Seen {}m ago", s / 60),
            s if s < 86_400 => format!("Seen {}h ago", s / 3_600),
            s => format!("Seen {}d ago", s / 86_400),
        },
    }
}

/// Now, in Unix seconds, by the browser's clock.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn now_unix() -> u64 {
    (js_sys::Date::now() / 1000.0) as u64
}

#[cfg(test)]
mod tests {
    use super::seen;

    #[test]
    fn seen_reads_the_gap_and_trusts_connected_over_it() {
        let now = 1_000_000;
        assert_eq!(seen(true, Some(0), now), "Connected now");
        assert_eq!(seen(false, None, now), "Never connected");
        assert_eq!(seen(false, Some(now - 90), now), "Seen 1m ago");
        assert_eq!(seen(false, Some(now - 3 * 86_400), now), "Seen 3d ago");
    }
}
