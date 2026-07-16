//! The Tasks page: a view of the task tree (`~/.adi/mono/tasks/tasks.json`). Stat tiles plus a
//! nested table; deeper mutations stay in the `adi-mono tasks ...` CLI surface.

use adi_webapp_api::types::{NewTask, TasksState};
use leptos::prelude::*;

use crate::fetch;
use crate::state::{Flash, State, TasksForm};
use crate::ui::{
    TextField, apply_mutation, data_table, effective_label_title, flash_view, placeholder_row,
    task_tree_rows, tile, updated_text,
};

/// The Tasks page: a view of the task tree (`~/.adi/mono/tasks/tasks.json`). Stat tiles plus a
/// nested table; deeper mutations stay in the `adi-mono tasks ...` CLI surface.
pub(crate) fn tasks_view(state: State, form: TasksForm) -> AnyView {
    let tasks = state.tasks;
    let projects = state.projects;
    let secs_since = state.secs_since;
    let flash = state.flash;
    let TasksForm {
        title,
        project,
        parent,
        tag,
        details,
        busy,
    } = form;
    view! {
        <section class="adi-tiles">
            {tile("Tasks",
                move || tasks.get().map_or_else(|| "—".to_string(), |t| t.tasks.len().to_string()),
                "in the tree")}
            {tile("Ready",
                move || tasks.get().map_or_else(|| "—".to_string(), |t| task_count(&t, "ready").to_string()),
                "actionable now")}
            {tile("Blocked",
                move || tasks.get().map_or_else(|| "—".to_string(), |t| task_count(&t, "blocked").to_string()),
                "waiting on subtasks")}
            {tile("Done",
                move || tasks.get().map_or_else(|| "—".to_string(), |t| task_count(&t, "done").to_string()),
                "completed")}
        </section>

        <section class="adi-panel">
            <div class="adi-panel__head">
                <h2 class="adi-panel__title">"Task tree"</h2>
                <span class="adi-updated">{move || updated_text(state.ports, secs_since)}</span>
            </div>

            {data_table(&["Task", "ID", "Project", "Tag", "Status", "Subtasks"], move || task_rows(tasks))}
            <form class="adi-form" on:submit=move |ev| {
                ev.prevent_default();
                let t = title.get().trim().to_string();
                if t.is_empty() {
                    flash.set(Some(Flash::err("A task title is required.".to_string())));
                    return;
                }
                let det = details.get().trim().to_string();
                let par = parent.get().trim().to_string();
                let tg = tag.get().trim().to_string();
                let proj = project.get().trim().to_string();
                let body = NewTask {
                    title: t.clone(),
                    details: (!det.is_empty()).then_some(det),
                    project: (!proj.is_empty()).then_some(proj),
                    tag: (!tg.is_empty()).then_some(tg),
                    parent: (!par.is_empty()).then_some(par),
                };
                title.set(String::new());
                details.set(String::new());
                parent.set(String::new());
                tag.set(String::new());
                // Keep the project selected — filing several tasks under one project is common.
                apply_tasks(state, Some(busy), format!("Created task “{t}”."),
                    fetch::create_task(body));
            }>
                <TextField id="task-title" label="Title" placeholder="What needs doing?" wide=true
                    field_style="flex:1 1 220px; min-width:0" value=title />
                <div class="adi-field">
                    <label class="adi-field__label" for="task-project">"Project"</label>
                    <select class="adi-input" id="task-project"
                        prop:value=move || project.get()
                        on:change=move |ev| project.set(event_target_value(&ev))>
                        <option value="">"— none —"</option>
                        {move || projects.get().map(|p| p.projects.into_iter()
                            .filter(|proj| !proj.is_archived())
                            .map(|proj| {
                                let id = proj.id.clone();
                                // Ids are UUIDs — label the option with the display name instead.
                                let label = proj.name.clone();
                                view! { <option value=id>{label}</option> }
                            }).collect::<Vec<_>>()).unwrap_or_default()}
                    </select>
                </div>
                <div class="adi-field">
                    <label class="adi-field__label" for="task-parent">"Parent (subtask of)"</label>
                    <select class="adi-input" id="task-parent"
                        prop:value=move || parent.get()
                        on:change=move |ev| parent.set(event_target_value(&ev))>
                        <option value="">"— none (root) —"</option>
                        {move || tasks.get().map(|t| t.tasks.into_iter().map(|task| {
                            let id = task.id.clone();
                            let label = format!("{} · {}", task.id, task.title);
                            view! { <option value=id>{label}</option> }
                        }).collect::<Vec<_>>()).unwrap_or_default()}
                    </select>
                </div>
                <TextField id="task-tag" label="Tag" placeholder="agent name" mono=true
                    hint="= an agent name auto-starts it" value=tag />
                <TextField id="task-details" label="Details" placeholder="optional notes" wide=true
                    field_style="flex:1 1 200px; min-width:0" value=details />
                <button class="adi-btn adi-btn--primary" type="submit" prop:disabled=move || busy.get()>
                    "Add task"
                </button>
            </form>
            {flash_view(flash)}
            <div class="adi-muted" style="padding:0 18px 14px; font-size:12.5px">
                "Completing, archiving, editing, and deleting stay in the "
                <code>"adi-mono tasks"</code> " CLI."
            </div>
        </section>
    }
    .into_any()
}

/// Count tasks whose computed effective status equals `effective` (`ready`/`blocked`/`done`/`archived`).
fn task_count(state: &TasksState, effective: &str) -> usize {
    state
        .tasks
        .iter()
        .filter(|t| t.effective == effective)
        .count()
}

/// Run a task mutation (currently just create): set the returned tree and a success flash, or an
/// error flash; toggles `busy` around the request when a form is driving it.
fn apply_tasks<F>(state: State, busy: Option<RwSignal<bool>>, ok_msg: String, fut: F)
where
    F: std::future::Future<Output = Result<TasksState, String>> + 'static,
{
    apply_mutation(state, busy, ok_msg, |s, t| s.tasks.set(Some(t)), fut);
}

/// Render the task table body: a loading/empty placeholder, or the tree flattened into rows
/// (a parent immediately followed by its subtree), each indented by its depth.
fn task_rows(tasks: RwSignal<Option<TasksState>>) -> AnyView {
    let Some(state_tasks) = tasks.get() else {
        return placeholder_row("6", "Loading…");
    };
    if state_tasks.tasks.is_empty() {
        return placeholder_row(
            "6",
            "No tasks yet — add one below, or use the adi-mono tasks add CLI command.",
        );
    }

    task_tree_rows(state_tasks.tasks)
        .into_iter()
        .map(|(depth, t)| {
            let indent = format!("padding-left:{}px", depth * 20);
            let subtasks = if t.children_total > 0 {
                format!("{}/{} open", t.children_open, t.children_total)
            } else {
                String::new()
            };
            let details = t.details.unwrap_or_default();
            let label = effective_label_title(&t.effective);
            let project_cell = match t.project {
                Some(p) if !p.trim().is_empty() => {
                    view! { <span class="adi-chip adi-mono">{p}</span> }.into_any()
                }
                _ => view! { <span class="adi-muted">"—"</span> }.into_any(),
            };
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
                    <td>{project_cell}</td>
                    <td>{tag_cell}</td>
                    <td><span class="adi-tstatus" data-status=t.effective>{label}</span></td>
                    <td class="adi-mono adi-muted">{subtasks}</td>
                </tr>
            }
        })
        .collect::<Vec<_>>()
        .into_any()
}
