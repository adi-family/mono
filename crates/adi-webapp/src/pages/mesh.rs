//! The Mesh page: this machine's id/ticket to share, the ports it exposes to peers, the peers
//! authorized to reach them, and the local→peer forwards.

use adi_ui::{Row as TableRow, Table};
use adi_webapp_api::types::{MeshForward, MeshForwardRef, MeshState};
use leptos::prelude::*;

use crate::fetch;
use crate::state::{MeshForm, State};
use crate::ui::{
    Key, TextField, apply_mutation, copy_row, menu_item, row_actions, rows_or_placeholder,
    sort_rows,
};

/// The exposed-ports table: one port per row, with its ⋯ menu. A single named column, so it
/// sorts but has no settings gear — there is nothing to hide or reorder.
pub(crate) const ALLOW_COLS: &[&str] = &["Port", ""];

/// The authorized-peers table. As with [`ALLOW_COLS`], one column and a menu.
pub(crate) const PEER_COLS: &[&str] = &["Endpoint id", ""];

/// The forwards table: a local listener, the peer it dials, and the port it reaches there.
pub(crate) const FORWARD_COLS: &[&str] = &["Name", "Local", "Peer", "Remote", ""];

/// The Mesh page: this machine's id/ticket to share, the ports it exposes to peers, the
/// peers authorized to reach them, and the local→peer forwards.
///
/// Starting the daemon is the screen's one orange: it is the action everything else on the page
/// waits on. While the daemon is up, nothing here is orange — its state is the green dot.
pub(crate) fn mesh_view(state: State, form: MeshForm) -> AnyView {
    let mesh = state.mesh;
    view! {
        {move || state.flash.get().map(|f| view! {
            <div class="adi-flash adi-flash--card" data-kind=f.kind>{f.msg}</div>
        })}

        <section class="adi-panel">
            <div class="adi-panel__head">
                <h2 class="adi-panel__title">"This machine"</h2>
                <span class="adi-status" data-state=move || mesh_state_data(mesh)>
                    <span class="adi-status__led"></span>
                    <span>{move || mesh.get().map_or_else(|| "\u{2026}".to_string(),
                        |m| if m.running { "daemon up".to_string() } else { "daemon down".to_string() })}</span>
                </span>
                <span class="adi-spacer"></span>
                {move || {
                    let running = mesh.get().is_some_and(|m| m.running);
                    let busy = form.busy.get();
                    if running {
                        view! {
                            <button class="adi-btn adi-btn--ghost" type="button" prop:disabled=busy
                                on:click=move |_| apply_mesh(state, Some(form.busy),
                                    "Stopped the mesh daemon.".to_string(), fetch::mesh_stop())>
                                "Stop mesh"
                            </button>
                        }.into_any()
                    } else {
                        view! {
                            <button class="adi-btn adi-btn--accent" type="button" prop:disabled=busy
                                on:click=move |_| apply_mesh(state, Some(form.busy),
                                    "Started the mesh daemon.".to_string(), fetch::mesh_start())>
                                "Start mesh"
                            </button>
                        }.into_any()
                    }
                }}
            </div>
            <div class="adi-panel__body">
                <div class="adi-field">
                    <label class="adi-field__label">"Endpoint id"</label>
                    {copy_row(form.id_ref, move || mesh.get().map(|m| m.id).unwrap_or_default())}
                    <div class="adi-field__note">"The minimal token a peer can dial; it is resolved through discovery."</div>
                </div>
                <div class="adi-field">
                    <label class="adi-field__label">"Ticket"</label>
                    {move || match mesh.get().and_then(|m| m.ticket) {
                        Some(ticket) => copy_row(form.ticket_ref, move || ticket.clone()).into_any(),
                        None => view! {
                            <div class="adi-field__note">
                                "Start the mesh daemon to publish a ticket a peer can dial without discovery."
                            </div>
                        }.into_any(),
                    }}
                    <div class="adi-field__note">"The id, the relay and the direct addresses: the reliable token to hand a peer."</div>
                </div>
            </div>
        </section>

        <section class="adi-panel">
            <div class="adi-panel__head">
                <h2 class="adi-panel__title">"Ports exposed to peers"</h2>
            </div>
            <Table state=state.tables.mesh_allow>{move || mesh_allow_rows(state)}</Table>
            <form class="adi-form" on:submit=move |ev| {
                ev.prevent_default();
                if let Some(port) = parse_port(&form.allow_port.get()) {
                    form.allow_port.set(String::new());
                    apply_mesh(state, Some(form.busy), format!("Exposed port {port} to peers."),
                        fetch::mesh_allow(port));
                }
            }>
                <TextField id="mesh-allow-port" label="Local port" placeholder="3000" numeric=true
                    value=form.allow_port />
                <button class="adi-btn adi-btn--primary" type="submit" prop:disabled=move || form.busy.get()>
                    "Expose port"
                </button>
            </form>
        </section>

        <section class="adi-panel">
            <div class="adi-panel__head">
                <h2 class="adi-panel__title">"Authorized peers"</h2>
                <span class="adi-spacer"></span>
                <span class="adi-updated">{move || mesh.get().map_or_else(String::new,
                    |m| match m.authorized_peers.len() {
                        0 => "none allowed".to_string(),
                        1 => "1 allowed".to_string(),
                        n => format!("{n} allowed"),
                    })}</span>
            </div>
            <Table state=state.tables.mesh_peers>{move || mesh_peer_rows(state)}</Table>
            <form class="adi-form" on:submit=move |ev| {
                ev.prevent_default();
                let peer = form.peer.get().trim().to_string();
                if !peer.is_empty() {
                    form.peer.set(String::new());
                    apply_mesh(state, Some(form.busy), "Authorized the peer.".to_string(),
                        fetch::mesh_allow_peer(peer));
                }
            }>
                <TextField id="mesh-peer" label="Peer id or ticket" placeholder="An endpoint id or an adimesh: ticket"
                    wide=true mono=true field_class="adi-field--grow" value=form.peer />
                <button class="adi-btn adi-btn--primary" type="submit" prop:disabled=move || form.busy.get()>
                    "Authorize peer"
                </button>
            </form>
        </section>

        <section class="adi-panel">
            <div class="adi-panel__head">
                <h2 class="adi-panel__title">"Forwards"</h2>
                <span class="adi-spacer"></span>
                <span class="adi-updated">"A local port, reaching a port on a peer"</span>
            </div>
            <Table state=state.tables.mesh_forwards>{move || mesh_forward_rows(state)}</Table>
            <form class="adi-form" on:submit=move |ev| {
                ev.prevent_default();
                let peer = form.fwd_peer.get().trim().to_string();
                match (parse_port(&form.fwd_listen.get()), parse_port(&form.fwd_port.get())) {
                    (Some(listen), Some(port)) if !peer.is_empty() => {
                        form.fwd_listen.set(String::new());
                        form.fwd_peer.set(String::new());
                        form.fwd_port.set(String::new());
                        apply_mesh(state, Some(form.busy),
                            format!("Forwarding 127.0.0.1:{listen} to the peer's {port}."),
                            fetch::mesh_add_forward(MeshForwardRef { listen, peer, port, name: None }));
                    }
                    _ => {}
                }
            }>
                <TextField id="fwd-listen" label="Local port" placeholder="5000" numeric=true value=form.fwd_listen />
                <TextField id="fwd-peer" label="Peer id or ticket" placeholder="The peer to reach" wide=true mono=true
                    field_class="adi-field--grow" value=form.fwd_peer />
                <TextField id="fwd-port" label="Remote port" placeholder="3000" numeric=true value=form.fwd_port />
                <button class="adi-btn adi-btn--primary" type="submit" prop:disabled=move || form.busy.get()>
                    "Add forward"
                </button>
            </form>
        </section>
    }
    .into_any()
}

