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
//! **§13 — a node's own standing instructions.** The instructions row under the table
//! (ADI-MONO-15) edits `NodeRecord::agent_instructions`, which a fresh conversation *that node*
//! opens here splices in behind the agent's own system prompt — and, per ADI-MONO-13, only a fresh
//! one: it is spliced once, at creation, and frozen from then on, so an edit here never reaches a
//! conversation already running. The hint under the form says exactly that, because the one way
//! this setting could surprise somebody is believing it applies retroactively.
//!
//! **Pairing starts here.** A node arrives by spending an invite this machine minted (§8), and the
//! panel at the foot of the page is where one is minted: a button, then the token drawn as a QR to
//! point a phone at, the text underneath for the camera that will not cooperate, and the two
//! commands for a headless machine that has no camera to point. It is shown whether or not anything
//! is paired, because with nothing paired it is the whole page and with a fleet running it is still
//! how the next node arrives.
//!
//! An invite is a **bearer token until it is spent**, which is what the countdown and
//! [`FleetForm::clear_invite`] are about: it comes down when it expires rather than sitting there
//! as a code somebody keeps scanning, and it is dropped when the page is left.
//!
//! **Pairing also *ends* here, from the other side.** §8's handshake is symmetric — one side mints
//! and accepts, the other spends and dials — and until [`join_panel`] this page could only ever do
//! the first half. That made a terminal the price of being the dialling machine, which is exactly
//! the machine whose operator is least likely to have one: a laptop handed to somebody who was
//! sent an invite. So the paste field is the same act as `adi-mono mesh join <token>`, and it
//! answers the way that command does — with the password the handshake minted, once, because
//! neither machine keeps it.
//!
//! [`FleetForm::clear_invite`]: crate::state::FleetForm::clear_invite

use adi_ui::{Icon, IconSize, Lucide, Row as TableRow, Table};
use adi_webapp_api::types::{FleetNode, FleetState, GRANT_PLACEHOLDER, MESH_CLIENT_URL};
use leptos::prelude::*;
use wasm_bindgen_futures::spawn_local;

use crate::fetch;
use crate::state::{Flash, FleetForm, State};
use crate::ui::{
    Key, TextField, apply_mutation, confirm, copy_row, fmt_date, menu_item, prompt, row_actions,
    rows_or_placeholder, sort_rows, updated_text,
};

/// The nodes table. `Node` carries the two names an operator reads (petname, then what the node
/// calls itself); `Key` is the identity of record behind them.
pub(crate) const COLS: &[&str] = &["Node", "Key", "Grants", "Password", "Instructions", "Paired", ""];

