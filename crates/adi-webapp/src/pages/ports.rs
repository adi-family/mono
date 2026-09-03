//! The Ports manager page: the live registry table (reserve/release), plus a scan of every
//! listening port with an ADI-managed filter.

use adi_webapp_api::types::{Lease, LeaseRef, UsedPort};
use leptos::prelude::*;
use wasm_bindgen_futures::spawn_local;

use crate::fetch;
use crate::state::{Flash, Form, State, load};
use adi_ui::{EmptyRow, Row as TableRow, Table};

use crate::ui::{
    Key, TextField, dash, flash_view, menu_item, row_actions, rows_or_placeholder, segmented,
    sort_rows, updated_text,
};

/// The port registry's columns; the trailing blank one holds the row's ⋯ menu.
pub(crate) const LEASE_COLS: &[&str] = &["Service", "Key", "Port", ""];

/// The columns of the "ports in use" scan. No action column — the rows are a read-only view of
/// what the machine is listening on, not something this page can change.
pub(crate) const USED_COLS: &[&str] = &["Port", "Process", "PID", "Owner"];

/// The Ports manager page: its own title line (the shell leaves this page to name itself), the
/// registry table, the reserve form, and the scan of what is listening.
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
        <header class="adi-bar">
            <h1 class="adi-bar__title">"Ports manager"</h1>
            <span class="adi-ports__meta">
                {move || ports.get().map_or_else(String::new, |p| match p.leases.len() {
                    1 => "1 lease".to_string(),
                    n => format!("{n} leases"),
                })}
                {move || used.get().map(|u| format!(" \u{b7} {} listening", u.ports.len())).unwrap_or_default()}
            </span>
            <span class="adi-spacer"></span>
            <span class="adi-updated">{move || updated_text(ports, secs_since)}</span>
        </header>

        <section class="adi-panel">
            <div class="adi-panel__head">
                <h2 class="adi-panel__title">"Port registry"</h2>
            </div>

            <Table state=state.tables.leases>{move || rows_view(state)}</Table>
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
                            reserved.set(format!("{}/{} \u{2192} :{}", r.service, r.key, r.port));
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
                <TextField id="key" label="Port key" mono=true placeholder="http" value=key />
                <button class="adi-btn adi-btn--primary" type="submit"
                    prop:disabled=move || reserving.get()>
                    "Reserve port"
                </button>
                <span class="adi-spacer"></span>
                <span class="adi-mono adi-muted">{move || reserved.get()}</span>
            </form>
            {flash_view(flash)}
        </section>

        <section class="adi-panel">
            <div class="adi-panel__head">
                <h2 class="adi-panel__title">"Ports in use"</h2>
                <span class="adi-spacer"></span>
                {segmented("Filter ports", managed_only, "All", "ADI managed")}
            </div>
            <Table state=state.tables.used_ports>
                {move || used_rows_view(state, managed_only)}
            </Table>
        </section>
    }
    .into_any()
}

/// Render the port table body: a loading/empty placeholder, or one row per lease in the order and
/// arrangement the header controls select. Reads `ports` reactively, so it re-renders on every
/// refresh.
fn rows_view(state: State) -> AnyView {
    let table = state.tables.leases;
    let mut leases = match rows_or_placeholder(
        table,
        state.ports.get().map(|v| v.leases),
        "No ports reserved yet — reserve one below.",
    ) {
        Ok(rows) => rows,
        Err(placeholder) => return placeholder,
    };
    // By the port number itself, not its rendering — the lease list reads as an allocation map.
    sort_rows(
        &mut leases,
        table.sort.get(),
        |l, col| match col {
            "Service" => Key::text(&l.service),
            "Key" => Key::text(&l.key),
            _ => Key::Int(i64::from(l.port)),
        },
        |l| Key::Int(i64::from(l.port)),
    );
    leases
        .into_iter()
        .map(|l| {
            let service = l.service.clone();
            let key = l.key.clone();
            let menu_key = format!("lease:{service}/{key}");
            let release = menu_item(state, "Release", true, move || {
                let (service, key) = (service.clone(), key.clone());
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
            });
            view! {
                <TableRow
                    state=table
                    // The row owns its lease: the cell builder is stored and re-run whenever
                    // the layout changes, so it cannot borrow one.
                    cell=move |col| lease_cell(col, &l)
                    actions=row_actions(state, menu_key, (), vec![release])
                />
            }
            .into_any()
        })
        .collect::<Vec<_>>()
        .into_any()
}

