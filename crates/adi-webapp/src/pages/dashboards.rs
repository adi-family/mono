//! The Dashboards page: every dashboard under `~/.adi/mono/dashboards/`, with its frontend and
//! backend liveness, and what agents have authored into it.
//!
//! A dashboard's UI is loose `.ts` files — `frontend/modules/*.ts` panels and
//! `backend/routes/*.ts` endpoints — so the module and route lists, not the two fixed entry
//! points, are what actually tell you what a dashboard does. That is what this page surfaces.

use adi_webapp_api::types::{Dashboard, NewDashboard};
use leptos::prelude::*;
use wasm_bindgen_futures::spawn_local;

use crate::fetch;
use crate::state::{DashboardsForm, Flash, State, load};
use crate::ui::{TextField, data_table, flash_view, placeholder_row, tile, updated_text};

/// The Dashboards page: summary tiles, one row per dashboard, and the create form.
pub(crate) fn dashboards_view(state: State, form: DashboardsForm) -> AnyView {
    let State {
        dashboards,
        secs_since,
        ..
    } = state;

    view! {
        <section class="adi-tiles">
            {tile("Dashboards",
                move || dashboards.get().map_or_else(|| "—".to_string(), |d| d.dashboards.len().to_string()),
                "bun-served, agent-authored")}
            {tile("Serving",
                move || dashboards.get().map_or_else(|| "—".to_string(),
                    |d| d.dashboards.iter().filter(|x| x.frontend_running).count().to_string()),
                move || dashboards.get().map_or_else(|| "frontends up".to_string(), |d| {
                    format!("{} backend(s) up", d.dashboards.iter().filter(|x| x.backend_running).count())
                }))}
            {tile("Authored",
                move || dashboards.get().map_or_else(|| "—".to_string(), |d| {
                    d.dashboards.iter().map(|x| x.modules.len()).sum::<usize>().to_string()
                }),
                move || dashboards.get().map_or_else(|| "modules".to_string(), |d| {
                    format!("modules · {} route(s)", d.dashboards.iter().map(|x| x.routes.len()).sum::<usize>())
                }))}
        </section>

        <section class="adi-panel">
            <div class="adi-panel__head">
                <h2 class="adi-panel__title">"Dashboards"</h2>
                <span class="adi-spacer"></span>
                <span class="adi-updated">{move || updated_text(dashboards, secs_since)}</span>
            </div>

            {data_table(
                &["Dashboard", "Frontend", "Backend", "Modules", "Routes"],
                move || rows_view(state),
            )}

            <form class="adi-form" on:submit=move |ev| {
                ev.prevent_default();
                let name = form.name.get().trim().to_string();
                if name.is_empty() {
                    return;
                }
                let description = form.description.get().trim().to_string();
                form.busy.set(true);
                spawn_local(async move {
                    let body = NewDashboard {
                        name: name.clone(),
                        description: (!description.is_empty()).then_some(description),
                    };
                    match fetch::create_dashboard(body).await {
                        Ok(d) => {
                            form.name.set(String::new());
                            form.description.set(String::new());
                            // The supervisor leases ports and starts both servers on its own
                            // within a few seconds; say so rather than showing a dead row.
                            state.flash.set(Some(Flash::ok(format!(
                                "Created “{}” ({}). Starting — ports appear in a few seconds.",
                                d.name,
                                short_id(&d.id),
                            ))));
                            load(state).await;
                        }
                        Err(e) => state.flash.set(Some(Flash::err(e))),
                    }
                    form.busy.set(false);
                });
            }>
                <TextField id="dash-name" label="Name" placeholder="Metrics" value=form.name />
                <TextField id="dash-desc" label="Description" placeholder="What it is for"
                    value=form.description wide=true field_class="adi-field--grow" />
                <button class="adi-btn adi-btn--primary" type="submit"
                    prop:disabled=move || form.busy.get()>
                    "New dashboard"
                </button>
            </form>
            {flash_view(state.flash)}

            <footer class="adi-footer">
                "Each dashboard is two bun services in " <code>"~/.adi/mono/dashboards/<id>/"</code>
                ", supervised by the per-user dashboards hive — it picks up a new one within a few "
                "seconds, no restart. Agents extend a dashboard by dropping "<code>".ts"</code>
                " files into its " <code>"frontend/modules/"</code> " and " <code>"backend/routes/"</code> "."
            </footer>
        </section>
    }
    .into_any()
}

