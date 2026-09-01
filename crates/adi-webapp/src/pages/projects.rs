//! The Projects page: the registry of project metadata manifests, with a create form and
//! per-project archive/restore controls.

use std::collections::HashMap;

use adi_ui::{EmptyRow, Row as TableRow, Table};
use adi_webapp_api::types::{NewProject, Project, ProjectsState, TasksState};
use leptos::prelude::*;

use crate::fetch;
use crate::routing::{Route, open_project, project_href};
use crate::state::{Flash, ProjectsForm, State};
use crate::ui::{
    Key, TextField, apply_mutation, confirm, flash_view, fmt_date, menu_item, row_actions,
    sort_rows, updated_text,
};

/// The live projects table's columns; the trailing blank one holds the row's Archive control.
pub(crate) const COLS: &[&str] = &["Name", "ID", "Tasks", "Created", "Status", ""];

/// The archive's columns. No Status — every row there is archived, so it would say the same thing
/// all the way down — and the date column is when it *was* archived, which is what you sort by
/// when hunting for something to restore.
pub(crate) const ARCHIVED_COLS: &[&str] = &["Name", "ID", "Tasks", "Archived", ""];

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
        <section class="adi-panel">
            <div class="adi-panel__head">
                <span class="adi-chip adi-mono" title="Active projects, at every depth">
                    {move || projects.get().map_or_else(|| "\u{2014}".to_string(),
                        |p| p.projects.iter().filter(|x| !x.is_archived()).count().to_string())}
                </span>
                <span class="adi-updated">{move || updated_text(projects, secs_since)}</span>
            </div>

            <Table state=state.tables.projects>{move || project_rows(state, route, false)}</Table>
        </section>

        <section class="adi-panel">
            <div class="adi-panel__head">
                <h2 class="adi-panel__title">"New project"</h2>
            </div>

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

        {archived_section(state, route, show_archived)}
    }
    .into_any()
}

/// The count of archived projects, at every depth. `0` while the first load is in flight, which
/// also keeps the archived disclosure hidden until there is something real to disclose.
fn archived_count(state: State) -> usize {
    state
        .projects
        .get()
        .map_or(0, |p| p.projects.iter().filter(|x| x.is_archived()).count())
}

/// The archive: its own collapsed panel at the foot of the page, with a caret header and a count.
/// Expanding reveals archived projects so they can be restored. A separate panel rather than a
/// section inside the main one — inline, the split between live and archived rows reads as one
/// continuous table. Renders nothing at all when nothing is archived.
fn archived_section(state: State, route: RwSignal<Route>, show: RwSignal<bool>) -> AnyView {
    view! {
        {move || {
            let n = archived_count(state);
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
                        {open.then(|| view! { <Table state=state.tables.projects_archived>{move || project_rows(state, route, true)}</Table> }.into_any())}
                    </section>
                }
                .into_any()
            })
        }}
    }
    .into_any()
}

/// Render a projects table body: a loading/empty placeholder, or one row per project matching
/// `archived`, indented by its depth in the tree. The name opens the project's page; the trailing
/// action archives or restores it. Archived rows are split into their own collapsed table, so the
/// main one shows only live projects — but both are built from this one function.
///
/// Each side is tree-flattened over its own subset, so a project whose parent fell on the other
/// side of the split renders as a root rather than vanishing.
///
/// The sort is applied to the flat list *before* flattening, so it orders siblings and the tree
/// survives: sorting a nested registry by date shouldn't tear sub-projects away from their parents.
fn project_rows(state: State, route: RwSignal<Route>, archived: bool) -> AnyView {
    let table = if archived {
        state.tables.projects_archived
    } else {
        state.tables.projects
    };
    let Some(loaded) = state.projects.get() else {
        return view! { <EmptyRow state=table>"Loading\u{2026}"</EmptyRow> }.into_any();
    };
    // Rolled up once, not per comparison: the Tasks column is both a sort key and a cell, and
    // both would otherwise rescan the whole task list for every project they touch.
    let counts = task_counts(state.tasks.get().as_ref());
    let mut mine: Vec<Project> = loaded
        .projects
        .into_iter()
        .filter(|p| p.is_archived() == archived)
        .collect();
    sort_rows(
        &mut mine,
        table.sort.get(),
        |p, col| match col {
            "ID" => Key::text(&p.id),
            "Tasks" => Key::count(counts.get(&p.id).map_or(0, |(open, _)| *open)),
            // Both date columns read the same field the cell renders.
            "Created" => Key::num(p.created_at),
            "Archived" => Key::num(p.archived_at.unwrap_or(p.created_at)),
            "Status" => Key::Bool(p.is_archived()),
            _ => Key::text(&p.name),
        },
        |p| Key::text(&p.name),
    );
    let rows = project_tree_rows(mine);
    if rows.is_empty() {
        return view! { <EmptyRow state=table>{if archived {
            "Nothing archived."
        } else {
            "No projects yet \u{2014} register one below."
        }}</EmptyRow> }
        .into_any();
    }

    rows.into_iter()
        .map(|(depth, p)| {
            let archived = p.is_archived();
            // Archived rows keep Restore inline with Delete in the kebab; a live row's lone Archive
            // stays inline (no overflow ⇒ `row_actions` drops the kebab).
            let action = {
                let id = p.id.clone();
                let key = format!("project:{id}");
                if archived {
                    let del_id = id.clone();
                    let restore = view! {
                        <button class="adi-btn adi-btn--link" on:click=move |_| {
                            apply_projects(state, None, format!("Restored {id}."),
                                fetch::unarchive_project(id.clone()));
                        }>"Restore"</button>
                    };
                    let delete = menu_item(state, "Delete", true, move || {
                        if !confirm(&format!(
                            "Permanently delete project {del_id}? This cannot be undone.")) {
                            return;
                        }
                        apply_projects(state, None, format!("Deleted {del_id}."),
                            fetch::remove_project(del_id.clone()));
                    });
                    row_actions(state, key, restore, vec![delete])
                } else {
                    let archive = view! {
                        <button class="adi-btn adi-btn--link" on:click=move |_| {
                            apply_projects(state, None, format!("Archived {id}."),
                                fetch::archive_project(id.clone()));
                        }>"Archive"</button>
                    };
                    row_actions(state, key, archive, Vec::new())
                }
            };
            let tasks = counts.get(&p.id).copied();
            view! { <TableRow state=table cell=move |col| project_cell(col, &p, depth, tasks, state, route) actions=action/> }.into_any()
        })
        .collect::<Vec<_>>()
        .into_any()
}

