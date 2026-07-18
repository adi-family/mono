//! The Projects page: the registry of project metadata manifests, with a create form and
//! per-project archive/restore controls.

use adi_webapp_api::types::{NewProject, Project, ProjectsState, TasksState};
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
        name,
        description,
        parent,
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

            {data_table(&["Name", "ID", "Tasks", "Created", "Status", ""],
                move || project_rows(state, show_archived, route))}

            <form class="adi-form" on:submit=move |ev| {
                ev.prevent_default();
                let display = name.get().trim().to_string();
                if display.is_empty() {
                    flash.set(Some(Flash::err("A project name is required.".to_string())));
                    return;
                }
                let desc = description.get().trim().to_string();
                let par = parent.get().trim().to_string();
                let body = NewProject {
                    name: display.clone(),
                    description: (!desc.is_empty()).then_some(desc),
                    parent: (!par.is_empty()).then_some(par),
                };
                name.set(String::new());
                description.set(String::new());
                parent.set(String::new());
                apply_projects(state, Some(busy), format!("Registered project {display}."),
                    fetch::create_project(body));
            }>
                <TextField id="proj-name" label="Name" placeholder="My App" value=name />
                <div class="adi-field">
                    <label class="adi-field__label" for="proj-parent">"Parent (sub-project of)"</label>
                    <select class="adi-input" id="proj-parent"
                        prop:value=move || parent.get()
                        on:change=move |ev| parent.set(event_target_value(&ev))>
                        <option value="">"— none (top-level) —"</option>
                        {move || projects.get().map(|p| project_tree_rows(p.projects.into_iter()
                            .filter(|proj| !proj.is_archived()).collect()).into_iter()
                            .map(|(depth, proj)| {
                                // Non-breaking spaces preserve indentation inside option text.
                                let indent = "\u{00a0}\u{00a0}".repeat(depth);
                                let value = proj.id.clone();
                                let label = format!("{indent}{}", proj.name);
                                view! { <option value=value>{label}</option> }
                            }).collect::<Vec<_>>()).unwrap_or_default()}
                    </select>
                </div>
                <TextField id="proj-desc" label="Description" placeholder="optional one-liner" wide=true
                    field_class="adi-field--grow" value=description />
                <button class="adi-btn adi-btn--primary" type="submit" prop:disabled=move || busy.get()>
                    "Add project"
                </button>
            </form>
            {flash_view(flash)}
        </section>
    }
    .into_any()
}

/// The projects currently in view: filtered by the archived toggle, then flattened into
/// tree order. `None` while the first load is still in flight. Shared by the tree and the
/// detail pane, so the two can never disagree about what is on screen.
fn visible_projects(state: State, show_archived: RwSignal<bool>) -> Option<Vec<(usize, Project)>> {
    let show_all = show_archived.get();
    let rows: Vec<Project> = state
        .projects
        .get()?
        .projects
        .into_iter()
        .filter(|p| show_all || !p.is_archived())
        .collect();
    Some(project_tree_rows(rows))
}

