//! The Fleet page: the remote adi nodes this machine is paired with.
//!
//! Its whole job is to make two rules of `docs/fleet.md` legible.
//!
//! **§2 — three names, never conflated.** Every row says all three: the **petname** this machine
//! calls the node by (and routes `<service>.<petname>.n.adi` to), the **nickname** it calls
//! itself, and the **key** underneath, which is the only thing authorization is ever decided by.
//! When a paired node re-introduces itself under a *different* nickname that is filed, never
//! applied — so the page lifts it into a panel of its own at the top, with the two decisions an
//! operator can make about it. Rule 4 exists because a silent re-point would let any paired node
//! rename itself to `main` and inherit every link the real `main` had; a notification nobody is
//! shown is the same bug with extra steps.
//!
//! **§5 — default-deny.** A node's grants are what it may reach *here*, and holding none means it
//! reaches nothing. The Grants cell therefore says `none` in as many words rather than leaving an
//! empty cell to be read as "unrestricted", and the Password cell says out loud when a node has no
//! Basic-auth credential — the second, human-scoped half of the gate, without which the mesh grant
//! alone lets *any* process on a paired machine through.
//!
//! Nothing here can add a node: pairing is a two-machine act (`adi-mono mesh invite` here,
//! `adi-mono mesh join <token>` there), so the page states that instead of pretending to a button
//! it cannot honour. That panel is most of the first-run experience and is shown whether or not
//! anything is paired.

use adi_webapp_api::types::{FleetNode, FleetState};
use leptos::prelude::*;
use adi_ui::{EmptyRow, Row as TableRow, Table};

use crate::fetch;
use crate::state::{FleetForm, State};
use crate::ui::{
    Key, TextField, apply_mutation, confirm, fmt_date, menu_item,
    row_actions, sort_rows, updated_text,
};

/// The nodes table. `Node` carries the two names an operator reads (petname, then what the node
/// calls itself); `Key` is the identity of record behind them.
pub(crate) const COLS: &[&str] = &["Node", "Key", "Grants", "Password", "Paired", ""];

/// The Fleet page: pending name changes, the paired nodes with their grants, and how to pair one.
pub(crate) fn fleet_view(state: State, form: FleetForm) -> AnyView {
    let fleet = state.fleet;
    view! {
        {move || state.flash.get().map(|f| view! {
            <div class="adi-flash adi-flash--card" data-kind=f.kind>{f.msg}</div>
        })}

        {move || name_changes(state)}

        <section class="adi-panel">
            <div class="adi-panel__head">
                <h2 class="adi-panel__title">"Paired nodes"</h2>
                <span class="adi-chip adi-mono" title="Nodes paired with this machine">
                    {move || fleet.get().map_or_else(|| "\u{2014}".to_string(),
                        |f| f.nodes.len().to_string())}
                </span>
                <span class="adi-spacer"></span>
                <span class="adi-updated">{move || updated_text(fleet, state.secs_since)}</span>
            </div>

            <Table state=state.tables.fleet>{move || node_rows(state)}</Table>

            <form class="adi-form" on:submit=move |ev| {
                ev.prevent_default();
                let (node, grant) = (form.grant_node.get(), form.grant.get().trim().to_string());
                if node.is_empty() || grant.is_empty() {
                    return;
                }
                // The picked node may have been renamed or unpaired since it was chosen — from
                // this page or another tab. Ask again rather than post a name that is gone.
                if !state.fleet.get().is_some_and(|f| f.nodes.iter().any(|n| n.petname == node)) {
                    form.grant_node.set(String::new());
                    return;
                }
                form.grant.set(String::new());
                apply_fleet(state, Some(form.busy), format!("Granted {grant} to {node}."),
                    fetch::fleet_grant(node, grant));
            }>
                {node_picker(state, form)}
                <TextField id="fleet-grant" label="Grant" placeholder="http:* — or http:nosh, tcp:127.0.0.1:22, ctl:read"
                    wide=true mono=true field_class="adi-field--grow" value=form.grant />
                <button class="adi-btn adi-btn--primary" type="submit"
                    prop:disabled=move || form.busy.get()>
                    "Grant"
                </button>
            </form>
            <div class="adi-hint">
                "A paired node reaches nothing here until a grant says otherwise. "
                <span class="adi-mono">"http:<service>"</span>" opens one service (or "
                <span class="adi-mono">"http:*"</span>" all of them), "
                <span class="adi-mono">"tcp:<ip>:<port>"</span>" one raw forward, "
                <span class="adi-mono">"ctl:<scope>"</span>" the control plane."
            </div>
        </section>

        {pairing_panel(state)}
    }
    .into_any()
}

