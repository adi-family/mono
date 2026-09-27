//! The device window, at `/devices/<name>`: one paired device, opened from its row in the top
//! bar's list ([`super::sources`]). The window itself is [`super::windows::Frame`]'s.
//!
//! What the row leaves out lives here: whether it answers right now, the two ways the pairing
//! goes with what each grants and when it was last used, its key and pairing date — and the
//! actions: take either way away on its own, or unpair it. Every action asks first, in place of
//! the button that started it, and says whether it will end the pairing.

use adi_webapp_api::types::{FleetNode, FleetState};
use leptos::prelude::*;

use super::fleet::{Fleet, ago, now_unix, reach_label};
use super::windows::{Desk, Win};
use crate::{fetch, ui};

/// What an action takes away.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Cut {
    /// Its grants here: it stops accessing this machine.
    Viewer,
    /// This machine stops reading it, and forgets its password for it.
    Source,
    /// Both, which ends the pairing.
    Pairing,
}

#[component]
pub(super) fn Device(fleet: Fleet, desk: Desk) -> impl IntoView {
    let confirming = RwSignal::new(None::<Cut>);
    let failed = RwSignal::new(None::<String>);
    let busy = RwSignal::new(false);

    // A different device in the same window starts clean.
    Effect::new(move |_| {
        desk.device.track();
        confirming.set(None);
        failed.set(None);
    });

    let act = move |node: FleetNode, cut: Cut| {
        busy.set(true);
        failed.set(None);
        leptos::task::spawn_local(async move {
            match cut_away(&node, cut).await {
                Ok(state) => {
                    let gone = !state.nodes.iter().any(|n| n.petname == node.petname);
                    fleet.apply(state);
                    confirming.set(None);
                    if gone {
                        desk.close(Win::Device);
                    }
                }
                Err(e) => failed.set(Some(e)),
            }
            busy.set(false);
        });
    };

    move || {
        let Some(name) = desk.device.get() else {
            return view! { <p class="adi-new-win__note adi-new-device__empty">"No device chosen."</p> }
                .into_any();
        };
        match (fleet.nodes.with(Option::is_some), fleet.node(&name)) {
            (false, _) => {
                view! { <p class="adi-new-win__note adi-new-device__empty">"Loading…"</p> }
                    .into_any()
            }
            (true, None) => view! {
                <p class="adi-new-win__note adi-new-device__empty">
                    "No device called "<strong>{name}</strong>" is paired with this machine."
                </p>
            }
            .into_any(),
            (true, Some(n)) => page(n, fleet, confirming, failed, busy, act).into_any(),
        }
    }
}

fn page(
    n: FleetNode,
    fleet: Fleet,
    confirming: RwSignal<Option<Cut>>,
    failed: RwSignal<Option<String>>,
    busy: RwSignal<bool>,
    act: impl Fn(FleetNode, Cut) + Copy + Send + Sync + 'static,
) -> impl IntoView {
    let now = now_unix();
    let name = n.petname.clone();
    let (tone, state) = reach_label(fleet.reach_of(&name));
    // The node's own name for itself, where it differs — what it is recognised by over there.
    let called = (n.nickname != n.petname).then(|| n.nickname.clone());
    let viewer = !n.grants.is_empty();
    let source = n.source;

    // One confirmation block, drawn where the action that asked for it was.
    let ask = {
        let n = n.clone();
        move |cut: Cut| {
            let n = n.clone();
            move || {
                (confirming.get() == Some(cut)).then(|| {
                    confirm(n.clone(), cut, failed, busy, move || confirming.set(None), {
                        let n = n.clone();
                        move || act(n.clone(), cut)
                    })
                })
            }
        }
    };
    let button = move |cut: Cut, label: &'static str, danger: bool| {
        move || {
            (confirming.get() != Some(cut)).then(|| {
                view! {
                    <button
                        class="adi-btn adi-btn--quiet adi-new-device__act"
                        class:adi-btn--danger=danger
                        type="button"
                        on:click=move |_| {
                            failed.set(None);
                            confirming.set(Some(cut));
                        }
                    >
                        {label}
                    </button>
                }
            })
        }
    };

    view! {
        <section class="adi-new-win__section adi-new-device__head">
            <span class="adi-new-device__state">
                <span class="adi-new-drop__dot" data-tone=tone></span>
                {state}
            </span>
            {called.map(|c| view! {
                <span class="adi-new-win__note">"Calls itself "{c}</span>
            })}
        </section>

        <section class="adi-new-win__section">
            <h2 class="adi-new-win__label">"Access"</h2>
            {viewer.then(|| view! {
                <div class="adi-new-device__way">
                    <div class="adi-new-device__line">
                        <span>"Can access you"</span>
                        <span class="adi-new-device__when">
                            {format!("Last access {}", ago(n.last_seen, now))}
                        </span>
                    </div>
                    <div class="adi-new-device__grants">
                        {n.grants
                            .iter()
                            .map(|g| view! { <code class="adi-new-device__grant">{g.clone()}</code> })
                            .collect_view()}
                    </div>
                    {button(Cut::Viewer, "Stop it accessing you…", false)}
                    {ask(Cut::Viewer)}
                </div>
            })}
            {source.then(|| view! {
                <div class="adi-new-device__way">
                    <div class="adi-new-device__line">
                        <span>"You can access"</span>
                        <span class="adi-new-device__when">
                            {format!("Last connected {}", ago(n.last_reached, now))}
                        </span>
                    </div>
                    {button(Cut::Source, "Stop accessing it…", false)}
                    {ask(Cut::Source)}
                </div>
            })}
        </section>

        <section class="adi-new-win__section">
            <h2 class="adi-new-win__label">"Details"</h2>
            <dl class="adi-new-device__facts">
                <dt>"Key"</dt>
                <dd class="mono adi-new-device__key">{n.key.clone()}</dd>
                <dt>"Paired"</dt>
                <dd>{ui::fmt_date(n.paired_at)}</dd>
            </dl>
        </section>

        <section class="adi-new-win__section">
            {button(Cut::Pairing, "Unpair…", true)}
            {ask(Cut::Pairing)}
        </section>
    }
}

