//! The Tasks page: a view of the task tree (`~/.adi/mono/tasks/tasks.json`) as a nested table,
//! a create form, and a collapsed block of finished tasks; deeper mutations stay in the
//! `adi-mono tasks ...` CLI surface.

use adi_webapp_api::types::{NewTask, TasksState};
use leptos::prelude::*;

use crate::fetch;
use crate::state::{Flash, State, TasksForm};
use crate::ui::{
    TextField, apply_mutation, confirm, data_table, effective_label_title, flash_view,
    placeholder_row, task_tree_rows, updated_text,
};

/// The Tasks page: a view of the task tree (`~/.adi/mono/tasks/tasks.json`) as a nested table,
/// a create form, and a collapsed block of finished tasks; deeper mutations stay in the
/// `adi-mono tasks ...` CLI surface.
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
        show_done,
    } = form;
    view! {
        <section class="adi-panel">
            <div class="adi-panel__head">
                <span class="adi-chip adi-mono" title="Open tasks, at every depth">
                    {move || tasks.get().map_or_else(|| "\u{2014}".to_string(),
                        |t| t.tasks.iter().filter(|x| !is_finished(&x.effective)).count().to_string())}
                </span>
                <span class="adi-updated">{move || updated_text(tasks, secs_since)}</span>
            </div>

            {data_table(&["Task", "ID", "Project", "Tag", "Status", "Subtasks", ""],
                move || task_rows(state, false))}
        </section>

        <section class="adi-panel">
            <div class="adi-panel__head">
                <h2 class="adi-panel__title">"New task"</h2>
            </div>

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
                    field_class="adi-field--grow" value=title />
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
                    field_class="adi-field--grow" value=details />
                <button class="adi-btn adi-btn--primary" type="submit" prop:disabled=move || busy.get()>
                    "Add task"
                </button>
            </form>
            {flash_view(flash)}
            <div class="adi-hint">
                "Archive takes a task and its subtasks off the plate; in the Done block, Reopen "
                "brings one back and Delete removes it for good (its subtasks reparent). "
                "Completing and editing stay in the " <code>"adi-mono tasks"</code> " CLI."
            </div>
        </section>

        {done_section(state, show_done)}
    }
    .into_any()
}

/// Whether a computed effective status counts as finished — the tasks that drop out of the main
/// tree into the collapsed block. `archived` rides along with `done`: both are off the plate.
pub(crate) fn is_finished(effective: &str) -> bool {
    matches!(effective, "done" | "archived")
}

/// The finished tasks: their own collapsed panel at the foot of the page, with a caret header and
/// a count. Renders nothing at all until something is actually finished, so a fresh tree stays
/// quiet. Mirrors the archive on the Projects page.
fn done_section(state: State, show: RwSignal<bool>) -> AnyView {
    view! {
        {move || {
            let n = state.tasks.get().map_or(0, |t| {
                t.tasks.iter().filter(|x| is_finished(&x.effective)).count()
            });
            (n > 0).then(|| {
                let open = show.get();
                view! {
                    <section class="adi-panel">
                        <div class="adi-panel__head">
                            <button class="adi-btn adi-btn--link" type="button"
                                aria-expanded=open.to_string()
                                on:click=move |_| show.update(|v| *v = !*v)>
                                {if open { "\u{25be}" } else { "\u{25b8}" }}" Done"
                            </button>
                            <span class="adi-chip adi-mono">{n.to_string()}</span>
                        </div>
                        {open.then(|| data_table(
                            &["Task", "ID", "Project", "Tag", "Status", "Subtasks", ""],
                            move || task_rows(state, true)))}
                    </section>
                }
                .into_any()
            })
        }}
    }
    .into_any()
}

/// Run a task mutation (create, archive, reopen): set the returned tree and a success flash, or an
/// error flash; toggles `busy` around the request when a form is driving it.
fn apply_tasks<F>(state: State, busy: Option<RwSignal<bool>>, ok_msg: String, fut: F)
where
    F: std::future::Future<Output = Result<TasksState, String>> + 'static,
{
    apply_mutation(state, busy, ok_msg, |s, t| s.tasks.set(Some(t)), fut);
}

/// Render a task table body: a loading/empty placeholder, or the tree flattened into rows (a
/// parent immediately followed by its subtree), each indented by its depth. `finished` picks the
/// side of the split — the open tree, or the collapsed Done block — and with it the trailing
/// action: Archive on a live row, Reopen on a finished one.
///
/// Each side is tree-flattened over its own subset, so an open subtask of a finished parent
/// re-roots into the main tree rather than disappearing with its parent.
fn task_rows(state: State, finished: bool) -> AnyView {
    let Some(state_tasks) = state.tasks.get() else {
        return placeholder_row("7", "Loading…");
    };
    let rows: Vec<_> = state_tasks
        .tasks
        .into_iter()
        .filter(|t| is_finished(&t.effective) == finished)
        .collect();
    if rows.is_empty() {
        return placeholder_row(
            "7",
            if finished {
                "Nothing finished yet."
            } else {
                "No open tasks — add one below, or use the adi-mono tasks add CLI command."
            },
        );
    }

    task_tree_rows(rows)
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
            let action = {
                let id = t.id.clone();
                if finished {
                    let del_id = id.clone();
                    view! {
                        <span style="display:inline-flex; gap:var(--space-2)">
                            <button class="adi-btn adi-btn--link" on:click=move |_| {
                                apply_tasks(state, None, format!("Reopened {id}."),
                                    fetch::reopen_task(id.clone()));
                            }>"Reopen"</button>
                            <button class="adi-btn adi-btn--link" style="color:var(--down)"
                                on:click=move |_| {
                                    if !confirm(&format!(
                                        "Permanently delete task {del_id}? This cannot be undone.")) {
                                        return;
                                    }
                                    apply_tasks(state, None, format!("Deleted {del_id}."),
                                        fetch::delete_task(del_id.clone()));
                                }>"Delete"</button>
                        </span>
                    }
                    .into_any()
                } else {
                    view! {
                        <button class="adi-btn adi-btn--link" on:click=move |_| {
                            apply_tasks(state, None, format!("Archived {id}."),
                                fetch::archive_task(id.clone()));
                        }>"Archive"</button>
                    }
                    .into_any()
                }
            };
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
                    <td>{action}</td>
                </tr>
            }
        })
        .collect::<Vec<_>>()
        .into_any()
}
