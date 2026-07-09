//! The Mesh page: this machine's id/ticket to share, the ports it exposes to peers, the peers
//! authorized to reach them, and the local→peer forwards.

use adi_webapp_api::types::{MeshForwardRef, MeshState};
use leptos::prelude::*;
use wasm_bindgen::JsCast;

use crate::fetch;
use crate::state::{MeshForm, State};
use crate::ui::{TextField, apply_mutation, data_table, placeholder_row, tile};

/// The Mesh page: this machine's id/ticket to share, the ports it exposes to peers, the
/// peers authorized to reach them, and the local→peer forwards.
pub(crate) fn mesh_view(state: State, form: MeshForm) -> AnyView {
    let mesh = state.mesh;
    view! {
        <section class="adi-tiles">
            {tile("Daemon",
                move || mesh.get().map_or_else(|| "—".to_string(),
                    |m| if m.running { "running".to_string() } else { "stopped".to_string() }),
                "runs adi-mesh; publishes a ticket while up")}
            {tile("Ports exposed",
                move || mesh.get().map_or_else(|| "—".to_string(), |m| m.allow.len().to_string()),
                "reachable by peers")}
            {tile("Forwards",
                move || mesh.get().map_or_else(|| "—".to_string(), |m| m.forwards.len().to_string()),
                "local → peer tunnels")}
        </section>

        {move || state.flash.get().map(|f| view! {
            <div class="adi-flash adi-flash--card" data-kind=f.kind>{f.msg}</div>
        })}

        <section class="adi-panel">
            <div class="adi-panel__head">
                <h2 class="adi-panel__title">"This machine"</h2>
                <span class="adi-spacer"></span>
                <span class="adi-status" data-state=move || mesh_state_data(mesh)>
                    <span class="adi-status__led"></span>
                    <span>{move || mesh.get().map_or_else(|| "…".to_string(),
                        |m| if m.running { "daemon up".to_string() } else { "daemon down".to_string() })}</span>
                </span>
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
                            <button class="adi-btn adi-btn--primary" type="button" prop:disabled=busy
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
                    <label class="adi-field__label">"Endpoint ID"</label>
                    {copy_row(form.id_ref, move || mesh.get().map(|m| m.id).unwrap_or_default())}
                    <div class="adi-field__hint">"The minimal token a peer can dial (resolved via discovery)."</div>
                </div>
                <div class="adi-field">
                    <label class="adi-field__label">"Ticket"</label>
                    {move || match mesh.get().and_then(|m| m.ticket) {
                        Some(ticket) => copy_row(form.ticket_ref, move || ticket.clone()).into_any(),
                        None => view! {
                            <div class="adi-field__hint adi-muted">
                                "Start the mesh daemon (the "<strong>"Start mesh"</strong>" button above) to publish a ticket a peer can dial without discovery."
                            </div>
                        }.into_any(),
                    }}
                    <div class="adi-field__hint">"id + relay + direct addresses — the reliable token to hand a peer."</div>
                </div>
            </div>
        </section>

        <section class="adi-panel">
            <div class="adi-panel__head">
                <h2 class="adi-panel__title">"Ports exposed to peers"</h2>
            </div>
            {data_table(&["Port", ""], move || mesh_allow_rows(state))}
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
                    |m| if m.authorized_peers.is_empty() { "any peer allowed".to_string() }
                        else { format!("{} allowed", m.authorized_peers.len()) })}</span>
            </div>
            {data_table(&["Endpoint ID", ""], move || mesh_peer_rows(state))}
            <form class="adi-form" on:submit=move |ev| {
                ev.prevent_default();
                let peer = form.peer.get().trim().to_string();
                if !peer.is_empty() {
                    form.peer.set(String::new());
                    apply_mesh(state, Some(form.busy), "Authorized the peer.".to_string(),
                        fetch::mesh_allow_peer(peer));
                }
            }>
                <TextField id="mesh-peer" label="Peer id or ticket" placeholder="an EndpointId or adimesh: ticket"
                    wide=true mono=true field_style="flex:1 1 240px; min-width:0" value=form.peer />
                <button class="adi-btn adi-btn--primary" type="submit" prop:disabled=move || form.busy.get()>
                    "Authorize peer"
                </button>
            </form>
        </section>

        <section class="adi-panel">
            <div class="adi-panel__head">
                <h2 class="adi-panel__title">"Forwards"</h2>
                <span class="adi-spacer"></span>
                <span class="adi-updated">"local 127.0.0.1:port → a peer's port"</span>
            </div>
            {data_table(&["Name", "Local", "Peer", "Remote", ""], move || mesh_forward_rows(state))}
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
                <TextField id="fwd-peer" label="Peer id or ticket" placeholder="peer to reach" wide=true mono=true
                    field_style="flex:1 1 220px; min-width:0" value=form.fwd_peer />
                <TextField id="fwd-port" label="Remote port" placeholder="3000" numeric=true value=form.fwd_port />
                <button class="adi-btn adi-btn--primary" type="submit" prop:disabled=move || form.busy.get()>
                    "Add forward"
                </button>
            </form>
        </section>
    }
    .into_any()
}

