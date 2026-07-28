//! The Dashboards page: every dashboard under `~/.adi/mono/dashboards/`, with its frontend and
//! backend liveness, and what agents have authored into it.
//!
//! A dashboard's UI is loose `.ts` files — `frontend/modules/*.ts` panels and
//! `backend/routes/*.ts` endpoints — so the module and route lists, not the two fixed entry
//! points, are what actually tell you what a dashboard does. That is what this page surfaces.

use adi_webapp_api::types::{Dashboard, DashboardsState, NewDashboard};
use leptos::prelude::*;
use wasm_bindgen_futures::spawn_local;

use crate::fetch;
use crate::state::{DashboardsForm, Flash, State, load};
use crate::ui::{
    Key, TableState, TextField, apply_mutation, body_row, configurable_table, confirm, flash_view,
    menu_item, placeholder_row, row_actions, sort_rows, updated_text,
};

/// The columns shared by the live table and the archived disclosure — both render one dashboard
/// per row through [`row_view`], with a trailing archive/restore action.
pub(crate) const COLS: &[&str] = &[
    "Dashboard",
    "Project",
    "Frontend",
    "Backend",
    "Modules",
    "Routes",
    "",
];

/// The Dashboards page: summary tiles, one row per dashboard, the create form, and a collapsed
/// archive of removed dashboards at the foot.
pub(crate) fn dashboards_view(state: State, form: DashboardsForm) -> AnyView {
    let State {
        dashboards,
        secs_since,
        ..
    } = state;

    view! {
        <section class="adi-panel">
            <div class="adi-panel__head">
                <span class="adi-chip adi-mono" title="Live dashboards">
                    {move || dashboards.get().map_or_else(|| "\u{2014}".to_string(),
                        |d| d.dashboards.iter().filter(|x| !x.is_archived()).count().to_string())}
                </span>
                <span class="adi-updated">{move || updated_text(dashboards, secs_since)}</span>
            </div>

            {configurable_table(state.tables.dashboards, COLS, move || rows_view(state, false))}
        </section>

        <section class="adi-panel">
            <div class="adi-panel__head">
                <h2 class="adi-panel__title">"New dashboard"</h2>
            </div>

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
                        // New dashboards start unfiled; file them from the row's Project picker.
                        project: None,
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

        </section>

        {archived_section(state, form.show_archived)}
    }
    .into_any()
}

/// The archive: its own collapsed panel at the foot of the page, with a caret header and a count.
/// Expanding reveals archived dashboards so they can be restored. Renders nothing at all when
/// nothing is archived. Mirrors the Projects page's archive.
fn archived_section(state: State, show: RwSignal<bool>) -> AnyView {
    view! {
        {move || {
            let n = state.dashboards.get().map_or(0,
                |d| d.dashboards.iter().filter(|x| x.is_archived()).count());
            (n > 0).then(|| {
                let open = show.get();
                view! {
                    <section class="adi-panel">
                        <div class="adi-panel__head">
                            <button class="adi-btn adi-btn--link" type="button"
                                aria-expanded=open.to_string()
                                on:click=move |_| show.update(|v| *v = !*v)>
                                {if open { "\u{25be}" } else { "\u{25b8}" }}" Archived"
                            </button>
                            <span class="adi-chip adi-mono">{n.to_string()}</span>
                        </div>
                        {open.then(|| configurable_table(state.tables.dashboards_archived, COLS,
                            move || rows_view(state, true)))}
                    </section>
                }
                .into_any()
            })
        }}
    }
    .into_any()
}

/// Render a table body — the live dashboards (`archived = false`) or the archived ones — as a
/// loading/empty placeholder or one row per matching dashboard.
fn rows_view(state: State, archived: bool) -> AnyView {
    let table: TableState = if archived {
        state.tables.dashboards_archived
    } else {
        state.tables.dashboards
    };
    let layout = table.layout.get();
    let Some(loaded) = state.dashboards.get() else {
        return placeholder_row(layout.span(), "Loading…");
    };
    let mut rows: Vec<Dashboard> = loaded
        .dashboards
        .into_iter()
        .filter(|d| d.is_archived() == archived)
        .collect();
    if rows.is_empty() {
        return placeholder_row(
            layout.span(),
            if archived {
                "Nothing archived."
            } else {
                "No dashboards yet — create one under ~/.adi/mono/dashboards/."
            },
        );
    }
    // A dead service sorts below every live one at the same port, so a descending Frontend sort
    // answers "what is actually up?" rather than interleaving the stopped ones.
    let service_key = |port: Option<u16>, running: bool| {
        Key::Int(i64::from(port.unwrap_or(0)) + if running { 1 << 20 } else { 0 })
    };
    // The Project cell shows a name, so the column orders by the name, not the id behind it.
    let names: std::collections::HashMap<String, String> = state
        .projects
        .get()
        .map(|p| p.projects.into_iter().map(|p| (p.id, p.name)).collect())
        .unwrap_or_default();
    sort_rows(
        &mut rows,
        table.sort.get(),
        |d, col| match col {
            "Project" => Key::maybe(d.project.as_deref().and_then(|id| names.get(id)).map(String::as_str)),
            "Frontend" => service_key(d.frontend_port, d.frontend_running),
            "Backend" => service_key(d.backend_port, d.backend_running),
            "Modules" => Key::count(d.modules.len()),
            "Routes" => Key::count(d.routes.len()),
            _ => Key::text(&d.name),
        },
        |d| Key::text(&d.name),
    );
    let shown = layout.shown();
    rows.into_iter()
        .map(|d| {
            let action = row_action(state, &d.id, d.is_archived());
            body_row(&shown, |col| cell(col, &d, state), Some(action))
        })
        .collect::<Vec<_>>()
        .into_any()
}