/// The `data-state` value for the "This machine" status dot.
fn mesh_state_data(mesh: RwSignal<Option<MeshState>>) -> &'static str {
    match mesh.get() {
        Some(m) if m.running => "online",
        Some(_) => "down",
        None => "unknown",
    }
}

/// Rows for the exposed-ports table: a placeholder, or one row per allowed port with a menu
/// that stops exposing it.
fn mesh_allow_rows(state: State) -> AnyView {
    let table = state.tables.mesh_allow;
    let mut ports = match rows_or_placeholder(
        table,
        state.mesh.get().map(|v| v.allow),
        "No ports exposed — add one below to let peers reach it.",
    ) {
        Ok(rows) => rows,
        Err(placeholder) => return placeholder,
    };
    sort_rows(
        &mut ports,
        table.sort.get(),
        |p: &u16, _| Key::Int(i64::from(*p)),
        |p: &u16| Key::Int(i64::from(*p)),
    );
    ports
        .into_iter()
        .map(|port| {
            let remove = menu_item(state, "Stop exposing", true, move || {
                apply_mesh(state, None, format!("Stopped exposing port {port}."),
                    fetch::mesh_deny(port));
            });
            view! {
                <TableRow
                    state=table
                    cell=move |_| view! { <span class="adi-tabnums">{port.to_string()}</span> }.into_any()
                    actions=row_actions(state, format!("mesh-allow:{port}"), (), vec![remove])
                />
            }
            .into_any()
        })
        .collect::<Vec<_>>()
        .into_any()
}

