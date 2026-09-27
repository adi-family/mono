//! The top bar's devices: every machine paired with this one, how many answer right now — and, on
//! a click, each one with what it can do, dropped down under the count the way a macOS menu-bar
//! extra drops its menu.
//!
//! Told from the device's side, by the operator's direction: one row per device, its reachability
//! (`GET /api/fleet/reach`) on the dot, and under its name, as tags, the two things a pairing can
//! carry —
//! **can access you** (it holds grants here; `FleetNode::active` says it did within the last
//! minute) and **you can access** (this machine reads it; `FleetNode::source`). "You" is this
//! machine, the one the panel is open on.
//!
//! Each of those is taken away on its own from the row's menu — a right click, or the `⋯` it shows
//! on hover. The pairing goes only when neither is left, and the confirmation says when it will.

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

/// One of the two things a pairing can carry — which one a menu item takes away.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Role {
    /// This machine reads the device.
    Source,
    /// The device reads this machine.
    Viewer,
}

/// A row's menu: which device, what it can do now, and where the menu was opened.
#[derive(Clone)]
struct Target {
    node: String,
    /// Its grants here — what "stop it accessing you" takes away.
    grants: Vec<String>,
    accesses_you: bool,
    you_access: bool,
    x: f64,
    y: f64,
}

