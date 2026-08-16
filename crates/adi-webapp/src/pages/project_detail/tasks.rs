//! The Tasks panel of the project detail page.

use adi_webapp_api::types::{NewTask, TasksState};
use leptos::prelude::*;
use adi_ui::{EmptyRow, Row as TableRow, Table};

use crate::fetch;
use crate::pages::tasks::{is_finished, task_cell, task_key};
use crate::routing::{ProjectSection, Route};
use crate::state::{Flash, State};
use crate::ui::{
    Key, Sort, TextField, apply_mutation, sort_rows,
    task_tree_rows,
};

/// The panel's columns. No Project column — every row is this project's (or a sub-project's,
/// which the Task cell marks inline), so it would say the same thing all the way down.
pub(crate) const COLS: &[&str] = &["Task", "ID", "Tag", "Status", "Subtasks", ""];

/// The project detail page's local task create form (title, an optional parent to nest under, and
/// optional tag/details; the project is fixed to the open project). `Copy` so it threads into the
/// panel view and its submit handler.
#[derive(Clone, Copy)]
pub(crate) struct TaskForm {
    pub(crate) title: RwSignal<String>,
    /// The id of the task to nest under (a subtask), or empty for a top-level task. The picker
    /// lists this project's whole tree, so a subtask can sit at any depth.
    pub(crate) parent: RwSignal<String>,
    pub(crate) tag: RwSignal<String>,
    /// Where the task's work happens — the directory a run picking it up starts in. Blank leaves
    /// it to the agent's own home; a subtask inherits its parent's.
    pub(crate) cwd: RwSignal<String>,
    pub(crate) details: RwSignal<String>,
    pub(crate) busy: RwSignal<bool>,
}

/// The Tasks panel on a project's detail page: the tasks filed under this project (from the shared
/// task tree at `/api/tasks`) plus a create form pre-scoped to it, so a task added here gets the
/// project's Jira-style `<KEY>-<n>` id without the user having to pick a project.
pub(crate) fn tasks_panel(state: State, route: RwSignal<Route>, form: TaskForm) -> AnyView {
    let TaskForm {
        title,
        parent,
        tag,
        cwd,
        details,
        busy,
    } = form;
    view! {
        <section class="adi-panel">
            <div class="adi-panel__head">
                <h2 class="adi-panel__title">"Tasks"</h2>
                <span class="adi-updated">"filed under this project & its sub-projects"</span>
            </div>
            <Table state=state.tables.project_tasks>{move || project_task_rows(state, route)}</Table>
            <form class="adi-form" on:submit=move |ev| {
                ev.prevent_default();
                let id = state.current_project.get_untracked();
                if id.is_empty() {
                    return;
                }
                let t = title.get().trim().to_string();
                if t.is_empty() {
                    state.flash.set(Some(Flash::err("A task title is required.".to_string())));
                    return;
                }
                let par = parent.get().trim().to_string();
                let tg = tag.get().trim().to_string();
                let det = details.get().trim().to_string();
                let dir = cwd.get().trim().to_string();
                let body = NewTask {
                    title: t.clone(),
                    details: (!det.is_empty()).then_some(det),
                    project: Some(id),
                    tag: (!tg.is_empty()).then_some(tg),
                    parent: (!par.is_empty()).then_some(par),
                    cwd: (!dir.is_empty()).then_some(dir),
                };
                title.set(String::new());
                parent.set(String::new());
                tag.set(String::new());
                details.set(String::new());
                // The directory stays: a project's tasks usually happen in the same place.
                apply_mutation(state, Some(busy), format!("Created task “{t}”."),
                    |s: State, ts: TasksState| s.tasks.set(Some(ts)), fetch::create_task(body));
            }>
                <TextField id="ptask-title" label="Title" placeholder="What needs doing?" wide=true
                    field_class="adi-field--grow" value=title />
                <div class="adi-field">
                    <label class="adi-field__label" for="ptask-parent">"Parent (subtask of)"</label>
                    <select class="adi-input" id="ptask-parent"
                        prop:value=move || parent.get()
                        on:change=move |ev| parent.set(event_target_value(&ev))>
                        <option value="">"— none (top-level) —"</option>
                        {move || project_task_options(state)}
                    </select>
                </div>
                <TextField id="ptask-tag" label="Tag" placeholder="agent name" mono=true
                    hint="= an agent name auto-starts it" value=tag />
                <TextField id="ptask-cwd" label="Working dir" placeholder="/path/to/work (optional)"
                    mono=true wide=true
                    hint="where a run picking this up starts; subtasks inherit it" value=cwd />
                <TextField id="ptask-details" label="Details" placeholder="optional notes" wide=true
                    field_class="adi-field--grow" value=details />
                <button class="adi-btn adi-btn--primary" type="submit" prop:disabled=move || busy.get()>
                    "Add task"
                </button>
            </form>
            <div class="adi-hint">
                "These appear in the global " <code>"Tasks"</code> " list too. Completing, editing, "
                "and subtasks stay in the " <code>"adi-mono tasks"</code> " CLI."
            </div>
        </section>
    }
    .into_any()
}

