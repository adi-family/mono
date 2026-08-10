//! The Sub-projects panel of the project detail page.

use adi_webapp_api::types::{NewProject, Project};
use leptos::prelude::*;
use adi_ui::{EmptyRow, Row as TableRow, Table};

use crate::fetch;
use crate::routing::{Route, open_project, project_href};
use crate::state::{Flash, State};
use crate::ui::{
    Key, TextField, fmt_date, sort_rows,
};

/// The panel's columns. No action column — archive and restore live on the Projects page; a row
/// here is a way in, not a thing to manage.
pub(crate) const COLS: &[&str] = &["Name", "ID", "Created", "Status"];

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
            <Table state=state.tables.subprojects>{move || subproject_rows(state, route)}</Table>
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
    let table = state.tables.subprojects;
    let Some(d) = state.project_detail.get() else {
        return view! { <EmptyRow state=table>"Loading…"</EmptyRow> }.into_any();
    };
    if d.subprojects.is_empty() {
        return view! { <EmptyRow state=table>"No sub-projects yet — add one below."</EmptyRow> }.into_any();
    }
    let mut subprojects = d.subprojects;
    sort_rows(
        &mut subprojects,
        table.sort.get(),
        |p, col| match col {
            "ID" => Key::text(&p.id),
            "Created" => Key::num(p.created_at),
            "Status" => Key::Bool(p.is_archived()),
            _ => Key::text(&p.name),
        },
        |p| Key::text(&p.name),
    );
    subprojects
        .into_iter()
        .map(|p| view! { <TableRow state=table cell=move |col| subproject_cell(col, &p, state, route)/> }.into_any())
        .collect::<Vec<_>>()
        .into_any()
}

/// One sub-project's cell under `col`. Matching the header text — the same key the sort uses —
/// is what lets the user hide and reorder columns without the row builder knowing about it.
fn subproject_cell(col: &str, p: &Project, state: State, route: RwSignal<Route>) -> AnyView {
    match col {
        "ID" => view! { <span class="font-mono">{p.id.clone()}</span> }.into_any(),
        "Created" => {
            view! { <span class="font-mono text-meta">{fmt_date(p.created_at)}</span> }.into_any()
        }
        "Status" => {
            if p.is_archived() {
                view! { <span><span class="adi-chip">"Archived"</span></span> }.into_any()
            } else {
                view! { <span><span class="adi-muted">"Active"</span></span> }.into_any()
            }
        }
        // "Name", and anything the layout offers that this match doesn't name.
        _ => {
            let open_id = p.id.clone();
            let href = project_href(&p.id);
            let title = p.description.clone().unwrap_or_default();
            view! {
                <span title=title>
                    <a class="adi-btn adi-btn--link" href=href
                        on:click=move |ev: web_sys::MouseEvent| {
                            if ev.meta_key() || ev.ctrl_key() || ev.shift_key() || ev.button() != 0 { return; }
                            ev.prevent_default();
                            open_project(state, route, open_id.clone());
                        }>{p.name.clone()}</a>
                </span>
            }
            .into_any()
        }
    }
}