/// The panel above the table: one line per node that now calls itself something else, with the
/// only two answers there are — adopt the declared name (the petname moves with it) or keep the
/// local one (we acknowledge the change and stop being told). Renders nothing when nothing is
/// pending, so the page is quiet until a node actually renames itself.
fn name_changes(state: State) -> AnyView {
    let Some(fleet) = state.fleet.get() else {
        return ().into_any();
    };
    let changes: Vec<FleetNode> = fleet
        .nodes
        .into_iter()
        .filter(FleetNode::has_pending_nickname)
        .collect();
    if changes.is_empty() {
        return ().into_any();
    }
    let rows: Vec<AnyView> = changes.into_iter().map(|n| change_row(state, &n)).collect();
    let count = rows.len().to_string();
    view! {
        <section class="adi-panel">
            <div class="adi-panel__head">
                <h2 class="adi-panel__title">"Name changes"</h2>
                <span class="adi-chip adi-mono">{count}</span>
            </div>
            {rows}
            <div class="adi-hint">
                "A node declaring a new nickname never re-points anything on its own — the name you
                 gave it stands until you say otherwise. That is what stops a node renaming itself
                 into another node's links; the key underneath is what it was always authorized by."
            </div>
        </section>
    }
    .into_any()
}

/// One pending rename, said as a sentence, with its two decisions.
fn change_row(state: State, node: &FleetNode) -> AnyView {
    let declared = node.pending_nickname.clone().unwrap_or_default();
    let petname = node.petname.clone();
    // Both buttons name the name they land on, so neither reads as a generic "OK"/"Dismiss".
    let adopt_label = format!("Adopt \u{201c}{declared}\u{201d}");
    let keep_label = format!("Keep \u{201c}{petname}\u{201d}");
    let (accept_name, accept_declared) = (petname.clone(), declared.clone());
    let keep_name = petname.clone();
    view! {
        <div class="adi-form adi-form--toolbar">
            <span>
                "The node you call "<strong class="adi-mono">{petname}</strong>
                " now calls itself "<strong class="adi-mono">{declared}</strong>"."
            </span>
            <span class="adi-spacer"></span>
            <button class="adi-btn adi-btn--primary" type="button"
                title="Adopt the declared name: this machine calls it that from now on, and its \
                       *.n.adi hostnames move with it."
                on:click=move |_| {
                    apply_fleet(state, None,
                        format!("{accept_name} is now {accept_declared}."),
                        fetch::fleet_accept_nickname(accept_name.clone()));
                }>
                {adopt_label}
            </button>
            <button class="adi-btn adi-btn--ghost" type="button"
                title="Acknowledge the change without moving the petname \u{2014} every link you \
                       already have keeps working."
                on:click=move |_| {
                    apply_fleet(state, None,
                        format!("Noted; it is still {keep_name} here."),
                        fetch::fleet_dismiss_nickname(keep_name.clone()));
                }>
                {keep_label}
            </button>
        </div>
    }
    .into_any()
}

/// Rows for the nodes table: a placeholder while loading or when nothing is paired, else one row
/// per node with its grants and the actions that change them.
fn node_rows(state: State) -> AnyView {
    let table = state.tables.fleet;
    let Some(fleet) = state.fleet.get() else {
        return view! { <EmptyRow state=table>"Loading…"</EmptyRow> }.into_any();
    };
    if fleet.nodes.is_empty() {
        return view! { <EmptyRow state=table>"No nodes paired yet — see “Pair a node” below."</EmptyRow> }.into_any();
    }
    let mut nodes = fleet.nodes;
    sort_rows(
        &mut nodes,
        table.sort.get(),
        |n: &FleetNode, col| match col {
            // By the full key, not its shortened rendering — two keys that abbreviate alike
            // still order.
            "Key" => Key::text(&n.key),
            "Grants" => Key::count(n.grants.len()),
            "Password" => Key::Bool(n.has_password),
            "Paired" => Key::num(n.paired_at),
            _ => Key::text(&n.petname),
        },
        |n| Key::text(&n.petname),
    );
    nodes
        .into_iter()
        .map(|n| {
            let action = row_action(state, &n);
            view! { <TableRow state=table cell=move |col| cell(col, &n, state) actions=action/> }.into_any()
        })
        .collect::<Vec<_>>()
        .into_any()
}