impl Target {
    fn new(n: &FleetNode, x: f64, y: f64) -> Self {
        Self {
            node: n.petname.clone(),
            grants: n.grants.clone(),
            accesses_you: !n.grants.is_empty(),
            you_access: n.source,
            x,
            y,
        }
    }
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
    // The menu's second step — "are you sure", about which of the two — and what the call last
    // said, if it failed.
    let confirming = RwSignal::new(None::<Role>);
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
        confirming.set(None);
        failed.set(None);
    };

    let disconnect = move |node: String, role: Role| {
        busy.set(true);
        leptos::task::spawn_local(async move {
            let done = match role {
                Role::Source => fetch::fleet_drop_source(node).await,
                Role::Viewer => fetch::fleet_drop_viewer(node).await,
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

    // `(reachable, devices)`; the first is `None` until the dials are back.
    let counts = move || {
        let devices = nodes.with(|n| n.as_ref().map(Vec::len))?;
        let up = reach.with(|r| {
            r.as_ref()
                .map(|r| r.values().filter(|r| **r == Reach::Reachable).count())
        });
        Some((up, devices))
    };

    let menu_view = move || {
        let Target {
            node,
            grants,
            accesses_you,
            you_access,
            x,
            y,
        } = menu.get()?;
        let width = window()
            .inner_width()
            .ok()
            .and_then(|w| w.as_f64())
            .unwrap_or_default();
        let left = x.min(width - MENU_W - 8.0).max(8.0);
        let style = format!("left: {left}px; top: {y}px");
        let body = match confirming.get() {
            Some(role) => {
                let name = node.clone();
                // Whether the other of the two survives; if not, this unpairs.
                let keeps = match role {
                    Role::Source => accesses_you,
                    Role::Viewer => you_access,
                };
                let question = match (role, keeps) {
                    (Role::Viewer, true) => view! {
                        <p class="adi-new-menu__text">
                            "Stop "<strong>{node.clone()}</strong>" accessing you? It stays "
                            "paired, and you can still access it. Removes "
                            {grants
                                .iter()
                                .map(|g| view! { <code>{g.clone()}</code>" " })
                                .collect_view()}
                        </p>
                    }
                    .into_any(),
                    (Role::Source, true) => view! {
                        <p class="adi-new-menu__text">
                            "Stop accessing "<strong>{node.clone()}</strong>"? It stays paired "
                            "and can still access you. This machine forgets its password for it."
                        </p>
                    }
                    .into_any(),
                    (_, false) => view! {
                        <p class="adi-new-menu__text">
                            "That is the last thing "<strong>{node.clone()}</strong>" is paired "
                            "for, so this unpairs it: only a new invite pairs them again."
                        </p>
                    }
                    .into_any(),
                };
                let verb = if keeps { "Remove" } else { "Unpair" };
                view! {
                    {question}
                    {move || failed.get().map(|e| view! { <p class="adi-new-menu__error">{e}</p> })}
                    <div class="adi-new-menu__actions">
                        <button
                            class="adi-new-menu__btn"
                            type="button"
                            on:click=move |_| close_menu()
                        >
                            "Cancel"
                        </button>
                        <button
                            class="adi-new-menu__btn adi-new-menu__btn--danger"
                            type="button"
                            disabled=move || busy.get()
                            on:click=move |_| disconnect(name.clone(), role)
                        >
                            {verb}
                        </button>
                    </div>
                }
                .into_any()
            }
            None => view! {
                {accesses_you.then(|| view! {
                    <button
                        class="adi-new-menu__item"
                        type="button"
                        role="menuitem"
                        on:click=move |_| confirming.set(Some(Role::Viewer))
                    >
                        <Icon icon=Lucide::EyeOff size=IconSize::Md/>
                        "Stop it accessing you…"
                    </button>
                })}
                {you_access.then(|| view! {
                    <button
                        class="adi-new-menu__item"
                        type="button"
                        role="menuitem"
                        on:click=move |_| confirming.set(Some(Role::Source))
                    >
                        <Icon icon=Lucide::Unplug size=IconSize::Md/>
                        "Stop accessing it…"
                    </button>
                })}
            }
            .into_any(),
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
                    {move || match nodes.get() {
                        None => view! { <p class="adi-new-drop__empty">"Loading…"</p> }.into_any(),
                        Some(n) if n.is_empty() => view! {
                            <p class="adi-new-drop__empty">"No paired machines yet"</p>
                        }
                        .into_any(),
                        Some(n) => {
                            let r = reach.get();
                            devices(&n, r.as_ref(), menu).into_any()
                        }
                    }}
                </div>
                {menu_view}
            </Portal>
        </Show>
    }
}

/// Every paired device: reachable first, then refusing, then out of reach, by name within each.
fn devices(
    nodes: &[FleetNode],
    reach: Option<&HashMap<String, Reach>>,
    menu: RwSignal<Option<Target>>,
) -> impl IntoView {
    let now = now_unix();
    let mut rows: Vec<(u8, &FleetNode, &'static str, &'static str)> = nodes
        .iter()
        .map(|n| {
            let (rank, tone, state) = match reach.and_then(|r| r.get(&n.petname)) {
                Some(Reach::Reachable) => (0, "on", "Reachable"),
                Some(Reach::Refused) => (1, "warn", "Refuses this machine"),
                Some(Reach::Unreachable) => (2, "err", "Unreachable"),
                Some(Reach::MeshOff) => (2, "err", "Mesh is off"),
                None => (3, "off", "Checking…"),
            };
            (rank, n, tone, state)
        })
        .collect();
    rows.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.petname.cmp(&b.1.petname)));
    let meta = match reach {
        Some(r) => format!(
            "{} of {} reachable",
            r.values().filter(|r| **r == Reach::Reachable).count(),
            nodes.len()
        ),
        None => "Checking…".into(),
    };
    view! {
        <section class="adi-new-drop__section">
            <div class="adi-new-drop__head">
                <span class="adi-new-drop__title">"Devices"</span>
                <span class="adi-new-drop__meta">{meta}</span>
            </div>
            <ul class="adi-new-drop__list">
                {rows
                    .into_iter()
                    .map(|(_, n, tone, state)| row(n, tone, state, now, menu))
                    .collect_view()}
            </ul>
        </section>
    }
}

/// One device: the dot and its reachability, its name, and under it what it can do.
fn row(
    n: &FleetNode,
    tone: &'static str,
    state: &'static str,
    now: u64,
    menu: RwSignal<Option<Target>>,
) -> impl IntoView {
    // The node's own name for itself, where it differs — what it is recognised by over there.
    let called = (n.nickname != n.petname).then(|| format!("Calls itself {}", n.nickname));
    let can = capabilities(n, now);
    let (on_right, on_more) = (n.clone(), n.clone());
    let name = n.petname.clone();
    view! {
        <li
            class="adi-new-drop__row adi-new-drop__row--device"
            title=called
            on:contextmenu=move |ev: ev::MouseEvent| {
                ev.prevent_default();
                menu.set(Some(Target::new(
                    &on_right,
                    f64::from(ev.client_x()),
                    f64::from(ev.client_y()),
                )));
            }
        >
            <span class="adi-new-drop__dot" data-tone=tone></span>
            <span class="adi-new-drop__text">
                <span class="adi-new-drop__line">
                    <span class="adi-new-drop__name">{name.clone()}</span>
                    <span class="adi-new-drop__seen">{state}</span>
                </span>
                <span class="adi-new-drop__can">
                    {can
                        .into_iter()
                        .map(|(tag, says)| view! {
                            <span class="adi-new-drop__tag" title=says>{tag}</span>
                        })
                        .collect_view()}
                </span>
            </span>
            <button
                class="adi-new-drop__more"
                type="button"
                aria-label=format!("{name} actions")
                aria-haspopup="menu"
                on:click=move |ev: ev::MouseEvent| {
                    let Some(el) = ev.current_target().and_then(|t| {
                        wasm_bindgen::JsCast::dyn_into::<web_sys::Element>(t).ok()
                    }) else {
                        return;
                    };
                    let b = el.get_bounding_client_rect();
                    menu.set(Some(Target::new(&on_more, b.left(), b.bottom() + DROP_GAP)));
                }
            >
                <Icon icon=Lucide::Ellipsis size=IconSize::Sm/>
            </button>
        </li>
    }
}

/// What a device can do, as tags — each with the sentence it stands for, shown on hover.
fn capabilities(n: &FleetNode, now: u64) -> Vec<(String, String)> {
    let mut can = Vec::new();
    if !n.grants.is_empty() {
        can.push((
            format!("Can access you · {}", seen(n.active, n.last_seen, now)),
            format!("It can access this machine: {}", n.grants.join(", ")),
        ));
    }
    if n.source {
        can.push((
            "You can access".to_string(),
            "This machine can access it".to_string(),
        ));
    }
    can
}

/// How lately a device accessed this machine, to follow "Can access you ·": `now`, `3d ago`, `never`.
fn seen(active: bool, last_seen: Option<u64>, now: u64) -> String {
    match (active, last_seen) {
        (true, _) => "now".into(),
        (false, None) => "never".into(),
        (false, Some(at)) => match now.saturating_sub(at) {
            0..=59 => "just now".into(),
            s if s < 3_600 => format!("{}m ago", s / 60),
            s if s < 86_400 => format!("{}h ago", s / 3_600),
            s => format!("{}d ago", s / 86_400),
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
    fn seen_reads_the_gap_and_trusts_active_over_it() {
        let now = 1_000_000;
        assert_eq!(seen(true, Some(0), now), "now");
        assert_eq!(seen(false, None, now), "never");
        assert_eq!(seen(false, Some(now - 90), now), "1m ago");
        assert_eq!(seen(false, Some(now - 3 * 86_400), now), "3d ago");
    }
}