/// One lease's cell under `col` — the cell's *contents*, since [`adi_ui::Row`] owns the `<td>`
/// and the padding, rule and hover that go with it.
///
/// Matching the header text — the same key the sort uses — is what lets the user hide and
/// reorder columns without the row builder knowing about it.
fn lease_cell(col: &str, l: &Lease) -> AnyView {
    match col {
        "Key" => view! { <span class="adi-mono adi-muted">{l.key.clone()}</span> }.into_any(),
        "Port" => view! { <span class="adi-tabnums">{l.port.to_string()}</span> }.into_any(),
        // "Service", and anything the layout offers that this match doesn't name.
        _ => view! { <span>{l.service.clone()}</span> }.into_any(),
    }
}

/// Render the "ports in use" table body: every listening port, or only the ADI-managed
/// ones when `managed_only`. A port is ADI-managed when a registry lease binds it.
fn used_rows_view(state: State, managed_only: RwSignal<bool>) -> AnyView {
    let table = state.tables.used_ports;
    let Some(used) = state.used.get() else {
        return view! { <EmptyRow state=table>"Scanning…"</EmptyRow> }.into_any();
    };
    let leases = state.ports.get().map(|p| p.leases).unwrap_or_default();
    let managed = managed_only.get();

    let mut rows: Vec<_> = used
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
        return view! { <EmptyRow state=table>{msg}</EmptyRow> }.into_any();
    }

    // An unowned port sorts as empty rather than dropping out, so "who owns what?" reads as one
    // block of owners followed by the unclaimed rest.
    sort_rows(
        &mut rows,
        table.sort.get(),
        |(u, lease), col| match col {
            "Process" => Key::maybe(u.process.as_deref()),
            "PID" => Key::num(u.pid.map_or(0, u64::from)),
            "Owner" => Key::text(
                lease
                    .as_ref()
                    .map_or_else(String::new, |l| format!("{}/{}", l.service, l.key)),
            ),
            _ => Key::Int(i64::from(u.port)),
        },
        |(u, _)| Key::Int(i64::from(u.port)),
    );

    rows.into_iter()
        .map(|(u, lease)| {
            view! { <TableRow state=table cell=move |col| used_cell(col, &u, lease.as_ref())/> }
                .into_any()
        })
        .collect::<Vec<_>>()
        .into_any()
}

/// One listening port's cell under `col`. See [`lease_cell`] on why this matches header text.
fn used_cell(col: &str, u: &UsedPort, lease: Option<&Lease>) -> AnyView {
    match col {
        "Process" => view! { <span>{dash(u.process.clone())}</span> }.into_any(),
        "PID" => match u.pid {
            Some(pid) => {
                view! { <span class="adi-mono adi-muted">{pid.to_string()}</span> }.into_any()
            }
            None => view! { <span class="adi-muted">"—"</span> }.into_any(),
        },
        "Owner" => match lease {
            Some(l) => view! {
                <span class="adi-chip">{format!("{}/{}", l.service, l.key)}</span>
            }
            .into_any(),
            None => view! { <span class="adi-muted">"—"</span> }.into_any(),
        },
        // "Port", and anything the layout offers that this match doesn't name.
        _ => view! { <span class="adi-tabnums">{u.port.to_string()}</span> }.into_any(),
    }
}
