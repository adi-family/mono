//! The Projects page: the registry of project metadata manifests, with a create form and
//! per-project archive/restore controls.

use adi_webapp_api::types::{NewProject, Project, ProjectsState};
use leptos::prelude::*;

use crate::fetch;
use crate::routing::{Route, open_project};
use crate::state::{Flash, ProjectsForm, State};
use crate::ui::{
    TextField, apply_mutation, data_table, flash_view, fmt_date, placeholder_row, segmented, tile,
    updated_text,
};

/// The Projects page: the registry of project metadata manifests, with a create form and
/// per-project archive/restore controls. A project's runtime config lives in its own
/// `.adi/hive.yaml`; this page manages only the `config.toml` manifest.
pub(crate) fn projects_view(state: State, form: ProjectsForm, route: RwSignal<Route>) -> AnyView {
    let State {
        projects,
        flash,
        secs_since,
        ..
    } = state;
    let ProjectsForm {
        id,
        name,
        description,
        busy,
        show_archived,
    } = form;
    view! {
        <section class="adi-tiles">
            {tile("Projects",
                move || projects.get().map_or_else(|| "—".to_string(), |p| p.projects.len().to_string()),
                "registered manifests")}
            {tile("Active",
                move || projects.get().map_or_else(|| "—".to_string(),
                    |p| p.projects.iter().filter(|x| !x.is_archived()).count().to_string()),
                move || projects.get().map_or_else(|| "not archived".to_string(),
                    |p| format!("{} archived", p.projects.iter().filter(|x| x.is_archived()).count())))}
        </section>

        <section class="adi-panel">
            <div class="adi-panel__head">
                <h2 class="adi-panel__title">"Registered projects"</h2>
                <span class="adi-updated">{move || updated_text(state.ports, secs_since)}</span>
                <span class="adi-spacer"></span>
                {segmented("Filter projects", show_archived, "Active", "All")}
            </div>

            {data_table(&["Name", "ID", "Created", "Status", ""],
                move || project_rows(state, show_archived, route))}

            <form class="adi-form" on:submit=move |ev| {
                ev.prevent_default();
                let pid = id.get().trim().to_string();
                if pid.is_empty() {
                    flash.set(Some(Flash::err("A project id is required.".to_string())));
                    return;
                }
                let display = name.get().trim().to_string();
                let desc = description.get().trim().to_string();
                let body = NewProject {
                    id: pid.clone(),
                    name: (!display.is_empty()).then_some(display),
                    description: (!desc.is_empty()).then_some(desc),
                };
                id.set(String::new());
                name.set(String::new());
                description.set(String::new());
                apply_projects(state, Some(busy), format!("Registered project {pid}."),
                    fetch::create_project(body));
            }>
                <TextField id="proj-id" label="Project id" placeholder="my-app" mono=true value=id />
                <TextField id="proj-name" label="Name" placeholder="My App (defaults to the id)" value=name />
                <TextField id="proj-desc" label="Description" placeholder="optional one-liner" wide=true
                    field_style="flex:1 1 240px; min-width:0" value=description />
                <button class="adi-btn adi-btn--primary" type="submit" prop:disabled=move || busy.get()>
                    "Add project"
                </button>
            </form>
            {flash_view(flash)}
        </section>
    }
    .into_any()
}

/// Render the projects table body: a loading/empty placeholder, or one row per project
/// (filtered to active-only unless `show_archived`). The name opens the project's detail
/// page; the trailing action archives/restores it.
fn project_rows(state: State, show_archived: RwSignal<bool>, route: RwSignal<Route>) -> AnyView {
    let Some(state_projects) = state.projects.get() else {
        return placeholder_row("5", "Loading…");
    };
    let show_all = show_archived.get();
    let rows: Vec<Project> = state_projects
        .projects
        .into_iter()
        .filter(|p| show_all || !p.is_archived())
        .collect();

    if rows.is_empty() {
        let msg = if show_all {
            "No projects yet — register one below."
        } else {
            "No active projects. Add one below, or switch to All to see archived ones."
        };
        return placeholder_row("5", msg);
    }

    rows.into_iter()
        .map(|p| {
            let archived = p.is_archived();
            let id = p.id.clone();
            let action = if archived {
                let id = id.clone();
                view! {
                    <button class="adi-btn adi-btn--link" on:click=move |_| {
                        apply_projects(state, None, format!("Restored {id}."),
                            fetch::unarchive_project(id.clone()));
                    }>"Restore"</button>
                }
                .into_any()
            } else {
                let id = id.clone();
                view! {
                    <button class="adi-btn adi-btn--link" on:click=move |_| {
                        apply_projects(state, None, format!("Archived {id}."),
                            fetch::archive_project(id.clone()));
                    }>"Archive"</button>
                }
                .into_any()
            };
            let status = if archived {
                view! { <span class="adi-chip">"Archived"</span> }.into_any()
            } else {
                view! { <span class="adi-muted">"Active"</span> }.into_any()
            };
            let created = fmt_date(p.created_at);
            let title = p.description.clone().unwrap_or_default();
            let open_id = id.clone();
            let href = format!("/projects/{id}");
            view! {
                <tr>
                    <td title=title>
                        <a class="adi-btn adi-btn--link" href=href
                            on:click=move |ev: web_sys::MouseEvent| {
                                if ev.meta_key() || ev.ctrl_key() || ev.shift_key() || ev.button() != 0 { return; }
                                ev.prevent_default();
                                open_project(state, route, open_id.clone());
                            }>{p.name}</a>
                    </td>
                    <td class="adi-mono">{p.id}</td>
                    <td class="adi-mono adi-muted">{created}</td>
                    <td>{status}</td>
                    <td style="text-align:right">{action}</td>
                </tr>
            }
        })
        .collect::<Vec<_>>()
        .into_any()
}

/// Run a projects mutation: set the returned state and a success flash, or an error flash;
/// toggles `busy` around the request when a form is driving it.
fn apply_projects<F>(state: State, busy: Option<RwSignal<bool>>, ok_msg: String, fut: F)
where
    F: std::future::Future<Output = Result<ProjectsState, String>> + 'static,
{
    apply_mutation(state, busy, ok_msg, |s, p| s.projects.set(Some(p)), fut);
}