/// One node's cell under `col`. Matching the header text — the same key the sort uses — is what
/// lets the user hide and reorder columns without the row builder knowing about it.
fn cell(col: &str, n: &FleetNode, state: State) -> AnyView {
    match col {
        "Key" => view! {
            <span class="font-mono text-meta" title=n.key.clone()>{n.key_short()}</span>
        }
        .into_any(),
        "Grants" => view! { <span>{grants_cell(state, n)}</span> }.into_any(),
        "Password" => view! { <span>{password_cell(n.has_password)}</span> }.into_any(),
        "Paired" => view! {
            <span class="font-mono text-meta" title="When this machine pinned the name to the key">
                {fmt_date(n.paired_at)}
            </span>
        }
        .into_any(),
        // "Node", and anything the layout offers that this match doesn't name: the two names,
        // stacked — what we call it, then what it calls itself.
        _ => {
            let href = format!("http://{}/", n.app_host());
            let host = n.app_host();
            view! {
                <span>
                    <div>
                        <a href=href.clone() target="_blank" rel="noreferrer"
                            title=format!("This node's control panel, over the mesh: {host}")>
                            {n.petname.clone()}
                        </a>
                    </div>
                    <div class="adi-muted">
                        "calls itself "<span class="adi-mono">{n.nickname.clone()}</span>
                    </div>
                    {pending_marker(n)}
                </span>
            }
            .into_any()
        }
    }
}

/// The row's own copy of an unacknowledged rename, so a node reads as unsettled wherever it is
/// looked at — the panel at the top is where it gets resolved.
fn pending_marker(n: &FleetNode) -> AnyView {
    let Some(declared) = n.pending_nickname.clone() else {
        return ().into_any();
    };
    view! {
        <div>
            <span class="adi-chip adi-mono"
                title="This node declared a new nickname. It changes nothing until you accept it.">
                {format!("\u{2192} {declared}?")}
            </span>
        </div>
    }
    .into_any()
}

/// A node's grants, one per line with a Revoke beside it, or the default-deny note when it holds
/// none. Said in words: an empty cell would read as "no restrictions", which is the opposite of
/// what an empty grant list means.
fn grants_cell(state: State, n: &FleetNode) -> AnyView {
    if n.grants.is_empty() {
        return view! {
            <span class="adi-muted" title="Default-deny: with no grants this node reaches nothing here.">
                "none"
            </span>
        }
        .into_any();
    }
    n.grants
        .iter()
        .map(|g| {
            let (petname, grant) = (n.petname.clone(), g.clone());
            view! {
                <div>
                    <span class="adi-chip adi-mono">{g.clone()}</span>
                    " "
                    <button class="adi-btn adi-btn--link" type="button"
                        title=format!("Revoke {g}")
                        on:click=move |_| {
                            apply_fleet(state, None,
                                format!("Revoked {grant} from {petname}."),
                                fetch::fleet_revoke(petname.clone(), grant.clone()));
                        }>
                        "Revoke"
                    </button>
                </div>
            }
            .into_any()
        })
        .collect::<Vec<_>>()
        .into_any()
}

/// Whether the node's Basic-auth credential is configured — never anything about the credential
/// itself, which the API deliberately never sends.
fn password_cell(has_password: bool) -> AnyView {
    let (state_attr, label, title) = if has_password {
        (
            "online",
            "set",
            "Requests from this node into this machine must carry its Basic-auth password.",
        )
    } else {
        (
            "down",
            "none",
            "No password: the mesh grant is all that stands between this node and what it may \
             reach here \u{2014} and a grant is machine-scoped, so it covers every process on it.",
        )
    };
    view! {
        <span class="adi-status" data-state=state_attr title=title>
            <span class="adi-status__led"></span><span>{label}</span>
        </span>
    }
    .into_any()
}

/// The row's trailing controls: Rename inline (the local rename of §2 rule 5), and a kebab with
/// Unpair — plus the two answers to a declared rename, when one is outstanding, so the row can
/// settle it without scrolling back to the panel above.
fn row_action(state: State, n: &FleetNode) -> AnyView {
    let petname = n.petname.clone();
    let rename_from = petname.clone();
    let rename = view! {
        <button class="adi-btn adi-btn--link" type="button"
            title="Rename it here only — the node is not involved, and its key does not change."
            on:click=move |_| start_rename(state, &rename_from)>
            "Rename"
        </button>
    };

    let mut items = Vec::new();
    if let Some(declared) = n.pending_nickname.clone() {
        let (adopt, keep) = (petname.clone(), petname.clone());
        let declared_label = declared.clone();
        items.push(menu_item(
            state,
            &format!("Adopt \u{201c}{declared_label}\u{201d}"),
            false,
            move || {
                apply_fleet(
                    state,
                    None,
                    format!("{adopt} is now {declared}."),
                    fetch::fleet_accept_nickname(adopt.clone()),
                );
            },
        ));
        items.push(menu_item(state, "Keep this name", false, move || {
            apply_fleet(
                state,
                None,
                format!("Noted; it is still {keep} here."),
                fetch::fleet_dismiss_nickname(keep.clone()),
            );
        }));
    }
    let unpair = petname.clone();
    items.push(menu_item(state, "Unpair", true, move || {
        if !confirm(&format!(
            "Unpair “{unpair}”? This machine forgets its key, its grants and its password, and \
             every {unpair}.n.adi name stops resolving to it."
        )) {
            return;
        }
        apply_fleet(
            state,
            None,
            format!("Unpaired {unpair}."),
            fetch::fleet_unpair(unpair.clone()),
        );
    }));

    row_actions(state, format!("fleet:{petname}"), rename, items)
}