/// "Are you sure", about one cut: what it takes away, and whether the pairing survives it.
fn confirm(
    n: FleetNode,
    cut: Cut,
    failed: RwSignal<Option<String>>,
    busy: RwSignal<bool>,
    cancel: impl Fn() + Send + Sync + 'static,
    go: impl Fn() + Send + Sync + 'static,
) -> impl IntoView {
    let name = n.petname.clone();
    // Whether the other way survives; if not, this unpairs.
    let keeps = match cut {
        Cut::Viewer => n.source,
        Cut::Source => !n.grants.is_empty(),
        Cut::Pairing => false,
    };
    let question = match (cut, keeps) {
        (Cut::Viewer, true) => view! {
            <p class="adi-new-device__ask">
                "Stop "<strong>{name}</strong>" accessing you? It stays paired, and you can "
                "still access it."
            </p>
        }
        .into_any(),
        (Cut::Source, true) => view! {
            <p class="adi-new-device__ask">
                "Stop accessing "<strong>{name}</strong>"? It stays paired and can still access "
                "you. This machine forgets its password for it."
            </p>
        }
        .into_any(),
        (Cut::Pairing, _) => view! {
            <p class="adi-new-device__ask">
                "Unpair "<strong>{name}</strong>"? Neither machine can access the other after "
                "this: only a new invite pairs them again."
            </p>
        }
        .into_any(),
        (_, false) => view! {
            <p class="adi-new-device__ask">
                "That is the last thing "<strong>{name}</strong>" is paired for, so this unpairs "
                "it: only a new invite pairs them again."
            </p>
        }
        .into_any(),
    };
    let verb = if keeps { "Remove" } else { "Unpair" };
    view! {
        <div class="adi-new-device__confirm">
            {question}
            {move || failed.get().map(|e| view! { <p class="adi-new-device__error">{e}</p> })}
            <div class="adi-new-win__actions">
                <button class="adi-btn adi-btn--quiet" type="button" on:click=move |_| cancel()>
                    "Cancel"
                </button>
                <button
                    class="adi-btn adi-btn--danger"
                    type="button"
                    disabled=move || busy.get()
                    on:click=move |_| go()
                >
                    {verb}
                </button>
            </div>
        </div>
    }
}

/// Take one thing away, answering with the registry as it now stands. Unpairing goes through the
/// two drops rather than `/api/fleet/unpair`, because the drops are what also forget the
/// password held for the device.
async fn cut_away(n: &FleetNode, cut: Cut) -> Result<FleetState, String> {
    let name = n.petname.clone();
    match cut {
        Cut::Viewer => fetch::fleet_drop_viewer(name).await,
        Cut::Source => fetch::fleet_drop_source(name).await,
        Cut::Pairing => {
            if n.source {
                let state = fetch::fleet_drop_source(name.clone()).await?;
                if !state.nodes.iter().any(|x| x.petname == name) {
                    return Ok(state);
                }
            }
            fetch::fleet_drop_viewer(name).await
        }
    }
}
