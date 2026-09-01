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
//! [`FleetForm::clear_invite`]: crate::state::FleetForm::clear_invite

use adi_ui::{Row as TableRow, Table};
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
pub(crate) const COLS: &[&str] = &["Node", "Key", "Grants", "Password", "Paired", ""];

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
                <TextField id="fleet-grant" label="Grant" placeholder=GRANT_PLACEHOLDER
                    wide=true mono=true field_class="adi-field--grow" value=form.grant />
                <button class="adi-btn adi-btn--primary" type="submit"
                    prop:disabled=move || form.busy.get()>
                    "Grant"
                </button>
            </form>
            <div class="adi-hint">
                "A paired node reaches nothing here until a grant says otherwise. "
                <span class="adi-mono">"http:<service>"</span>" opens one service (or "
                <span class="adi-mono">"http:*"</span>" all of them), and that is the whole of it — "
                <span class="adi-mono">"tcp:"</span>" and "<span class="adi-mono">"ctl:"</span>
                " still parse for old files but nothing enforces them, so neither opens anything."
            </div>
        </section>

        {pairing_panel(state, form)}
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
    let mut nodes = match rows_or_placeholder(
        table,
        state.fleet.get().map(|v| v.nodes),
        "No nodes paired yet — press “Show pairing QR” below.",
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
                <span>
                    <div>{name}</div>
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

/// How a node joins: a button that mints an invite and shows it as a QR, and under it the two
/// commands for a machine with no camera. Always shown — with nothing paired it is the whole page,
/// and with a fleet already running it is still how the next node arrives.
fn pairing_panel(state: State, form: FleetForm) -> AnyView {
    view! {
        <section class="adi-panel">
            <div class="adi-panel__head">
                <h2 class="adi-panel__title">
                    {move || match state.fleet.get() {
                        Some(f) if f.nodes.is_empty() => "Pair your first node",
                        _ => "Pair another node",
                    }}
                </h2>
                <span class="adi-spacer"></span>
                {move || countdown(state, form).map(|left| view! {
                    <span class="adi-chip adi-mono"
                        title="A pairing code is single-use and short-lived. When it runs out, mint another.">
                        {format!("expires in {left}")}
                    </span>
                })}
                <button class="adi-btn adi-btn--primary" type="button"
                    title="Mint a single-use invite and show it as a QR code to point a phone at."
                    prop:disabled=move || form.minting.get()
                    on:click=move |_| mint(state, form)>
                    {move || if form.invite.with(Option::is_some) {
                        "New code"
                    } else {
                        "Show pairing QR"
                    }}
                </button>
            </div>
            <div class="adi-panel__body">
                {move || invite_view(form)}
                <div class="adi-field">
                    <label class="adi-field__label">"No camera? Pair from a terminal instead"</label>
                    <div class="adi-mono">"adi-mono mesh invite"</div>
                    <div class="adi-field__note">
                        "The same token this button mints, printed on this machine. Then, on the
                         node: "<span class="adi-mono">"adi-mono mesh join <token>"</span>" — it
                         dials out, so nothing needs to be open on it. It offers a nickname, and if
                         that name is free here it becomes the petname; a clash is answered with a
                         suggestion, never a refused connection."
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

/// The minted invite: the code to point a phone at, what to point at it, and the token itself.
///
/// Renders nothing until something is minted — the panel is an instruction either way, and a QR
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
        <div class="adi-pair">
            // The server drew this, from the token, as a self-contained <svg> with no script and
            // no external reference — see `adi-webapp-api`'s `handlers::qr`. Inlined rather than
            // dropped in an <img src="data:…">, so it scales with the page and costs no request.
            <div class="adi-qr" inner_html=invite.svg></div>
            <div class="adi-pair__say">
                <p class="adi-pair__lede">
                    "On the phone, open "
                    <a href=MESH_CLIENT_URL target="_blank" rel="noreferrer">
                        "mono-mesh-client.withadi.dev"</a>
                    ", press "<strong>"Scan"</strong>", and point it at this code."
                </p>
                <div class="adi-field__note">
                    // Said out loud because the failure is silent and looks like a broken QR: the
                    // code carries the token and not a URL, on purpose — a URL would put a live
                    // credential in an address bar and a history entry.
                    "A phone's own camera app will only offer to copy this — it has to be the
                     client's own Scan button."
                </div>
                <div class="adi-field">
                    <label class="adi-field__label">"Or hand over the token"</label>
                    {copy_row(field, move || token.clone())}
                    <div class="adi-field__note">
                        "Single-use, and good for one machine only \u{2014} hand it over the way you
                         would a password."
                    </div>
                </div>
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

/// Run a fleet mutation: set the returned state and a success flash, or an error flash; toggles
/// `busy` around the request when a form is driving it. A thin typed wrapper over
/// [`apply_mutation`], as `apply_mesh` is for the mesh endpoints.
fn apply_fleet<F>(state: State, busy: Option<RwSignal<bool>>, ok_msg: String, fut: F)
where
    F: std::future::Future<Output = Result<FleetState, String>> + 'static,
{
    apply_mutation(state, busy, ok_msg, |s, f| s.fleet.set(Some(f)), fut);
}