/// Ask for the new petname and post it. A prompt rather than a form field: a rename names the row
/// it was started from, and carrying that in a form would let the row you meant and the row the
/// form remembers drift apart between the click and the submit.
fn start_rename(state: State, from: &str) {
    let Some(to) = prompt(&format!("Rename “{from}” to:"), from) else {
        return;
    };
    let to = to.trim().to_string();
    if to.is_empty() || to == from {
        return;
    }
    apply_fleet(
        state,
        None,
        format!("{from} is now {to}."),
        fetch::fleet_rename(from.to_string(), to),
    );
}

/// The grant form's node picker: which paired node the grant lands on. A `<select>` rather than a
/// typed name — every valid answer is on the page already, and a typo would be a 404 the operator
/// has to decode.
fn node_picker(state: State, form: FleetForm) -> AnyView {
    view! {
        <div class="adi-field">
            <label class="adi-field__label" for="fleet-grant-node">"Node"</label>
            <select class="adi-input" id="fleet-grant-node"
                on:change=move |ev| form.grant_node.set(event_target_value(&ev))>
                <option value="" selected=move || form.grant_node.get().is_empty()>
                    "\u{2014} pick a node \u{2014}"
                </option>
                {move || {
                    let current = form.grant_node.get();
                    state.fleet.get().map(|f| f.nodes.into_iter().map(|n| {
                        let (petname, selected) = (n.petname.clone(), n.petname == current);
                        view! { <option value=petname selected=selected>{n.petname}</option> }
                    }).collect::<Vec<_>>())
                }}
            </select>
        </div>
    }
    .into_any()
}

/// How a node joins: the two commands, in the order they are run and on the machine each is run
/// on. Always shown — with nothing paired it is the whole page, and with a fleet already running
/// it is still how the next node arrives.
fn pairing_panel(state: State) -> AnyView {
    view! {
        <section class="adi-panel">
            <div class="adi-panel__head">
                <h2 class="adi-panel__title">
                    {move || match state.fleet.get() {
                        Some(f) if f.nodes.is_empty() => "Pair your first node",
                        _ => "Pair another node",
                    }}
                </h2>
            </div>
            <div class="adi-panel__body">
                <div class="adi-field">
                    <label class="adi-field__label">"1. On this machine — mint a token"</label>
                    <div class="adi-mono">"adi-mono mesh invite"</div>
                    <div class="adi-field__note">
                        "The token carries this machine's key and how to reach it. It is
                         single-use and expires — hand it over the way you would a password."
                    </div>
                </div>
                <div class="adi-field">
                    <label class="adi-field__label">"2. On the node — redeem it"</label>
                    <div class="adi-mono">"adi-mono mesh join <token>"</div>
                    <div class="adi-field__note">
                        "The node dials out; nothing needs to be open on it. It offers a nickname,
                         and if that name is free here it becomes the petname — a clash is answered
                         with a suggestion, never a refused connection."
                    </div>
                </div>
            </div>
            <div class="adi-hint">
                "Once paired, the node's services are yours at "
                <span class="adi-mono">"<service>.<node>.n.adi"</span>" — its control panel at "
                <span class="adi-mono">"app.<node>.n.adi"</span>". Grant it what it may reach here
                 from the table above; it starts able to reach nothing."
            </div>
        </section>
    }
    .into_any()
}

/// Run a fleet mutation: set the returned state and a success flash, or an error flash; toggles
/// `busy` around the request when a form is driving it. A thin typed wrapper over
/// [`apply_mutation`], as `apply_mesh` is for the mesh endpoints.
fn apply_fleet<F>(state: State, busy: Option<RwSignal<bool>>, ok_msg: String, fut: F)
where
    F: std::future::Future<Output = Result<FleetState, String>> + 'static,
{
    apply_mutation(state, busy, ok_msg, |s, f| s.fleet.set(Some(f)), fut);
}

/// A native prompt, returning `Some` only when the user accepts with text. A browser that has no
/// `prompt` (or denies it) reads as "cancelled", so nothing is renamed by accident — the same
/// fail-quiet shape [`confirm`](crate::ui::confirm) has for the destructive actions.
fn prompt(message: &str, default: &str) -> Option<String> {
    web_sys::window()?
        .prompt_with_message_and_default(message, default)
        .ok()
        .flatten()
}