/// The Fleet page: pending name changes, the paired nodes with their grants, and how to pair one.
pub(crate) fn fleet_view(state: State, form: FleetForm) -> AnyView {
    let fleet = state.fleet;
    // The countdown is also what takes the code down. An expired invite is refused by the minter,
    // so leaving it on screen is an offer to scan something that cannot work — and the token has no
    // reason to stay in memory a moment longer. Tracks the shell's one-second tick and nothing
    // else: reading the invite here too would make this effect its own trigger.
    Effect::new(move |_| {
        let _ = state.secs_since.get();
        if form.invite.with_untracked(Option::is_some)
            && js_sys::Date::now() >= form.invite_until.get_untracked()
        {
            form.clear_invite();
        }
    });
    // Picking a node in the instructions form loads what it already carries, so editing means
    // changing that text rather than retyping it from nothing — and picking a fresh one wipes
    // whatever was left over from the last.
    Effect::new(move |_| {
        let node = form.instructions_node.get();
        let current = state
            .fleet
            .get_untracked()
            .and_then(|f| f.nodes.into_iter().find(|n| n.petname == node))
            .and_then(|n| n.agent_instructions)
            .unwrap_or_default();
        form.instructions.set(current);
    });
    view! {
        {move || state.flash.get().map(|f| view! {
            <div class="adi-flash adi-flash--card" data-kind=f.kind>{f.msg}</div>
        })}

        {move || name_changes(state)}

        <section class="adi-panel">
            <div class="adi-panel__head">
                <h2 class="adi-panel__title">"Paired nodes"</h2>
                <span class="adi-updated" title="Nodes paired with this machine">
                    {move || fleet.get().map_or_else(|| "\u{2014}".to_string(),
                        |f| format!("{} paired", f.nodes.len()))}
                </span>
                <span class="adi-spacer"></span>
                <span class="adi-updated">{move || updated_text(fleet, state.secs_since)}</span>
            </div>

            <Table state=state.tables.fleet>{move || node_rows(state)}</Table>

            // The grant row: a node, a grant, one plain button. The action lives under the table
            // it changes, and the page's main action is pairing, which is why this is not it.
            <form class="adi-fleet-grantrow" on:submit=move |ev| {
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
                {node_picker(state, "fleet-grant-node", form.grant_node)}
                <TextField id="fleet-grant" label="Grant" placeholder=GRANT_PLACEHOLDER
                    wide=true mono=true value=form.grant />
                <button class="adi-btn" type="submit" prop:disabled=move || form.busy.get()>
                    "Grant"
                </button>
            </form>
            <div class="adi-hint">
                "A paired node reaches nothing here until a grant says otherwise. "
                <code>"http:<service>"</code>" opens one service (or "<code>"http:*"</code>
                " all of them), and that is the whole of it — "<code>"tcp:"</code>" and "
                <code>"ctl:"</code>" still parse for old files but nothing enforces them, so
                 neither opens anything."
            </div>

            // The node's own standing instructions (ADI-MONO-15): a second row, the same shape as
            // the grant row above, because it names a node and edits one free-text setting on it.
            <form class="adi-fleet-grantrow" on:submit=move |ev| {
                ev.prevent_default();
                let node = form.instructions_node.get();
                if node.is_empty()
                    || !state.fleet.get().is_some_and(|f| f.nodes.iter().any(|n| n.petname == node))
                {
                    form.instructions_node.set(String::new());
                    return;
                }
                let instructions = form.instructions.get();
                let msg = if instructions.trim().is_empty() {
                    format!("Cleared {node}'s agent instructions.")
                } else {
                    format!("Updated {node}'s agent instructions.")
                };
                apply_fleet(state, Some(form.busy), msg,
                    fetch::fleet_instructions(node, instructions));
            }>
                {node_picker(state, "fleet-instructions-node", form.instructions_node)}
                <TextField id="fleet-instructions" label="Agent instructions"
                    placeholder="Always run the tests before answering…" wide=true
                    value=form.instructions />
                <button class="adi-btn" type="submit" prop:disabled=move || form.busy.get()>
                    "Save"
                </button>
            </form>
            <div class="adi-hint">
                "Spliced behind this agent's own system prompt, once, the moment a fresh
                 conversation from this node is opened here — never into one already running,
                 so a change here only ever reaches the next conversation that node starts.
                 Leave it blank and save to clear it."
            </div>
        </section>

        {pairing_panel(state, form)}
        {join_panel(state, form)}
    }
    .into_any()
}

/// The section above the table: one line per node that now calls itself something else, with the
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
    let count = changes.len();
    let rows: Vec<AnyView> = changes.into_iter().map(|n| change_row(state, &n)).collect();
    view! {
        <section class="adi-panel">
            <div class="adi-panel__head">
                <h2 class="adi-panel__title">"Name changes"</h2>
                <span class="adi-updated">{format!("{count} waiting on you")}</span>
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

/// One pending rename, said as a sentence, with its two decisions. Adopting is the change and
/// takes the page's ink; keeping is the quiet answer.
fn change_row(state: State, node: &FleetNode) -> AnyView {
    let declared = node.pending_nickname.clone().unwrap_or_default();
    let petname = node.petname.clone();
    // Both buttons name the name they land on, so neither reads as a generic "OK"/"Dismiss".
    let adopt_label = format!("Adopt \u{201c}{declared}\u{201d}");
    let keep_label = format!("Keep \u{201c}{petname}\u{201d}");
    let (accept_name, accept_declared) = (petname.clone(), declared.clone());
    let keep_name = petname.clone();
    view! {
        <div class="adi-fleet-change">
            <span>
                "The node you call "<b>{petname}</b>" now calls itself "<b>{declared}</b>"."
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
/// per node with its grants and the ⋯ menu that changes them.
fn node_rows(state: State) -> AnyView {
    let table = state.tables.fleet;
    let mut nodes = match rows_or_placeholder(
        table,
        state.fleet.get().map(|v| v.nodes),
        "No nodes paired yet — mint a pairing code below.",
    ) {
        Ok(rows) => rows,
        Err(placeholder) => return placeholder,
    };
    sort_rows(
        &mut nodes,
        table.sort.get(),
        |n: &FleetNode, col| match col {
            // By the full key, not its shortened rendering — two keys that abbreviate alike
            // still order.
            "Key" => Key::text(&n.key),
            "Grants" => Key::count(n.grants.len()),
            "Password" => Key::Bool(n.has_password),
            "Instructions" => Key::Bool(n.agent_instructions.is_some()),
            "Paired" => Key::num(n.paired_at),
            _ => Key::text(&n.petname),
        },
        |n| Key::text(&n.petname),
    );
    nodes
        .into_iter()
        .map(|n| {
            let action = row_action(state, &n);
            view! { <TableRow state=table cell=move |col| cell(col, &n, state) actions=action/> }
                .into_any()
        })
        .collect::<Vec<_>>()
        .into_any()
}

/// One node's cell under `col`. Matching the header text — the same key the sort uses — is what
/// lets the user hide and reorder columns without the row builder knowing about it.
fn cell(col: &str, n: &FleetNode, state: State) -> AnyView {
    match col {
        // The key is the identity of record, shortened to its ends: mono, one step dimmer than
        // the name, the whole of it in the hover.
        "Key" => view! {
            <span class="adi-mono adi-muted" title=n.key.clone()>{n.key_short()}</span>
        }
        .into_any(),
        "Grants" => view! { <span>{grants_cell(state, n)}</span> }.into_any(),
        "Password" => view! { <span>{password_cell(n.has_password)}</span> }.into_any(),
        "Instructions" => view! { <span>{instructions_cell(n)}</span> }.into_any(),
        // A date is not a machine string: sans, and dimmed, since it says the same kind of thing
        // in every row.
        "Paired" => view! {
            <span class="adi-muted adi-tabnums"
                title="When this machine pinned the name to the key">
                {fmt_date(n.paired_at)}
            </span>
        }
        .into_any(),
        // "Node", and anything the layout offers that this match doesn't name: the two names,
        // stacked — what we call it, then what it calls itself.
        _ => {
            let host = n.app_host();
            // The petname is this machine's alone (§2), so the address exists only from here —
            // read through a node, the name stays and the link goes.
            let name = match crate::origin::service_url(&host) {
                Some(href) => view! {
                    <a href=href target="_blank" rel="noreferrer"
                        title=format!("This node's control panel, over the mesh: {host}")>
                        {n.petname.clone()}
                    </a>
                }
                .into_any(),
                None => view! { <span>{n.petname.clone()}</span> }.into_any(),
            };
            view! {
                <span class="adi-fleet-node">
                    <b>{name}</b>
                    <span>{format!("calls itself {}", n.nickname)}</span>
                    {pending_marker(n)}
                </span>
            }
            .into_any()
        }
    }
}

/// The row's own copy of an unacknowledged rename, so a node reads as unsettled wherever it is
/// looked at — the section at the top is where it gets resolved.
fn pending_marker(n: &FleetNode) -> AnyView {
    let Some(declared) = n.pending_nickname.clone() else {
        return ().into_any();
    };
    view! {
        <span class="adi-fleet-node__pending"
            title="This node declared a new nickname. It changes nothing until you accept it.">
            {format!("now calls itself {declared}?")}
        </span>
    }
    .into_any()
}

/// A node's grants as pills, each with its × inside — the remove action lives in the object, so
/// the word "Revoke" is never repeated per row (§6) — or the default-deny note when it holds none.
/// Said in words: an empty cell would read as "no restrictions", which is the opposite of what an
/// empty grant list means.
fn grants_cell(state: State, n: &FleetNode) -> AnyView {
    if n.grants.is_empty() {
        return view! {
            <span class="adi-muted" title="Default-deny: with no grants this node reaches nothing here.">
                "none"
            </span>
        }
        .into_any();
    }
    let pills = n
        .grants
        .iter()
        .map(|g| {
            let (petname, grant) = (n.petname.clone(), g.clone());
            let label = format!("Revoke {g}");
            view! {
                <span class="adi-fleet-grant">
                    {g.clone()}
                    <button type="button" title=label.clone() aria-label=label
                        on:click=move |_| {
                            apply_fleet(state, None,
                                format!("Revoked {grant} from {petname}."),
                                fetch::fleet_revoke(petname.clone(), grant.clone()));
                        }>
                        <Icon icon=Lucide::X size=IconSize::Sm/>
                    </button>
                </span>
            }
        })
        .collect::<Vec<_>>();
    view! { <span class="adi-fleet-grants">{pills}</span> }.into_any()
}

/// Whether the node's Basic-auth credential is configured — never anything about the credential
/// itself, which the API deliberately never sends. A green dot and `set`, or a dash: the absence
/// is the thing to notice, and the hover says why.
fn password_cell(has_password: bool) -> AnyView {
    if has_password {
        return view! {
            <span class="adi-status" data-state="online"
                title="Requests from this node into this machine must carry its Basic-auth password.">
                <span class="adi-status__led"></span><span>"set"</span>
            </span>
        }
        .into_any();
    }
    view! {
        <span class="adi-muted"
            title="No password: the mesh grant is all that stands between this node and what it may \
                   reach here \u{2014} and a grant is machine-scoped, so it covers every process on it.">
            "\u{2014}"
        </span>
    }
    .into_any()
}

/// Whether a node carries standing agent instructions (ADI-MONO-15) — never the text itself, which
/// can run long and belongs in the edit field below the table, not a table cell. The full text
/// rides the hover, the same way the Key cell's full key does.
fn instructions_cell(n: &FleetNode) -> AnyView {
    match n.agent_instructions.as_deref() {
        Some(text) => view! {
            <span class="adi-muted" title=text.to_string()>"set"</span>
        }
        .into_any(),
        None => view! {
            <span class="adi-muted"
                title="Nothing spliced into a conversation this node opens here.">
                "\u{2014}"
            </span>
        }
        .into_any(),
    }
}

/// The row's ⋯ menu: Rename (the local rename of §2 rule 5), the two answers to a declared rename
/// when one is outstanding — so the row can settle it without scrolling back to the section above
/// — and Unpair.
fn row_action(state: State, n: &FleetNode) -> AnyView {
    let petname = n.petname.clone();
    let rename_from = petname.clone();
    let mut items = vec![menu_item(state, "Rename\u{2026}", false, move || {
        start_rename(state, &rename_from);
    })];
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

    row_actions(state, format!("fleet:{petname}"), (), items)
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

/// A node picker: which paired node an under-table form's action lands on. A `<select>` rather
/// than a typed name — every valid answer is on the page already, and a typo would be a 404 the
/// operator has to decode. Shared by the grant row and the agent-instructions row, each with its
/// own `id` (for the `<label for>`) and its own signal, so picking a node in one form never moves
/// what the other is doing.
fn node_picker(state: State, id: &'static str, node: RwSignal<String>) -> AnyView {
    view! {
        <div class="adi-field">
            <label class="adi-field__label" for=id>"Node"</label>
            <select class="adi-input adi-input--wide" id=id
                on:change=move |ev| node.set(event_target_value(&ev))>
                <option value="" selected=move || node.get().is_empty()>
                    "Pick a node"
                </option>
                {move || {
                    let current = node.get();
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

/// How a node joins: the one card on the page (§5 — a pairing block is genuinely detachable).
/// Its head carries the countdown and the button that mints; its body is the QR beside what to do
/// with it, once one is minted; its foot is the two commands for a machine with no camera.
/// Always shown — with nothing paired it is the whole page, and with a fleet already running it is
/// still how the next node arrives.
fn pairing_panel(state: State, form: FleetForm) -> AnyView {
    view! {
        <div class="adi-fleet-card">
            <div class="adi-fleet-card__head">
                <span>
                    {move || match state.fleet.get() {
                        Some(f) if f.nodes.is_empty() => "Pair your first node",
                        _ => "Pair another node",
                    }}
                </span>
                <div class="adi-fleet-card__meta">
                    {move || countdown(state, form).map(|left| view! {
                        <span title="A pairing code is single-use and short-lived. When it runs out, mint another.">
                            {format!("expires in {left}")}
                        </span>
                    })}
                    <button class="adi-btn" type="button"
                        title="Mint a single-use invite and show it as a QR code to point a phone at."
                        prop:disabled=move || form.minting.get()
                        on:click=move |_| mint(state, form)>
                        {move || if form.invite.with(Option::is_some) {
                            "New code"
                        } else {
                            "Show pairing code"
                        }}
                    </button>
                </div>
            </div>
            {move || invite_view(form)}
            <div class="adi-fleet-card__foot">
                <span class="adi-field__label">"No camera? Pair from a terminal instead"</span>
                <p>
                    <code>"adi-mono mesh invite"</code>" prints the same token this card mints. Then,
                     on the node: "<code>"adi-mono mesh join <token>"</code>" — it dials out, so
                     nothing needs to be open on it. It offers a nickname, and if that name is free
                     here it becomes the petname; a clash is answered with a suggestion, never a
                     refused connection."
                </p>
                <p>
                    "Once paired, the node's services are yours at "
                    <code>"<service>.<node>.n.adi"</code>" — its control panel at "
                    <code>"app.<node>.n.adi"</code>". Grant it what it may reach here from the
                     table above; it starts able to reach nothing."
                </p>
            </div>
        </div>
    }
    .into_any()
}

/// The other direction: an invite minted somewhere else, pasted here and spent.
///
/// A section of its own rather than a second button in [`pairing_panel`], because it is the
/// opposite act on the opposite machine — this one is not enrolling a node, it is *being*
/// enrolled — and the two are one mistaken click apart. The distinction the operator has to hold
/// is which machine can be dialled: whoever can be, mints (§8).
fn join_panel(state: State, form: FleetForm) -> AnyView {
    view! {
        <section class="adi-panel">
            <div class="adi-panel__head">
                <h2 class="adi-panel__title">"Join a fleet"</h2>
                <span class="adi-spacer"></span>
                {move || form.joining.get().then(|| view! {
                    <span class="adi-updated"
                        title="Dialling the machine that minted this invite, over the mesh.">
                        "dialling\u{2026}"
                    </span>
                })}
            </div>
            <form class="adi-fleet-joinrow" on:submit=move |ev| {
                ev.prevent_default();
                join(state, form);
            }>
                <TextField id="fleet-join" label="Invite token"
                    placeholder="adi-invite:\u{2026}" wide=true mono=true value=form.join_token />
                <button class="adi-btn adi-btn--primary" type="submit"
                    prop:disabled=move || form.joining.get()
                        || form.join_token.with(|t| t.trim().is_empty())>
                    "Join"
                </button>
            </form>
            <div class="adi-hint">
                "Minted on the machine you want to pair with \u{2014} its own Fleet page, or "
                <code>"adi-mono mesh invite"</code>" there. This machine dials out, so nothing has
                 to be open here. Same token either way: this field is "
                <code>"adi-mono mesh join"</code>" without the terminal. Pairing is mutual: the far
                 side files this machine with "<code>"http:app"</code>" \u{2014} its panel, and
                 nothing else here \u{2014} and this machine files it the same way, under the name
                 it appears by in the table above. Both directions are gated by the one password
                 below."
            </div>
            {move || joined_view(form)}
        </section>
    }
    .into_any()
}

/// What the last spent invite bought: the two names, the credential, and the link it opens.
///
/// Rendered only after a join, and never re-fetched — there is nothing to re-fetch. The password
/// exists in plaintext exactly once, in this response, on this screen; both machines keep only a
/// salted verifier. So this says so in as many words rather than leaving an operator to discover
/// it by navigating away.
fn joined_view(form: FleetForm) -> AnyView {
    let Some(joined) = form.joined.get() else {
        return ().into_any();
    };
    // A ref per render, as in `invite_view`: it reaches only the field drawn beside it, which is
    // the one holding the credential now on screen.
    let field = NodeRef::new();
    let password = joined.password.clone();
    let host = joined.app_host();
    let url = format!("http://{host}/");
    let shown = url.clone();
    view! {
        <div class="adi-fleet-joined">
            <p>
                "Paired with "<b>{joined.viewer.clone()}</b>", which files this machine as "
                <code>{joined.petname.clone()}</code>"."
            </p>
            <div class="adi-field">
                <span class="adi-field__label">
                    {format!("Password for {} at {host}", joined.username)}
                </span>
                {copy_row(field, move || password.clone())}
                <div class="adi-field__note">
                    "Copy it now: it is stored nowhere, on either machine \u{2014} only a salted
                     verifier is. Lose it and you re-pair, which mints a new one."
                </div>
            </div>
            <div class="adi-field">
                <span class="adi-field__label">"Its control panel"</span>
                <div>
                    <a href=url target="_blank" rel="noreferrer" class="adi-mono">{shown}</a>
                </div>
                <div class="adi-field__note">
                    "Give it about five seconds \u{2014} the far side's gateway serves from a
                     snapshot of its registry and re-reads it on an interval, so the first request
                     can arrive before the pairing does. "<code>"https://"</code>" warns until this
                     machine's front door is next started; "<code>"http://"</code>" works now."
                </div>
            </div>
        </div>
    }
    .into_any()
}

/// The minted invite: the code to point a phone at, what to point at it, and the token itself.
///
/// Renders nothing until something is minted — the card is an instruction either way, and a QR
/// drawn before it is asked for would be a live credential on screen for anyone who walked past.
fn invite_view(form: FleetForm) -> AnyView {
    let Some(invite) = form.invite.get() else {
        return ().into_any();
    };
    // A node ref per render, and a fresh one per invite: it only ever reaches the input rendered
    // beside it, which is the field holding the token now on screen.
    let field = NodeRef::new();
    let token = invite.token.clone();
    view! {
        <div class="adi-fleet-card__body">
            // The server drew this, from the token, as a self-contained <svg> with no script and
            // no external reference — see `adi-webapp-api`'s `handlers::qr`. Inlined rather than
            // dropped in an <img src="data:…">, so it scales with the page and costs no request.
            <div class="adi-fleet-qr" inner_html=invite.svg></div>
            <div class="adi-fleet-say">
                <p>
                    "On the phone, open "
                    <a href=MESH_CLIENT_URL target="_blank" rel="noreferrer">
                        "mono-mesh-client.withadi.dev"</a>
                    ", press "<b>"Scan"</b>", and point it at this code."
                </p>
                // Said out loud because the failure is silent and looks like a broken QR: the
                // code carries the token and not a URL, on purpose — a URL would put a live
                // credential in an address bar and a history entry.
                <p class="adi-fleet-say__sm">
                    "A phone's own camera app will only offer to copy this — it has to be the
                     client's own Scan button."
                </p>
                <span class="adi-field__label adi-fleet-say__label">"Or hand over the token"</span>
                {copy_row(field, move || token.clone())}
                <p class="adi-fleet-say__sm">
                    "Single-use, and good for one machine only \u{2014} hand it over the way you
                     would a password."
                </p>
            </div>
        </div>
    }
    .into_any()
}

/// Time left on the invite on screen as `m:ss`, or `None` when there is none to count.
///
/// Subscribes to the shell's one-second tick, which is what makes it tick at all, and measures
/// against `invite_until` — this browser's clock, captured when the answer landed.
fn countdown(state: State, form: FleetForm) -> Option<String> {
    let _ = state.secs_since.get();
    form.invite.with(Option::is_some).then_some(())?;
    // Whole seconds, rounded up, so a fresh ten-minute invite reads `10:00` rather than `9:59`.
    let left = ((form.invite_until.get() - js_sys::Date::now()) / 1000.0).ceil();
    if left <= 0.0 {
        return None;
    }
    let (mins, secs) = ((left / 60.0).floor(), left % 60.0);
    Some(format!("{mins:.0}:{secs:02.0}"))
}

/// Mint an invite and put it on screen.
///
/// Written out rather than run through [`apply_mutation`] for two reasons: a success here has no
/// flash — the code appearing *is* the answer, and a second "done" line above it would be noise —
/// and the deadline has to be captured from `ttl_secs` at the moment the answer lands.
///
/// Reachable from outside this page because the ⌘K menu's **Pair new device** row is the same
/// press as the button above: it lands on this page and raises the QR, rather than navigating
/// near it and leaving the button to be found. The caller has to arrive here *first* — the shell
/// clears the invite whenever the route is not Fleet, so a mint that ran before the navigation
/// would be swept away by it (see [`crate::menu::Shell::pair`]).
pub(crate) fn mint(state: State, form: FleetForm) {
    if form.minting.get_untracked() {
        return;
    }
    form.minting.set(true);
    spawn_local(async move {
        match fetch::fleet_invite().await {
            Ok(invite) => {
                // The deadline is captured here, from the TTL, rather than read from the token's
                // absolute `expires`: the panel may be being read over the mesh from a machine
                // whose clock is not this one's, and a countdown must never call a live invite dead.
                form.invite_until
                    .set(js_sys::Date::now() + f64::from(invite.ttl_secs) * 1000.0);
                form.invite.set(Some(invite));
                // A previous refusal ("the mesh is not running here") is answered by this code
                // existing; leaving it above would contradict what the page now shows.
                state.flash.set(None);
            }
            Err(e) => state.flash.set(Some(Flash::err(e))),
        }
        form.minting.set(false);
    });
}

/// Spend the pasted invite.
///
/// Written out rather than run through [`apply_mutation`] for the same shape of reason as [`mint`],
/// with one addition: the answer carries *two* things — the fresh registry, which belongs in
/// `state.fleet` like any other mutation's, and a credential, which belongs on screen and nowhere
/// else. The token field is cleared on success and only on success: a spent invite cannot be spent
/// again, while a refused one is usually worth a second press (the mesh was still starting, the
/// far side was not up yet), and clearing it would make the operator go and ask for the token
/// again.
fn join(state: State, form: FleetForm) {
    let token = form.join_token.get_untracked().trim().to_string();
    if token.is_empty() || form.joining.get_untracked() {
        return;
    }
    form.joining.set(true);
    // A previous credential belongs to a previous pairing: leaving it under a form that is now
    // dialling somewhere else would attach it to the wrong machine.
    form.joined.set(None);
    spawn_local(async move {
        match fetch::fleet_join(token).await {
            Ok(joined) => {
                let msg = format!("Joined {}'s fleet as {}.", joined.viewer, joined.petname);
                state.fleet.set(Some(joined.fleet.clone()));
                form.join_token.set(String::new());
                form.joined.set(Some(joined));
                state.flash.set(Some(Flash::ok(msg)));
            }
            Err(e) => state.flash.set(Some(Flash::err(e))),
        }
        form.joining.set(false);
    });
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