/// The `data-state` value for the "This machine" status pill.
fn mesh_state_data(mesh: RwSignal<Option<MeshState>>) -> &'static str {
    match mesh.get() {
        Some(m) if m.running => "online",
        Some(_) => "down",
        None => "unknown",
    }
}

/// Rows for the exposed-ports table: a placeholder, or one row per allowed port with a
/// button to stop exposing it.
fn mesh_allow_rows(state: State) -> AnyView {
    let Some(mesh) = state.mesh.get() else {
        return placeholder_row("2", "Loading…");
    };
    if mesh.allow.is_empty() {
        return placeholder_row(
            "2",
            "No ports exposed — add one below to let peers reach it.",
        );
    }
    let mut ports = mesh.allow;
    ports.sort_unstable();
    ports
        .into_iter()
        .map(|port| {
            view! {
                <tr>
                    <td class="adi-mono adi-table__port">{port.to_string()}</td>
                    <td style="text-align:right">
                        <button class="adi-btn adi-btn--link" on:click=move |_| {
                            apply_mesh(state, None, format!("Stopped exposing port {port}."),
                                fetch::mesh_deny(port));
                        }>"Remove"</button>
                    </td>
                </tr>
            }
        })
        .collect::<Vec<_>>()
        .into_any()
}

/// Rows for the authorized-peers table: a note when open to any peer, else one row per id.
fn mesh_peer_rows(state: State) -> AnyView {
    let Some(mesh) = state.mesh.get() else {
        return placeholder_row("2", "Loading…");
    };
    if mesh.authorized_peers.is_empty() {
        return placeholder_row(
            "2",
            "Any peer may use the exposed ports. Add one to restrict access.",
        );
    }
    mesh.authorized_peers
        .into_iter()
        .map(|peer| {
            let full = peer.clone();
            view! {
                <tr>
                    <td class="adi-mono" title=full.clone()>{short_id(&peer)}</td>
                    <td style="text-align:right">
                        <button class="adi-btn adi-btn--link" on:click=move |_| {
                            apply_mesh(state, None, "Revoked the peer.".to_string(),
                                fetch::mesh_deny_peer(full.clone()));
                        }>"Revoke"</button>
                    </td>
                </tr>
            }
        })
        .collect::<Vec<_>>()
        .into_any()
}

/// Rows for the forwards table: a placeholder, or one row per forward with a remove button.
fn mesh_forward_rows(state: State) -> AnyView {
    let Some(mesh) = state.mesh.get() else {
        return placeholder_row("5", "Loading…");
    };
    if mesh.forwards.is_empty() {
        return placeholder_row(
            "5",
            "No forwards — add one below to reach a peer's port locally.",
        );
    }
    mesh.forwards
        .into_iter()
        .map(|f| {
            let listen = f.listen;
            view! {
                <tr>
                    <td>{f.name}</td>
                    <td class="adi-mono adi-table__port">{format!("127.0.0.1:{}", f.listen)}</td>
                    <td class="adi-mono" title=f.peer.clone()>{short_id(&f.peer)}</td>
                    <td class="adi-mono">{format!(":{}", f.port)}</td>
                    <td style="text-align:right">
                        <button class="adi-btn adi-btn--link" on:click=move |_| {
                            apply_mesh(state, None, format!("Removed the forward on 127.0.0.1:{listen}."),
                                fetch::mesh_remove_forward(listen));
                        }>"Remove"</button>
                    </td>
                </tr>
            }
        })
        .collect::<Vec<_>>()
        .into_any()
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

/// A read-only field with a Copy button (the mesh id/ticket rows): selects on focus and copies to
/// the clipboard. `node` lets the button reach the input's live text.
fn copy_row(
    node: NodeRef<leptos::html::Input>,
    value: impl Fn() -> String + Send + 'static,
) -> impl IntoView {
    view! {
        <div class="adi-copyrow">
            <input class="adi-input adi-input--wide adi-mono" readonly=true node_ref=node
                prop:value=value
                on:focus=move |ev| select_target(&ev) />
            <button class="adi-btn adi-btn--ghost" type="button"
                on:click=move |_| copy_field(node)>"Copy"</button>
        </div>
    }
}

/// Copy a read-only field's text to the clipboard: select it (a visible affordance and a
/// manual-copy fallback), then write it via `navigator.clipboard` on wasm. Best-effort.
fn copy_field(node: NodeRef<leptos::html::Input>) {
    if let Some(input) = node.get() {
        input.select();
        #[cfg(target_arch = "wasm32")]
        clipboard_write(&input.value());
    }
}

/// One-click clipboard write via `navigator.clipboard.writeText`, as a tiny JS shim — so it
/// needs neither the unstable web-sys Clipboard API nor its cfg flag. wasm target only.
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen::prelude::wasm_bindgen(
    inline_js = "export function adiClipboardWrite(t){ try { if (navigator.clipboard) navigator.clipboard.writeText(t); } catch (e) {} }"
)]
extern "C" {
    #[wasm_bindgen(js_name = adiClipboardWrite)]
    fn clipboard_write(text: &str);
}

/// Select all text of the focused input, so clicking the id/ticket field readies a manual copy.
fn select_target(ev: &web_sys::FocusEvent) {
    if let Some(input) = ev
        .target()
        .and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok())
    {
        input.select();
    }
}