/// Render the table body: a loading/empty placeholder, or one row per dashboard.
fn rows_view(state: State) -> AnyView {
    match state.dashboards.get() {
        None => placeholder_row("5", "Loading…"),
        Some(d) if d.dashboards.is_empty() => placeholder_row(
            "5",
            "No dashboards yet — create one under ~/.adi/mono/dashboards/.",
        ),
        Some(d) => d
            .dashboards
            .into_iter()
            .map(row_view)
            .collect::<Vec<_>>()
            .into_any(),
    }
}

/// One dashboard row: identity, both services' state, and what it serves.
///
/// The name is the primary way in: while the dashboard is up it is a real link to the running
/// page, so the row can be verified by clicking rather than by reading a port number.
fn row_view(d: Dashboard) -> AnyView {
    let name = match d.frontend_port.filter(|_| d.frontend_running) {
        Some(port) => {
            let href = format!("http://127.0.0.1:{port}");
            view! { <a href=href.clone() target="_blank" rel="noreferrer" title=href>{d.name}</a> }
                .into_any()
        }
        // Nothing is listening, so a link would only 404 — show the name plainly instead.
        None => view! { <span>{d.name}</span> }.into_any(),
    };

    view! {
        <tr>
            <td>
                <div>{name}</div>
                <div class="adi-mono adi-muted" title=d.id.clone()>{short_id(&d.id)}</div>
            </td>
            // Only the frontend is a link — the backend serves JSON to the page, not the reader.
            <td>{service_cell(d.frontend_port, d.frontend_running, true)}</td>
            <td>{service_cell(d.backend_port, d.backend_running, false)}</td>
            <td class="adi-mono">{summarize(&d.modules)}</td>
            <td class="adi-mono">{summarize(&d.routes)}</td>
        </tr>
    }
    .into_any()
}

/// A service cell: the running led plus its loopback port, or a note when nothing is leased yet.
/// Dashboards carry no hostname, so the port *is* the address — `link` makes it openable.
fn service_cell(port: Option<u16>, running: bool, link: bool) -> AnyView {
    let Some(port) = port else {
        return view! { <span class="adi-muted">"not allocated"</span> }.into_any();
    };
    let (state_attr, label) = if running {
        ("online", format!(":{port}"))
    } else {
        ("down", format!(":{port} down"))
    };
    // A dead port is not worth offering to open, so only a running frontend becomes a link.
    let body = if link && running {
        view! {
            <a class="adi-mono" href=format!("http://127.0.0.1:{port}") target="_blank"
                rel="noreferrer" title=format!("http://127.0.0.1:{port}")>{label}</a>
        }
        .into_any()
    } else {
        view! { <span class="adi-mono">{label}</span> }.into_any()
    };
    view! {
        <span class="adi-status" data-state=state_attr>
            <span class="adi-status__led"></span>{body}
        </span>
    }
    .into_any()
}

/// The leading segment of a uuid — enough to recognize a dashboard without filling the column.
fn short_id(id: &str) -> String {
    id.split('-').next().unwrap_or(id).to_string()
}

/// Name the entries when there are few, else just count them — a dashboard an agent has been
/// working on for a while can have far more than fit in a cell.
fn summarize(items: &[String]) -> String {
    match items.len() {
        0 => "—".to_string(),
        1..=3 => items.join(", "),
        n => format!("{}, +{}", items[..2].join(", "), n - 2),
    }
}