/// The tasks in `scope`, filtered from the shared tree and flattened into depth-annotated tree
/// order (so subtasks nest under their parent, at any depth). A scope carrying sub-projects folds
/// their tasks in too — each sub-project's tasks form their own subtree, since their `parent`
/// links point within the same sub-project.
/// `sort` orders the flat list before flattening, so it reorders siblings and the tree survives.
/// The parent picker passes the panel's declared default, since an `<option>` list has no headers
/// to sort by.
fn project_task_tree(
    state: State,
    scope: &super::ProjectScope,
    sort: Sort,
) -> Vec<(usize, adi_webapp_api::types::TaskRow)> {
    let Some(tasks) = state.tasks.get() else {
        return Vec::new();
    };
    let mut mine: Vec<_> = tasks
        .tasks
        .into_iter()
        .filter(|t| scope.contains(t.project.as_deref()))
        .collect();
    sort_rows(&mut mine, sort, task_key, |t| Key::text(&t.title));
    task_tree_rows(mine)
}

/// Rows for the project's task table: this project's own tasks plus those filed under its nested
/// sub-projects (each marked with a chip linking to the owning sub-project), as a nested tree —
/// each row indented by its depth, with its title, Jira id, tag, effective status, and subtask
/// rollup. Loading/empty placeholders otherwise.
fn project_task_rows(state: State, route: RwSignal<Route>) -> AnyView {
    let table = state.tables.project_tasks;
    if state.tasks.get().is_none() {
        return view! { <EmptyRow state=table>"Loading…"</EmptyRow> }.into_any();
    }
    let scope = super::ProjectScope::open(state, true);
    let tree = project_task_tree(state, &scope, table.sort.get());
    if tree.is_empty() {
        return view! { <EmptyRow state=table>"No tasks in this project yet — add one below."</EmptyRow> }.into_any();
    }
    tree.into_iter()
        .map(|(depth, t)| {
            // A task belonging to a sub-project is marked with a chip opening that sub-project's
            // Tasks section. Kept as its ids, not a built view: the cell builder is called per
            // column, so it has to be able to render the marker rather than consume one.
            let owner = scope.owner(t.project.as_deref());
            let action = {
                let id = t.id.clone();
                let store = |s: State, ts: TasksState| s.tasks.set(Some(ts));
                if is_finished(&t.effective) {
                    view! {
                        <button class="adi-btn adi-btn--link" on:click=move |_| {
                            apply_mutation(state, None, format!("Reopened {id}."), store,
                                fetch::reopen_task(id.clone()));
                        }>"Reopen"</button>
                    }
                    .into_any()
                } else {
                    view! {
                        <button class="adi-btn adi-btn--link" on:click=move |_| {
                            apply_mutation(state, None, format!("Archived {id}."), store,
                                fetch::archive_task(id.clone()));
                        }>"Archive"</button>
                    }
                    .into_any()
                }
            };
            view! { <TableRow state=table cell=move |col| {
                    // Every column but Task renders exactly as it does on the global page; only
                    // this one differs, by carrying the owning sub-project's marker.
                    if col != "Task" {
                        return task_cell(col, &t, depth);
                    }
                    let indent = format!("padding-left:{}px", depth * 20);
                    let details = t.details.clone().unwrap_or_default();
                    let marker = owner.clone().map(|(oid, oname)| {
                        super::sub_marker(state, route, oid, oname, ProjectSection::Tasks)
                    });
                    view! {
                        <span title=details>
                            <span style=indent>{t.title.clone()}</span>{marker}
                        </span>
                    }
                    .into_any()
                } actions=action/> }.into_any()
        })
        .collect::<Vec<_>>()
        .into_any()
}

/// `<option>`s for the parent picker: every task in this project, indented by tree depth so a
/// subtask can be nested under any node at any level. Sub-project tasks are deliberately excluded
/// — a task added here files under this project, so it should only nest under this project's own.
fn project_task_options(state: State) -> AnyView {
    project_task_tree(state, &super::ProjectScope::open(state, false), Sort::new("Task"))
        .into_iter()
        .map(|(depth, t)| {
            // Non-breaking spaces so the depth indent survives inside <option> text.
            let indent = "\u{00a0}\u{00a0}".repeat(depth);
            let value = t.id.clone();
            let label = format!("{indent}{} · {}", t.id, t.title);
            view! { <option value=value>{label}</option> }
        })
        .collect::<Vec<_>>()
        .into_any()
}