/// One project's cell under `col`. Matching the header text — the same key the sort uses — is
/// what lets the user hide and reorder columns without the row builder knowing about it.
fn project_cell(
    col: &str,
    p: &Project,
    depth: usize,
    counts: Option<(usize, usize)>,
    state: State,
    route: RwSignal<Route>,
) -> AnyView {
    match col {
        "ID" => view! { <span class="font-mono">{p.id.clone()}</span> }.into_any(),
        "Tasks" => match counts {
            Some((open, total)) => {
                let tip = format!("{open} open \u{b7} {total} total");
                view! {
                    <span>
                        <span class="adi-chip adi-mono" title=tip>{format!("{open} open")}</span>
                    </span>
                }
                .into_any()
            }
            None => view! { <span><span class="adi-muted">"\u{2014}"</span></span> }.into_any(),
        },
        // The archive dates rows by when they were archived; the live table by creation.
        "Created" => {
            view! { <span class="font-mono text-meta">{fmt_date(p.created_at)}</span> }.into_any()
        }
        "Archived" => {
            let at = fmt_date(p.archived_at.unwrap_or(p.created_at));
            view! { <span class="font-mono text-meta">{at}</span> }.into_any()
        }
        "Status" => {
            let label = if p.is_archived() {
                "Archived"
            } else {
                "Active"
            };
            view! { <span><span class="adi-muted">{label}</span></span> }.into_any()
        }
        // "Name", and anything the layout offers that this match doesn't name.
        _ => {
            let open_id = p.id.clone();
            let href = project_href(&p.id);
            let title = p.description.clone().unwrap_or_default();
            // A computed per-row indent — the one thing here that genuinely varies per row.
            let indent = format!("padding-left:{}px", depth * 16);
            view! {
                <span title=title>
                    <span style=indent>
                        <a class="adi-btn adi-btn--link" href=href
                            on:click=move |ev: web_sys::MouseEvent| {
                                if ev.meta_key() || ev.ctrl_key() || ev.shift_key() || ev.button() != 0 { return; }
                                ev.prevent_default();
                                open_project(state, route, open_id.clone());
                            }>{p.name.clone()}</a>
                    </span>
                </span>
            }
            .into_any()
        }
    }
}

/// Flatten projects into depth-annotated tree order by their `parent` links: every root followed
/// by its sub-projects (recursively), preserving the incoming id sort among siblings. A project
/// whose parent isn't in the list (filtered out, or removed) renders as a root — nothing is lost.
/// Mirrors `task_tree_rows` for the task tree.
pub(crate) fn project_tree_rows(rows: Vec<Project>) -> Vec<(usize, Project)> {
    use std::collections::HashSet;

    fn walk(
        node: Project,
        depth: usize,
        children: &mut HashMap<String, Vec<Project>>,
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

/// Every project's `(open, total)` task counts, keyed by project id, in one pass over the tree.
/// A project absent from the map has no tasks (or the tree is still loading) and renders as a
/// dash rather than a zero.
fn task_counts(tasks: Option<&TasksState>) -> HashMap<String, (usize, usize)> {
    let mut counts: HashMap<String, (usize, usize)> = HashMap::new();
    let Some(tasks) = tasks else {
        return counts;
    };
    for t in &tasks.tasks {
        let Some(project) = t.project.as_deref() else {
            continue;
        };
        let entry = counts.entry(project.to_string()).or_default();
        entry.1 += 1;
        if t.status == "open" {
            entry.0 += 1;
        }
    }
    counts
}

/// Run a projects mutation: set the returned state and a success flash, or an error flash;
/// toggles `busy` around the request when a form is driving it.
fn apply_projects<F>(state: State, busy: Option<RwSignal<bool>>, ok_msg: String, fut: F)
where
    F: std::future::Future<Output = Result<ProjectsState, String>> + 'static,
{
    apply_mutation(state, busy, ok_msg, |s, p| s.projects.set(Some(p)), fut);
}
