//! The top bar's sources: the paired machines, how many of them are active — and, on a click, each
//! one with its own state, dropped down under the count the way a macOS menu-bar extra drops its
//! menu.
//!
//! "Active" is the fleet's own answer (`FleetNode::active`): the machine made a request into this
//! one within the last minute (`adi_mesh::activity::ACTIVE_WINDOW_SECS`). That says nothing about
//! a machine this one could reach but that has not called in, so the list shows when each was
//! last seen as well — the count alone reads as "they are all down" when they are only quiet.

use adi_ui::{Icon, IconSize, Lucide};
use adi_webapp_api::types::FleetNode;
use leptos::{ev, html, portal::Portal, prelude::*};

use crate::fetch;

/// How often the paired machines are counted again while the list is closed. A machine coming up
/// is not urgent news, and every count is a request to the stack. Opening the list asks at once.
const TICK_MS: u32 = 30_000;

/// The gap between the top bar and the list hanging from it.
const DROP_GAP: f64 = 4.0;

/// The count in the top bar and the list it opens.
#[component]
pub(super) fn Sources(#[prop(into)] light: Signal<bool>) -> impl IntoView {
    // `None` until the fleet first answers, so the bar never claims "0 of 0" it has not been told.
    let nodes = RwSignal::new(None::<Vec<FleetNode>>);
    let open = RwSignal::new(false);
    // Where the list hangs: `(top, right)` in viewport pixels, from the button's own box.
    let at = RwSignal::new((0.0, 0.0));
    let panel: NodeRef<html::Div> = NodeRef::new();

    let load = move || {
        leptos::task::spawn_local(async move {
            // A failed recount keeps the last answer: stale by a tick beats blank.
            if let Ok(fleet) = fetch::fleet().await {
                nodes.set(Some(fleet.nodes));
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
        load();
        open.set(true);
    };

    let counts = move || {
        nodes.with(|n| {
            n.as_ref()
                .map(|n| (n.iter().filter(|n| n.active).count(), n.len()))
        })
    };

    view! {
        {move || counts().map(|(active, paired)| {
            let label = format!("{active} of {paired} paired machines active now");
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
                        {active}<span class="adi-new-top__of">"/"{paired}</span>
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
                <div class="adi-new-drop-catch" on:click=move |_| open.set(false)></div>
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
                    aria-label="Sources"
                    tabindex="-1"
                    on:keydown=move |ev: ev::KeyboardEvent| {
                        if ev.key() == "Escape" {
                            ev.prevent_default();
                            ev.stop_propagation();
                            open.set(false);
                        }
                    }
                >
                    <div class="adi-new-drop__head">
                        <span class="adi-new-drop__title">"Sources"</span>
                        <span class="adi-new-drop__meta">
                            {move || counts().map(|(a, p)| format!("{a} of {p} active"))}
                        </span>
                    </div>
                    {move || list(nodes.get())}
                    <p class="adi-new-drop__note">
                        "Active means it reached this machine in the last minute."
                    </p>
                </div>
            </Portal>
        </Show>
    }
}

/// The rows: active machines first, then the most recently seen, then the never seen.
fn list(nodes: Option<Vec<FleetNode>>) -> AnyView {
    let Some(mut nodes) = nodes else {
        return view! { <p class="adi-new-drop__empty">"Loading…"</p> }.into_any();
    };
    if nodes.is_empty() {
        return view! { <p class="adi-new-drop__empty">"No paired machines yet"</p> }.into_any();
    }
    nodes.sort_by(|a, b| {
        b.active
            .cmp(&a.active)
            .then(b.last_seen.cmp(&a.last_seen))
            .then_with(|| a.petname.cmp(&b.petname))
    });
    let now = now_unix();
    view! {
        <ul class="adi-new-drop__list">
            {nodes
                .into_iter()
                .map(|n| {
                    let seen = seen(n.active, n.last_seen, now);
                    // The node's own name for itself, where it differs from the one given it
                    // here — what someone recognises it by on the other side.
                    let called = (n.nickname != n.petname)
                        .then(|| format!("Calls itself {}", n.nickname));
                    view! {
                        <li class="adi-new-drop__row" title=called>
                            <span class="adi-new-drop__dot" class:is-on=n.active></span>
                            <span class="adi-new-drop__name">{n.petname}</span>
                            <span class="adi-new-drop__seen">{seen}</span>
                        </li>
                    }
                })
                .collect_view()}
        </ul>
    }
    .into_any()
}

/// A machine's state as a person reads it: `Active now`, `Seen 3d ago`, `Never seen`.
fn seen(active: bool, last_seen: Option<u64>, now: u64) -> String {
    match (active, last_seen) {
        (true, _) => "Active now".into(),
        (false, None) => "Never seen".into(),
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
    fn seen_reads_the_gap_and_trusts_active_over_it() {
        let now = 1_000_000;
        assert_eq!(seen(true, Some(0), now), "Active now");
        assert_eq!(seen(false, None, now), "Never seen");
        assert_eq!(seen(false, Some(now - 90), now), "Seen 1m ago");
        assert_eq!(seen(false, Some(now - 3 * 86_400), now), "Seen 3d ago");
    }
}
