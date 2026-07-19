//! The Ports Manager page: the live registry table (reserve/release), plus a scan of every
//! listening port with an ADI-managed filter.

use adi_webapp_api::types::LeaseRef;
use leptos::prelude::*;
use wasm_bindgen_futures::spawn_local;

use crate::fetch;
use crate::state::{Flash, Form, State, load};
use crate::ui::{
    TextField, dash, data_table, flash_view, placeholder_row, segmented, updated_text,
};

/// The Ports Manager page: the live registry table plus the reserve/release controls.
pub(crate) fn ports_manager_view(
    state: State,
    form: Form,
    managed_only: RwSignal<bool>,
) -> AnyView {
    let State {
        ports,
        flash,
        secs_since,
        used,
        ..
    } = state;
    let Form {
        svc,
        key,
        reserving,
        reserved,
    } = form;
    view! {
        <section class="adi-panel">
            <div class="adi-panel__head">
                // Two panels here list different things, so each keeps its title.
                <h2 class="adi-panel__title">"Port registry"</h2>
                <span class="adi-chip adi-mono" title="Active leases">
                    {move || ports.get().map_or_else(|| "\u{2014}".to_string(),
                        |p| p.leases.len().to_string())}
                </span>
                <span class="adi-spacer"></span>
                <span class="adi-updated">{move || updated_text(ports, secs_since)}</span>
            </div>

            {data_table(&["Service", "Key", "Port", ""], move || rows_view(state))}
        </section>

        <section class="adi-panel">
            <div class="adi-panel__head">
                <h2 class="adi-panel__title">"Reserve a port"</h2>
            </div>

            <form class="adi-form" on:submit=move |ev| {
                ev.prevent_default();
                let service = svc.get().trim().to_string();
                let k = key.get().trim().to_string();
                if service.is_empty() || k.is_empty() {
                    return;
                }
                reserving.set(true);
                spawn_local(async move {
                    match fetch::reserve(&LeaseRef { service: service.clone(), key: k.clone() }).await {
                        Ok(r) => {
                            reserved.set(format!("{}/{} → :{}", r.service, r.key, r.port));
                            flash.set(Some(Flash::ok(
                                format!("Reserved port {} for {}/{}.", r.port, r.service, r.key),
                            )));
                            load(state).await;
                        }
                        Err(e) => flash.set(Some(Flash::err(e))),
                    }
                    reserving.set(false);
                });
            }>
                <TextField id="svc" label="Service" placeholder="frontend" value=svc />
                <TextField id="key" label="Port key" placeholder="http" value=key />
                <button class="adi-btn adi-btn--primary" type="submit"
                    prop:disabled=move || reserving.get()>
                    "Reserve port"
                </button>
                <span class="adi-spacer"></span>
                <span class="adi-chip adi-mono">{move || reserved.get()}</span>
            </form>
            {flash_view(flash)}
        </section>

        <section class="adi-panel">
            <div class="adi-panel__head">
                <h2 class="adi-panel__title">"Ports in use"</h2>
                <span class="adi-updated">
                    {move || used.get().map_or(String::new(), |u| format!("{} listening", u.ports.len()))}
                </span>
                <span class="adi-spacer"></span>
                {segmented("Filter ports", managed_only, "All", "ADI managed")}
            </div>
            {data_table(&["Port", "Process", "PID", "Owner"], move || used_rows_view(state, managed_only))}
        </section>
    }
    .into_any()
}

/// Render the port table body: a loading/empty placeholder, or one row per lease sorted
/// by port. Reads `ports` reactively, so it re-renders on every refresh.
fn rows_view(state: State) -> AnyView {
    match state.ports.get() {
        None => placeholder_row("4", "Loading…"),
        Some(p) if p.leases.is_empty() => {
            placeholder_row("4", "No ports reserved yet — reserve one below.")
        }
        Some(p) => {
            let mut leases = p.leases;
            leases.sort_by_key(|l| l.port);
            leases
                .into_iter()
                .map(|l| {
                    let service = l.service.clone();
                    let key = l.key.clone();
                    view! {
                        <tr>
                            <td class="adi-mono">{l.service}</td>
                            <td class="adi-mono">{l.key}</td>
                            <td class="adi-mono adi-table__port">{l.port.to_string()}</td>
                            <td class="adi-table__actions">
                                <button class="adi-btn adi-btn--link" on:click=move |_| {
                                    let service = service.clone();
                                    let key = key.clone();
                                    spawn_local(async move {
                                        let req = LeaseRef { service, key };
                                        match fetch::release(&req).await {
                                            Ok(r) => {
                                                let msg = match r.freed {
                                                    Some(port) => format!("Released port {port}."),
                                                    None => "Nothing to release.".to_string(),
                                                };
                                                state.flash.set(Some(Flash::ok(msg)));
                                                load(state).await;
                                            }
                                            Err(e) => state.flash.set(Some(Flash::err(e))),
                                        }
                                    });
                                }>"Release"</button>
                            </td>
                        </tr>
                    }
                })
                .collect::<Vec<_>>()
                .into_any()
        }
    }
}

/// Render the "ports in use" table body: every listening port, or only the ADI-managed
/// ones when `managed_only`. A port is ADI-managed when a registry lease binds it.
fn used_rows_view(state: State, managed_only: RwSignal<bool>) -> AnyView {
    let Some(used) = state.used.get() else {
        return placeholder_row("4", "Scanning…");
    };
    let leases = state.ports.get().map(|p| p.leases).unwrap_or_default();
    let managed = managed_only.get();

    let rows: Vec<_> = used
        .ports
        .into_iter()
        .filter_map(|u| {
            let lease = leases.iter().find(|l| l.port == u.port).cloned();
            // ADI-managed: bound by a registry lease, or owned by an `adi-*` service process.
            let is_adi =
                lease.is_some() || u.process.as_deref().is_some_and(|p| p.starts_with("adi"));
            if managed && !is_adi {
                return None;
            }
            Some((u, lease))
        })
        .collect();

    if rows.is_empty() {
        let msg = if managed {
            "No ADI-managed ports are listening."
        } else {
            "No listening ports found."
        };
        return placeholder_row("4", msg);
    }

    rows.into_iter()
        .map(|(u, lease)| {
            let owner = match lease {
                Some(l) => view! {
                    <td><span class="adi-chip">{format!("{}/{}", l.service, l.key)}</span></td>
                }
                .into_any(),
                None => view! { <td class="adi-muted">"—"</td> }.into_any(),
            };
            let process = dash(u.process);
            let pid = u.pid.map_or_else(|| "—".to_string(), |p| p.to_string());
            view! {
                <tr>
                    <td class="adi-mono adi-table__port">{u.port.to_string()}</td>
                    <td>{process}</td>
                    <td class="adi-mono adi-muted">{pid}</td>
                    {owner}
                </tr>
            }
        })
        .collect::<Vec<_>>()
        .into_any()
}
