//! The Sub-projects panel of the project detail page.

use adi_webapp_api::types::NewProject;
use leptos::prelude::*;

use crate::fetch;
use crate::routing::{Route, open_project, project_href};
use crate::state::{Flash, State};
use crate::ui::{TextField, data_table, fmt_date, placeholder_row};

use super::apply_detail_mutation;

/// The project detail page's quick sub-project create form (just a name — the id is generated
/// server-side and the parent is fixed to the open project). Descriptions and deeper nesting
/// live on the Projects page. `Copy` so it threads into the panel view and its submit handler.
#[derive(Clone, Copy)]
pub(crate) struct QuickSubprojectForm {
    pub(crate) name: RwSignal<String>,
    pub(crate) busy: RwSignal<bool>,
}

/// The Sub-projects panel on a project's detail page: the projects nested directly under this
/// one (served in the detail payload), each opening its own detail page, plus a quick create
/// form pre-scoped to the open project as the parent.
pub(crate) fn subprojects_panel(
    state: State,
    route: RwSignal<Route>,
    form: QuickSubprojectForm,
) -> AnyView {
    let QuickSubprojectForm { name, busy } = form;
    view! {
        <section class="adi-panel">
            <div class="adi-panel__head">
                <h2 class="adi-panel__title">"Sub-projects"</h2>
                <span class="adi-updated">"nested under this project"</span>
            </div>
            {data_table(&["Name", "ID", "Created", "Status"], move || subproject_rows(state, route))}
            <form class="adi-form" on:submit=move |ev| {
                ev.prevent_default();
                let parent = state.current_project.get_untracked();
                if parent.is_empty() {
                    return;
                }
                let display = name.get().trim().to_string();
                if display.is_empty() {
                    state.flash.set(Some(Flash::err("A project name is required.".to_string())));
                    return;
                }
                let body = NewProject {
                    name: display.clone(),
                    description: None,
                    parent: Some(parent.clone()),
                };
                name.set(String::new());
                apply_detail_mutation(state, parent, Some(busy), format!("Registered sub-project {display}."),
                    fetch::create_project(body));
            }>
                <TextField id="psub-name" label="Name" placeholder="My Sub-project" wide=true
                    field_class="adi-field--grow" value=name />
                <button class="adi-btn adi-btn--primary" type="submit" prop:disabled=move || busy.get()>
                    "Add sub-project"
                </button>
            </form>
            <div class="adi-hint">
                "These are full projects (each with its own directory, tasks, agents, and triggers),
                 nested here. They appear in the global " <code>"Projects"</code> " list too."
            </div>
        </section>
    }
    .into_any()
}

/// Rows for the sub-projects table: one per nested project, its name opening the detail page.
/// Loading/empty placeholders otherwise.
fn subproject_rows(state: State, route: RwSignal<Route>) -> AnyView {
    let Some(d) = state.project_detail.get() else {
        return placeholder_row("4", "Loading…");
    };
    if d.subprojects.is_empty() {
        return placeholder_row("4", "No sub-projects yet — add one below.");
    }
    d.subprojects
        .into_iter()
        .map(|p| {
            let id = p.id.clone();
            let open_id = id.clone();
            let href = project_href(&id);
            let created = fmt_date(p.created_at);
            let title = p.description.clone().unwrap_or_default();
            let status = if p.is_archived() {
                view! { <span class="adi-chip">"Archived"</span> }.into_any()
            } else {
                view! { <span class="adi-muted">"Active"</span> }.into_any()
            };
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
                    <td class="adi-mono">{id}</td>
                    <td class="adi-mono adi-muted">{created}</td>
                    <td>{status}</td>
                </tr>
            }
        })
        .collect::<Vec<_>>()
        .into_any()
}
