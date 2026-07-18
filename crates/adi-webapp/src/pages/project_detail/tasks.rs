//! The Tasks panel of the project detail page.

use adi_webapp_api::types::{NewTask, TasksState};
use leptos::prelude::*;

use crate::fetch;
use crate::state::{Flash, State};
use crate::ui::{
    TextField, apply_mutation, data_table, effective_label_title, placeholder_row, task_tree_rows,
};

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
    pub(crate) details: RwSignal<String>,
    pub(crate) busy: RwSignal<bool>,
}

/// The Tasks panel on a project's detail page: the tasks filed under this project (from the shared
/// task tree at `/api/tasks`) plus a create form pre-scoped to it, so a task added here gets the
/// project's Jira-style `<KEY>-<n>` id without the user having to pick a project.
pub(crate) fn tasks_panel(state: State, form: TaskForm) -> AnyView {
    let TaskForm {
        title,
        parent,
        tag,
        details,
        busy,
    } = form;
    view! {
        <section class="adi-panel">
            <div class="adi-panel__head">
                <h2 class="adi-panel__title">"Tasks"</h2>
                <span class="adi-updated">"filed under this project"</span>
            </div>
            {data_table(&["Task", "ID", "Tag", "Status", "Subtasks"], move || project_task_rows(state))}
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
                let body = NewTask {
                    title: t.clone(),
                    details: (!det.is_empty()).then_some(det),
                    project: Some(id),
                    tag: (!tg.is_empty()).then_some(tg),
                    parent: (!par.is_empty()).then_some(par),
                };
                title.set(String::new());
                parent.set(String::new());
                tag.set(String::new());
                details.set(String::new());
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

/// This project's tasks, filtered from the shared tree and flattened into depth-annotated tree
/// order (so subtasks nest under their parent, at any depth).
fn project_task_tree(state: State) -> Vec<(usize, adi_webapp_api::types::TaskRow)> {
    let id = state.current_project.get();
    let Some(tasks) = state.tasks.get() else {
        return Vec::new();
    };
    let mine: Vec<_> = tasks
        .tasks
        .into_iter()
        .filter(|t| t.project.as_deref() == Some(id.as_str()))
        .collect();
    task_tree_rows(mine)
}

/// Rows for the project's task table: this project's tasks as a nested tree — each row indented by
/// its depth, with its title, Jira id, tag, effective status, and subtask rollup. Loading/empty
/// placeholders otherwise.
fn project_task_rows(state: State) -> AnyView {
    if state.tasks.get().is_none() {
        return placeholder_row("5", "Loading…");
    }
    let tree = project_task_tree(state);
    if tree.is_empty() {
        return placeholder_row("5", "No tasks in this project yet — add one below.");
    }
    tree.into_iter()
        .map(|(depth, t)| {
            let indent = format!("padding-left:{}px", depth * 20);
            let subtasks = if t.children_total > 0 {
                format!("{}/{} open", t.children_open, t.children_total)
            } else {
                String::new()
            };
            let details = t.details.unwrap_or_default();
            let label = effective_label_title(&t.effective);
            let tag_cell = match t.tag {
                Some(tg) if !tg.trim().is_empty() => {
                    view! { <span class="adi-chip adi-mono">{tg}</span> }.into_any()
                }
                _ => view! { <span class="adi-muted">"—"</span> }.into_any(),
            };
            view! {
                <tr>
                    <td title=details><span style=indent>{t.title}</span></td>
                    <td class="adi-mono adi-muted">{t.id}</td>
                    <td>{tag_cell}</td>
                    <td><span class="adi-tstatus" data-status=t.effective>{label}</span></td>
                    <td class="adi-mono adi-muted">{subtasks}</td>
                </tr>
            }
        })
        .collect::<Vec<_>>()
        .into_any()
}

/// `<option>`s for the parent picker: every task in this project, indented by tree depth so a
/// subtask can be nested under any node at any level.
fn project_task_options(state: State) -> AnyView {
    project_task_tree(state)
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