/// One dashboard's cell under `col`: identity, both services' state, and what it serves. Matching
/// the header text — the same key the sort uses — is what lets the user hide and reorder columns
/// without the row builder knowing about it.
///
/// The name is the primary way in: while the dashboard is up it is a real link to the running
/// page, so the row can be verified by clicking rather than by reading a port number.
fn cell(col: &str, d: &Dashboard, state: State) -> AnyView {
    match col {
        "Project" => view! { <td>{project_cell(state, d)}</td> }.into_any(),
        // Only the frontend is a link — the backend serves JSON to the page, not the reader.
        "Frontend" => {
            view! { <td>{service_cell(d.frontend_port, d.frontend_running, true)}</td> }.into_any()
        }
        "Backend" => {
            view! { <td>{service_cell(d.backend_port, d.backend_running, false)}</td> }.into_any()
        }
        "Modules" => view! { <td class="adi-mono">{summarize(&d.modules)}</td> }.into_any(),
        "Routes" => view! { <td class="adi-mono">{summarize(&d.routes)}</td> }.into_any(),
        // "Dashboard", and anything the layout offers that this match doesn't name.
        _ => {
            let name = match d.frontend_port.filter(|_| d.frontend_running) {
                Some(port) => {
                    let href = format!("http://127.0.0.1:{port}");
                    view! {
                        <a href=href.clone() target="_blank" rel="noreferrer" title=href>
                            {d.name.clone()}
                        </a>
                    }
                    .into_any()
                }
                // Nothing is listening, so a link would only 404 — show the name plainly.
                None => view! { <span>{d.name.clone()}</span> }.into_any(),
            };
            view! {
                <td>
                    <div>{name}</div>
                    <div class="adi-mono adi-muted" title=d.id.clone()>{short_id(&d.id)}</div>
                </td>
            }
            .into_any()
        }
    }
}

/// The Project cell: a compact picker filing this dashboard under a project (or none). Choosing an
/// option posts the change and folds the fresh listing back in, so the grouping updates at once.
fn project_cell(state: State, d: &Dashboard) -> AnyView {
    let id = d.id.clone();
    let current = d.project.clone().unwrap_or_default();
    let cur_none = current.is_empty();
    let options = state
        .projects
        .get()
        .map(|p| p.projects)
        .unwrap_or_default()
        .into_iter()
        .filter(|p| !p.is_archived())
        .map(|p| {
            let selected = p.id == current;
            view! { <option value=p.id.clone() selected=selected>{p.name}</option> }
        })
        .collect::<Vec<_>>();
    view! {
        <select class="adi-input adi-dash__proj"
            on:change=move |ev| {
                let val = event_target_value(&ev);
                apply_dashboards(state, "Filed dashboard.".to_string(),
                    fetch::set_dashboard_project(id.clone(), (!val.is_empty()).then_some(val)));
            }>
            <option value="" selected=cur_none>"\u{2014} none \u{2014}"</option>
            {options}
        </select>
    }
    .into_any()
}

/// The trailing action for a dashboard row: Archive while live (stops both services and hides it, a
/// single inline action — no kebab), or **Restore** inline with Delete in the kebab while archived
/// (Restore brings it back under supervision; Delete removes its directory for good, behind a
/// confirm). Each posts and folds the fresh [`DashboardsState`] back into the page.
fn row_action(state: State, id: &str, archived: bool) -> AnyView {
    let id = id.to_string();
    let short = short_id(&id);
    let key = format!("dashboard:{id}");
    if archived {
        let del_id = id.clone();
        let del_short = short.clone();
        let restore = view! {
            <button class="adi-btn adi-btn--link" on:click=move |_| {
                apply_dashboards(state, format!("Restored {short}."),
                    fetch::unarchive_dashboard(id.clone()));
            }>"Restore"</button>
        };
        let delete = menu_item(state, "Delete", true, move || {
            if !confirm(&format!(
                "Permanently delete dashboard {del_short}? This removes all of its files \
                 and cannot be undone.")) {
                return;
            }
            apply_dashboards(state, format!("Deleted {del_short}."),
                fetch::delete_dashboard(del_id.clone()));
        });
        row_actions(state, key, restore, vec![delete])
    } else {
        let archive = view! {
            <button class="adi-btn adi-btn--link" on:click=move |_| {
                apply_dashboards(state, format!("Archived {short}."),
                    fetch::archive_dashboard(id.clone()));
            }>"Archive"</button>
        };
        row_actions(state, key, archive, Vec::new())
    }
}

/// Run a dashboards mutation: fold the returned state into the page and flash success, or flash
/// the error. A thin typed wrapper over [`apply_mutation`], as `apply_projects` is for projects.
fn apply_dashboards<F>(state: State, ok_msg: String, fut: F)
where
    F: std::future::Future<Output = Result<DashboardsState, String>> + 'static,
{
    apply_mutation(state, None, ok_msg, |s, d| s.dashboards.set(Some(d)), fut);
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