/// Render the projects table body: a loading/empty placeholder, or one row per project
/// (filtered to active-only unless `show_archived`), indented by its depth in the tree.
/// The name opens the project's page; the trailing action archives or restores it. The
/// hierarchy itself is the explorer's job — this table is the registry.
fn project_rows(state: State, show_archived: RwSignal<bool>, route: RwSignal<Route>) -> AnyView {
    let Some(rows) = visible_projects(state, show_archived) else {
        return placeholder_row("6", "Loading\u{2026}");
    };
    if rows.is_empty() {
        let msg = if show_archived.get() {
            "No projects yet \u{2014} register one below."
        } else {
            "No active projects. Add one below, or switch to All to see archived ones."
        };
        return placeholder_row("6", msg);
    }
    let tasks = state.tasks.get();

    rows.into_iter()
        .map(|(depth, p)| {
            let archived = p.is_archived();
            let id = p.id.clone();
            let action = {
                let id = id.clone();
                if archived {
                    view! {
                        <button class="adi-btn adi-btn--link" on:click=move |_| {
                            apply_projects(state, None, format!("Restored {id}."),
                                fetch::unarchive_project(id.clone()));
                        }>"Restore"</button>
                    }
                    .into_any()
                } else {
                    view! {
                        <button class="adi-btn adi-btn--link" on:click=move |_| {
                            apply_projects(state, None, format!("Archived {id}."),
                                fetch::archive_project(id.clone()));
                        }>"Archive"</button>
                    }
                    .into_any()
                }
            };
            let status = if archived {
                view! { <span class="adi-chip">"Archived"</span> }.into_any()
            } else {
                view! { <span class="adi-muted">"Active"</span> }.into_any()
            };
            let tasks_cell = match open_tasks(tasks.as_ref(), &p.id) {
                Some((open, total)) => {
                    let tip = format!("{open} open \u{b7} {total} total");
                    view! { <span class="adi-chip adi-mono" title=tip>{format!("{open} open")}</span> }
                        .into_any()
                }
                None => view! { <span class="adi-muted">"\u{2014}"</span> }.into_any(),
            };
            let created = fmt_date(p.created_at);
            let title = p.description.clone().unwrap_or_default();
            let open_id = id.clone();
            let href = format!("/projects/{id}");
            // A computed per-row indent — the one thing here that genuinely varies per row.
            let indent = format!("padding-left:{}px", depth * 16);
            view! {
                <tr>
                    <td title=title>
                        <span style=indent>
                            <a class="adi-btn adi-btn--link" href=href
                                on:click=move |ev: web_sys::MouseEvent| {
                                    if ev.meta_key() || ev.ctrl_key() || ev.shift_key() || ev.button() != 0 { return; }
                                    ev.prevent_default();
                                    open_project(state, route, open_id.clone());
                                }>{p.name}</a>
                        </span>
                    </td>
                    <td class="adi-mono">{p.id}</td>
                    <td>{tasks_cell}</td>
                    <td class="adi-mono adi-muted">{created}</td>
                    <td>{status}</td>
                    <td class="adi-table__actions">{action}</td>
                </tr>
            }
        })
        .collect::<Vec<_>>()
        .into_any()
}

/// Flatten projects into depth-annotated tree order by their `parent` links: every root followed
/// by its sub-projects (recursively), preserving the incoming id sort among siblings. A project
/// whose parent isn't in the list (filtered out, or removed) renders as a root — nothing is lost.
/// Mirrors `task_tree_rows` for the task tree.
pub(crate) fn project_tree_rows(rows: Vec<Project>) -> Vec<(usize, Project)> {
    use std::collections::{HashMap, HashSet};

    fn walk(
        node: Project,
        depth: usize,
        children: &mut std::collections::HashMap<String, Vec<Project>>,
        out: &mut Vec<(usize, Project)>,
    ) {
        let id = node.id.clone();
        out.push((depth, node));
        if let Some(kids) = children.remove(&id) {
            for kid in kids {
                walk(kid, depth + 1, children, out);
            }
        }
    }

    let ids: HashSet<String> = rows.iter().map(|r| r.id.clone()).collect();
    let mut children: HashMap<String, Vec<Project>> = HashMap::new();
    let mut roots: Vec<Project> = Vec::new();
    for r in rows {
        match &r.parent {
            Some(p) if ids.contains(p) => children.entry(p.clone()).or_default().push(r),
            _ => roots.push(r),
        }
    }

    let mut out = Vec::new();
    for root in roots {
        walk(root, 0, &mut children, &mut out);
    }
    out
}

/// A project's `(open, total)` task counts, or `None` when tasks are still loading or the
/// project has none — both of which render as a dash rather than a zero.
fn open_tasks(tasks: Option<&TasksState>, project_id: &str) -> Option<(usize, usize)> {
    let tasks = tasks?;
    let mut open = 0usize;
    let mut total = 0usize;
    for t in tasks
        .tasks
        .iter()
        .filter(|t| t.project.as_deref() == Some(project_id))
    {
        total += 1;
        if t.status == "open" {
            open += 1;
        }
    }
    (total > 0).then_some((open, total))
}

/// Run a projects mutation: set the returned state and a success flash, or an error flash;
/// toggles `busy` around the request when a form is driving it.
fn apply_projects<F>(state: State, busy: Option<RwSignal<bool>>, ok_msg: String, fut: F)
where
    F: std::future::Future<Output = Result<ProjectsState, String>> + 'static,
{
    apply_mutation(state, busy, ok_msg, |s, p| s.projects.set(Some(p)), fut);
}