/// Rows for the authorized-peers table: a note when the list is empty, else one row per id.
fn mesh_peer_rows(state: State) -> AnyView {
    let table = state.tables.mesh_peers;
    let mut peers = match rows_or_placeholder(
        table,
        state.mesh.get().map(|v| v.authorized_peers),
        "No peer may use the exposed ports. Add a key to allow one.",
    ) {
        Ok(rows) => rows,
        Err(placeholder) => return placeholder,
    };
    // By the full token, not its shortened rendering — two ids that abbreviate alike still order.
    sort_rows(
        &mut peers,
        table.sort.get(),
        |p: &String, _| Key::text(p),
        |p: &String| Key::text(p),
    );
    peers
        .into_iter()
        .map(|peer| {
            let full = peer.clone();
            let menu_key = format!("mesh-peer:{peer}");
            let revoke = menu_item(state, "Revoke", true, move || {
                apply_mesh(
                    state,
                    None,
                    "Revoked the peer.".to_string(),
                    fetch::mesh_deny_peer(full.clone()),
                );
            });
            view! {
                <TableRow
                    state=table
                    cell=move |_| {
                        view! {
                            <span class="adi-mono" title=peer.clone()>{short_id(&peer)}</span>
                        }
                        .into_any()
                    }
                    actions=row_actions(state, menu_key, (), vec![revoke])
                />
            }
            .into_any()
        })
        .collect::<Vec<_>>()
        .into_any()
}

/// Rows for the forwards table: a placeholder, or one row per forward with a menu to remove it.
fn mesh_forward_rows(state: State) -> AnyView {
    let table = state.tables.mesh_forwards;
    let mut forwards = match rows_or_placeholder(
        table,
        state.mesh.get().map(|v| v.forwards),
        "No forwards — add one below to reach a peer's port locally.",
    ) {
        Ok(rows) => rows,
        Err(placeholder) => return placeholder,
    };
    sort_rows(
        &mut forwards,
        table.sort.get(),
        |f, col| match col {
            "Local" => Key::Int(i64::from(f.listen)),
            "Peer" => Key::text(&f.peer),
            "Remote" => Key::Int(i64::from(f.port)),
            _ => Key::text(&f.name),
        },
        |f| Key::Int(i64::from(f.listen)),
    );
    forwards
        .into_iter()
        .map(|f| {
            let listen = f.listen;
            let remove = menu_item(state, "Remove", true, move || {
                apply_mesh(
                    state,
                    None,
                    format!("Removed the forward on 127.0.0.1:{listen}."),
                    fetch::mesh_remove_forward(listen),
                );
            });
            view! {
                <TableRow state=table cell=move |col| forward_cell(col, &f)
                    actions=row_actions(state, format!("mesh-fwd:{listen}"), (), vec![remove])/>
            }
            .into_any()
        })
        .collect::<Vec<_>>()
        .into_any()
}

/// One forward's cell under `col`. Matching the header text — the same key the sort uses — is
/// what lets the user hide and reorder columns without the row builder knowing about it.
fn forward_cell(col: &str, f: &MeshForward) -> AnyView {
    match col {
        "Local" => {
            view! { <span class="adi-mono">{format!("127.0.0.1:{}", f.listen)}</span> }.into_any()
        }
        "Peer" => view! { <span class="adi-mono" title=f.peer.clone()>{short_id(&f.peer)}</span> }
            .into_any(),
        "Remote" => view! { <span class="adi-mono">{format!(":{}", f.port)}</span> }.into_any(),
        // "Name", and anything the layout offers that this match doesn't name.
        _ => {
            if f.name.is_empty() {
                view! { <span class="adi-muted">"\u{2014}"</span> }.into_any()
            } else {
                view! { <span>{f.name.clone()}</span> }.into_any()
            }
        }
    }
}

/// Run a mesh mutation: set the returned state and a success flash, or an error flash;
/// toggles `busy` around the request when a form is driving it.
fn apply_mesh<F>(state: State, busy: Option<RwSignal<bool>>, ok_msg: String, fut: F)
where
    F: std::future::Future<Output = Result<MeshState, String>> + 'static,
{
    apply_mutation(state, busy, ok_msg, |s, m| s.mesh.set(Some(m)), fut);
}

/// Parse a `1..=65535` port from user input, rejecting blanks and `0`.
fn parse_port(raw: &str) -> Option<u16> {
    match raw.trim().parse::<u16>() {
        Ok(p) if p != 0 => Some(p),
        _ => None,
    }
}

/// A compact display for a peer token: `ticket` for a ticket, else a shortened id.
fn short_id(s: &str) -> String {
    if s.starts_with("adimesh:") {
        "ticket".to_string()
    } else if s.len() > 16 {
        format!("{}…{}", &s[..8], &s[s.len() - 4..])
    } else {
        s.to_string()
    }
}
