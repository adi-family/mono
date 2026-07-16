//! The project detail page (`/projects/<id>`): the manifest, its actions, the services read from
//! the project's `.adi/hive.yaml`, and an in-place file browser/editor scoped to the project's own
//! directory (via the isolated `adi-fs` jail).

use adi_webapp_api::types::{
    AgentsState, NewProject, NewService, NewTask, ProjectDetail, ProjectService, ProjectsState,
    SaveAgent, SaveTrigger, TasksState, TriggersState,
};
use leptos::prelude::*;
use wasm_bindgen_futures::spawn_local;

use crate::fetch;
use super::agents::{agent_actions, live_view as agent_live_view};
use super::triggers::{log_view, trigger_actions};
use super::workspaces::{
    NewHookForm, WorkspaceForm, hook_editor_view, hook_log_view, term_view, workspaces_panel,
};
use crate::routing::{Route, go_projects, open_project};
use crate::state::{
    AgentsWatch, Flash, HookEditor, HookLogView, State, TermWatch, TriggersLogView,
};
use crate::ui::{
    TextField, apply_mutation, dash, data_table, effective_label_title, flash_view, fmt_date,
    fmt_ports, placeholder_row, task_tree_rows, tile,
};

/// The project detail page (`/projects/<id>`): the manifest, its actions, and the services
/// read from the project's `.adi/hive.yaml` — what's "inside" the project.
pub(crate) fn project_detail_view(
    state: State,
    route: RwSignal<Route>,
    triggers_log: TriggersLogView,
    agents_watch: AgentsWatch,
    hook_log: HookLogView,
    term: TermWatch,
) -> AnyView {
    let State {
        project_detail,
        flash,
        ..
    } = state;
    // Two-step delete confirmation, so a hard delete needs a deliberate second click (no
    // native confirm dialog, which would need an extra web-sys feature).
    let confirm_delete = RwSignal::new(false);
    // The project-scoped task create form. Created here (once per navigation) so its inputs
    // survive the reactive re-renders of the detail body and task list.
    let task_form = TaskForm {
        title: RwSignal::new(String::new()),
        parent: RwSignal::new(String::new()),
        tag: RwSignal::new(String::new()),
        details: RwSignal::new(String::new()),
        busy: RwSignal::new(false),
    };
    // The project-scoped quick trigger create form (same lifetime rationale as the task form).
    let trigger_form = QuickTriggerForm {
        name: RwSignal::new(String::new()),
        kind: RwSignal::new(String::new()),
        code: RwSignal::new(String::new()),
        busy: RwSignal::new(false),
    };
    // The project-scoped quick agent create form (same lifetime rationale as the task form).
    let agent_form = QuickAgentForm {
        name: RwSignal::new(String::new()),
        backend: RwSignal::new(String::new()),
        system_prompt: RwSignal::new(String::new()),
        busy: RwSignal::new(false),
    };
    // The sub-project quick create form (same lifetime rationale as the task form).
    let subproject_form = QuickSubprojectForm {
        name: RwSignal::new(String::new()),
        busy: RwSignal::new(false),
    };
    // The quick service create form (same lifetime rationale as the task form).
    let service_form = QuickServiceForm {
        name: RwSignal::new(String::new()),
        run: RwSignal::new(String::new()),
        host: RwSignal::new(String::new()),
        port: RwSignal::new(String::new()),
        busy: RwSignal::new(false),
    };
    // The Workspaces panel's create forms (same lifetime rationale as the task form).
    let workspace_form = WorkspaceForm {
        name: RwSignal::new(String::new()),
        path: RwSignal::new(String::new()),
        local: RwSignal::new(false),
        busy: RwSignal::new(false),
    };
    let new_hook_form = NewHookForm {
        name: RwSignal::new(String::new()),
        template: RwSignal::new("blank".to_string()),
        busy: RwSignal::new(false),
    };
    // The hook editor (opened by the hooks table's Edit action). Created here, per
    // navigation, so a fresh project page starts with it closed.
    let hook_editor = HookEditor::new();
    view! {
        <div class="adi-bar">
            <a class="adi-btn adi-btn--link" href=Route::Projects.path()
                on:click=move |ev: web_sys::MouseEvent| {
                    if ev.meta_key() || ev.ctrl_key() || ev.shift_key() || ev.button() != 0 { return; }
                    ev.prevent_default();
                    go_projects(state, route);
                }>"← Projects"</a>
        </div>

        {move || match project_detail.get() {
            None => view! {
                <section class="adi-panel"><div class="adi-empty">"Loading…"</div></section>
            }.into_any(),
            Some(d) => detail_body(state, route, confirm_delete, service_form, d),
        }}

        {move || term_view(state, term)}

        {move || hook_log_view(hook_log)}

        {move || hook_editor_view(state, hook_editor)}

        {workspaces_panel(state, workspace_form, new_hook_form, hook_log, hook_editor, term)}

        {subprojects_panel(state, route, subproject_form)}

        {tasks_panel(state, task_form)}

        {move || agent_live_view(state, agents_watch)}

        {agents_panel(state, agent_form, agents_watch)}

        {move || log_view(triggers_log)}

        {triggers_panel(state, trigger_form, triggers_log)}

        {files_view(state)}

        {flash_view(flash)}
    }
    .into_any()
}

/// The project detail page's local task create form (title, an optional parent to nest under, and
/// optional tag/details; the project is fixed to the open project). `Copy` so it threads into the
/// panel view and its submit handler.
#[derive(Clone, Copy)]
struct TaskForm {
    title: RwSignal<String>,
    /// The id of the task to nest under (a subtask), or empty for a top-level task. The picker
    /// lists this project's whole tree, so a subtask can sit at any depth.
    parent: RwSignal<String>,
    tag: RwSignal<String>,
    details: RwSignal<String>,
    busy: RwSignal<bool>,
}

/// The quick service create form (name, run command, optional host and port; the project is
/// fixed to the open project). The service lands in the project's `.adi/hive.yaml`; editing or
/// removing one means editing that file in the Files panel. `Copy` so it threads into the
/// panel view and its submit handler.
#[derive(Clone, Copy)]
struct QuickServiceForm {
    name: RwSignal<String>,
    run: RwSignal<String>,
    host: RwSignal<String>,
    /// The explicit `http` port as typed, or empty for an auto-leased ports-manager port.
    port: RwSignal<String>,
    busy: RwSignal<bool>,
}

/// Render one loaded [`ProjectDetail`]: header + actions, key facts, description, and the
/// services table. Rebuilt whenever the `project_detail` signal changes.
fn detail_body(
    state: State,
    route: RwSignal<Route>,
    confirm_delete: RwSignal<bool>,
    service_form: QuickServiceForm,
    d: ProjectDetail,
) -> AnyView {
    let archived = d.is_archived();
    let id = d.id.clone();
    let name = d.name.clone();
    let created = fmt_date(d.created_at);
    let archived_note = d
        .archived_at
        .map_or_else(String::new, |ts| format!("archived {}", fmt_date(ts)));
    let status_label = if archived { "Archived" } else { "Active" };
    let description = d.description.clone();
    let has_hive = d.has_hive;
    let services = d.services.clone();
    let service_count = services.len();
    let reload_id = id.clone();
    let rows_id = id.clone();

    // Archive / restore action.
    let toggle_id = id.clone();
    let archive_btn = if archived {
        view! {
            <button class="adi-btn" on:click=move |_| {
                apply_detail_mutation(state, toggle_id.clone(), None,
                    format!("Restored {}.", toggle_id), fetch::unarchive_project(toggle_id.clone()));
            }>"Restore"</button>
        }.into_any()
    } else {
        view! {
            <button class="adi-btn" on:click=move |_| {
                apply_detail_mutation(state, toggle_id.clone(), None,
                    format!("Archived {}.", toggle_id), fetch::archive_project(toggle_id.clone()));
            }>"Archive"</button>
        }
        .into_any()
    };

    // Two-step delete control (reactive on `confirm_delete`).
    let del_id = id.clone();
    let delete_ctrl = move || {
        if confirm_delete.get() {
            let yes_id = del_id.clone();
            view! {
                <span class="adi-muted">"Delete permanently?"</span>
                <button class="adi-btn" style="color:var(--danger,#c0392b)" on:click=move |_| {
                    let yes_id = yes_id.clone();
                    spawn_local(async move {
                        match fetch::remove_project(yes_id.clone()).await {
                            Ok(list) => {
                                state.projects.set(Some(list));
                                state.flash.set(Some(Flash::ok(format!("Deleted {}.", yes_id))));
                                go_projects(state, route);
                            }
                            Err(e) => state.flash.set(Some(Flash::err(e))),
                        }
                    });
                }>"Yes, delete"</button>
                <button class="adi-btn adi-btn--link"
                    on:click=move |_| confirm_delete.set(false)>"Cancel"</button>
            }
            .into_any()
        } else {
            view! {
                <button class="adi-btn adi-btn--link"
                    on:click=move |_| confirm_delete.set(true)>"Delete…"</button>
            }
            .into_any()
        }
    };

    view! {
        <div class="adi-bar">
            <h1 class="adi-bar__title">{name}</h1>
            <span class="adi-chip">{status_label}</span>
            {parent_link(state, route, d.parent.clone())}
            <span class="adi-spacer" style="flex:1"></span>
            {archive_btn}
            {delete_ctrl}
        </div>

        <section class="adi-tiles">
            <div class="adi-tile">
                <div class="adi-tile__label">"ID"</div>
                <div class="adi-tile__value adi-mono" style="font-size:1.1rem">{id}</div>
                <div class="adi-tile__note">"directory under ~/.adi/mono/projects"</div>
            </div>
            {tile("Created", created, archived_note)}
            {tile("Services", service_count.to_string(), "from .adi/hive.yaml")}
        </section>

        {description.map(|text| view! {
            <section class="adi-panel">
                <div class="adi-panel__head"><h2 class="adi-panel__title">"Description"</h2></div>
                <p class="adi-muted">{text}</p>
            </section>
        })}

        <section class="adi-panel">
            <div class="adi-panel__head">
                <h2 class="adi-panel__title">"Services"</h2>
                <span class="adi-spacer"></span>
                <button class="adi-btn adi-btn--ghost" type="button"
                    title="Re-read this project's .adi/hive.yaml from disk"
                    on:click=move |_| reload_project(state, reload_id.clone())>"Reload config"</button>
                <span class="adi-updated">"the project's .adi/hive.yaml"</span>
            </div>
            {data_table(&["Service", "Host", "Ports", "Command", "Restart", ""],
                service_rows(state, rows_id, services, has_hive))}
            {service_create_form(state, service_form)}
            <div class="adi-muted" style="padding:0 18px 14px; font-size:12.5px">
                "Written to the project's " <code>".adi/hive.yaml"</code> " — the front door picks the "
                "service up from there. Edit or remove services by editing that file in the Files panel."
            </div>
        </section>
    }
    .into_any()
}

/// The header's link up to a sub-project's parent page, or nothing for a top-level project.
fn parent_link(state: State, route: RwSignal<Route>, parent: Option<String>) -> Option<AnyView> {
    let pid = parent?;
    let open_pid = pid.clone();
    let href = format!("/projects/{pid}");
    Some(
        view! {
            <a class="adi-btn adi-btn--link" href=href title="open the parent project"
                on:click=move |ev: web_sys::MouseEvent| {
                    if ev.meta_key() || ev.ctrl_key() || ev.shift_key() || ev.button() != 0 { return; }
                    ev.prevent_default();
                    open_project(state, route, open_pid.clone());
                }>{format!("↑ {pid}")}</a>
        }
        .into_any(),
    )
}

/// Rows for the services table: a message when there's no hive / no services, else one row per
/// service (host, ports as `key:port`, run command, restart policy, and a Start action for
/// services that declare a runner).
fn service_rows(
    state: State,
    project: String,
    services: Vec<ProjectService>,
    has_hive: bool,
) -> AnyView {
    if services.is_empty() {
        let msg = if has_hive {
            "This project's .adi/hive.yaml declares no services."
        } else {
            "No .adi/hive.yaml — this project has no runtime services yet."
        };
        return placeholder_row("6", msg);
    }
    services
        .into_iter()
        .map(|s| {
            let name = s.name.clone();
            let host = dash(s.host);
            let ports = fmt_ports(&s.ports);
            // Only a service with a `run` command has a runner to start/stop.
            let has_runner = s.run.is_some();
            let running = s.running;
            let run = dash(s.run);
            let restart = dash(s.restart);
            // Action reflects live state: Stop (+ a running dot) when up, Start when down.
            let action = if !has_runner {
                view! { <span class="adi-muted">"—"</span> }.into_any()
            } else if running {
                let (p, n) = (project.clone(), name.clone());
                view! {
                    <span style="color:var(--ok,#3fb950);margin-right:.5rem" title="Primary port is listening">"● Running"</span>
                    <button class="adi-btn adi-btn--ghost" type="button" title="Stop this service"
                        on:click=move |_| stop_service(state, Some(p.clone()), n.clone())>
                        "Stop"
                    </button>
                }
                .into_any()
            } else {
                let (p, n) = (project.clone(), name.clone());
                view! {
                    <button class="adi-btn adi-btn--ghost" type="button"
                        title="Run this service's command with its ports-manager port"
                        on:click=move |_| start_service(state, Some(p.clone()), n.clone())>
                        "Start"
                    </button>
                }
                .into_any()
            };
            view! {
                <tr>
                    <td class="adi-mono">{name}</td>
                    <td class="adi-mono">{host}</td>
                    <td class="adi-mono adi-table__port">{ports}</td>
                    <td class="adi-mono adi-muted">{run}</td>
                    <td class="adi-muted">{restart}</td>
                    <td>{action}</td>
                </tr>
            }
        })
        .collect::<Vec<_>>()
        .into_any()
}

/// The quick service create form under the Services table: name + run command, an optional
/// proxied host, and an optional explicit port (empty → a ports-manager-leased one). Posts to
/// `/api/hive/create`, which writes the service into the project's `.adi/hive.yaml` and
/// returns the fresh detail.
fn service_create_form(state: State, form: QuickServiceForm) -> AnyView {
    let QuickServiceForm {
        name,
        run,
        host,
        port,
        busy,
    } = form;
    view! {
        <form class="adi-form" on:submit=move |ev| {
            ev.prevent_default();
            let id = state.current_project.get_untracked();
            if id.is_empty() {
                return;
            }
            let nm = name.get().trim().to_string();
            if nm.is_empty() {
                state.flash.set(Some(Flash::err("A service name is required.".to_string())));
                return;
            }
            let run_cmd = run.get().trim().to_string();
            if run_cmd.is_empty() {
                state.flash.set(Some(Flash::err("A run command is required.".to_string())));
                return;
            }
            let host_v = host.get().trim().to_string();
            let port_txt = port.get().trim().to_string();
            let port_v = if port_txt.is_empty() {
                None
            } else {
                match port_txt.parse::<u16>() {
                    Ok(p) => Some(p),
                    Err(_) => {
                        state.flash.set(Some(Flash::err(
                            "The port must be a number (1–65535), or empty for an auto-leased one.".to_string(),
                        )));
                        return;
                    }
                }
            };
            let body = NewService {
                project: id,
                name: nm.clone(),
                run: run_cmd,
                host: (!host_v.is_empty()).then_some(host_v),
                port: port_v,
                working_dir: None,
                restart: None,
            };
            name.set(String::new());
            run.set(String::new());
            host.set(String::new());
            port.set(String::new());
            apply_mutation(state, Some(busy), format!("Added service “{nm}”."),
                |s: State, d: ProjectDetail| s.project_detail.set(Some(d)), fetch::create_service(body));
        }>
            <TextField id="pservice-name" label="Name" placeholder="api" mono=true
                hint="the key under services:" value=name />
            <TextField id="pservice-run" label="Command" placeholder="bun run start" mono=true wide=true
                field_style="flex:1 1 260px; min-width:0"
                hint="runs as sh -c with PORT injected" value=run />
            <TextField id="pservice-host" label="Host" placeholder="myapp.adi" mono=true
                hint="optional — routed by the front door" value=host />
            <TextField id="pservice-port" label="Port" placeholder="auto" mono=true numeric=true
                hint="optional — auto-leased when empty" value=port />
            <button class="adi-btn adi-btn--primary" type="submit" prop:disabled=move || busy.get()>
                "Add service"
            </button>
        </form>
    }
    .into_any()
}

/// Run a detail-page mutation (archive/restore, sub-project create) that returns the fresh
/// project list, then re-fetch this project's detail so the page reflects the change; flashes
/// success or error. Toggles `busy` around the request when a form is driving it.
fn apply_detail_mutation<F>(state: State, id: String, busy: Option<RwSignal<bool>>, ok_msg: String, fut: F)
where
    F: std::future::Future<Output = Result<ProjectsState, String>> + 'static,
{
    if let Some(busy) = busy {
        busy.set(true);
    }
    spawn_local(async move {
        match fut.await {
            Ok(list) => {
                state.projects.set(Some(list));
                if let Ok(d) = fetch::project_detail(&id).await {
                    state.project_detail.set(Some(d));
                }
                state.flash.set(Some(Flash::ok(ok_msg)));
            }
            Err(e) => state.flash.set(Some(Flash::err(e))),
        }
        if let Some(busy) = busy {
            busy.set(false);
        }
    });
}

/// Start a service's runner on the backend (its `run` command, with the ports-manager `PORT`
/// injected), then refresh the project page so its status can flip to running.
fn start_service(state: State, project: Option<String>, service: String) {
    spawn_local(async move {
        match fetch::start_service(project.clone(), service.clone()).await {
            Ok(r) => {
                let at = r.port.map_or(String::new(), |p| format!(" on :{p}"));
                state
                    .flash
                    .set(Some(Flash::ok(format!("Started {}{at}.", r.service))));
                if let Some(id) = project {
                    reload_project(state, id);
                }
            }
            Err(e) => state
                .flash
                .set(Some(Flash::err(format!("Couldn't start {service}: {e}")))),
        }
    });
}

/// Stop a running service on the backend (kill its port's listener), then refresh the project page.
fn stop_service(state: State, project: Option<String>, service: String) {
    spawn_local(async move {
        match fetch::stop_service(project.clone(), service.clone()).await {
            Ok(r) => {
                state
                    .flash
                    .set(Some(Flash::ok(format!("Stopped {}.", r.service))));
                if let Some(id) = project {
                    reload_project(state, id);
                }
            }
            Err(e) => state
                .flash
                .set(Some(Flash::err(format!("Couldn't stop {service}: {e}")))),
        }
    });
}

/// Re-fetch one project's detail — which re-reads its `.adi/hive.yaml` from disk (re-running any
/// `bash`…`` port commands) — and refresh the project page.
fn reload_project(state: State, id: String) {
    spawn_local(async move {
        match fetch::project_detail(&id).await {
            Ok(d) => {
                state.project_detail.set(Some(d));
                state
                    .flash
                    .set(Some(Flash::ok("Reloaded project config.".to_string())));
            }
            Err(e) => state.flash.set(Some(Flash::err(format!(
                "Couldn't reload project config: {e}"
            )))),
        }
    });
}

// ---- sub-projects (registered projects nested under this one) -------------------------

/// The project detail page's quick sub-project create form (just a name — the id is generated
/// server-side and the parent is fixed to the open project). Descriptions and deeper nesting
/// live on the Projects page. `Copy` so it threads into the panel view and its submit handler.
#[derive(Clone, Copy)]
struct QuickSubprojectForm {
    name: RwSignal<String>,
    busy: RwSignal<bool>,
}

/// The Sub-projects panel on a project's detail page: the projects nested directly under this
/// one (served in the detail payload), each opening its own detail page, plus a quick create
/// form pre-scoped to the open project as the parent.
fn subprojects_panel(state: State, route: RwSignal<Route>, form: QuickSubprojectForm) -> AnyView {
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
                // The server generates the sub-project's id (a UUID); only the name is sent.
                let body = NewProject {
                    name: display.clone(),
                    description: None,
                    parent: Some(parent.clone()),
                };
                name.set(String::new());
                // The mutation returns the fresh project list; re-fetching the detail then pulls
                // the new sub-project into this panel.
                apply_detail_mutation(state, parent, Some(busy), format!("Registered sub-project {display}."),
                    fetch::create_project(body));
            }>
                <TextField id="psub-name" label="Name" placeholder="My Sub-project" wide=true
                    field_style="flex:1 1 220px; min-width:0" value=name />
                <button class="adi-btn adi-btn--primary" type="submit" prop:disabled=move || busy.get()>
                    "Add sub-project"
                </button>
            </form>
            <div class="adi-muted" style="padding:0 18px 14px; font-size:12.5px">
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
            let href = format!("/projects/{id}");
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

// ---- project tasks (the shared task tree, filtered to this project) ------------------

/// The Tasks panel on a project's detail page: the tasks filed under this project (from the shared
/// task tree at `/api/tasks`) plus a create form pre-scoped to it, so a task added here gets the
/// project's Jira-style `<KEY>-<n>` id without the user having to pick a project.
fn tasks_panel(state: State, form: TaskForm) -> AnyView {
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
                    field_style="flex:1 1 220px; min-width:0" value=title />
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
                    field_style="flex:1 1 200px; min-width:0" value=details />
                <button class="adi-btn adi-btn--primary" type="submit" prop:disabled=move || busy.get()>
                    "Add task"
                </button>
            </form>
            <div class="adi-muted" style="padding:0 18px 14px; font-size:12.5px">
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

// ---- project agents (the shared agent list, filtered to this project) -----------------

/// The project detail page's quick agent create form (name, backend, system prompt; the project
/// is fixed to the open project). Full editing — models, permission modes, backend params —
/// lives on the Agents page. `Copy` so it threads into the panel view and its submit handler.
#[derive(Clone, Copy)]
struct QuickAgentForm {
    name: RwSignal<String>,
    backend: RwSignal<String>,
    system_prompt: RwSignal<String>,
    busy: RwSignal<bool>,
}

/// The Agents panel on a project's detail page: the agents filed under this project (from the
/// shared list at `/api/agents`) with live Run/View/Stop actions, plus a quick create form
/// pre-scoped to it.
fn agents_panel(state: State, form: QuickAgentForm, watch: AgentsWatch) -> AnyView {
    let QuickAgentForm {
        name,
        backend,
        system_prompt,
        busy,
    } = form;
    let agents = state.agents;
    view! {
        <section class="adi-panel">
            <div class="adi-panel__head">
                <h2 class="adi-panel__title">"Agents"</h2>
                <span class="adi-updated">"filed under this project"</span>
            </div>
            {data_table(&["Name", "Backend", "Model", "Status", ""], move || project_agent_rows(state, watch))}
            <form class="adi-form" on:submit=move |ev| {
                ev.prevent_default();
                let id = state.current_project.get_untracked();
                if id.is_empty() {
                    return;
                }
                let nm = name.get().trim().to_string();
                if nm.is_empty() {
                    state.flash.set(Some(Flash::err("An agent name is required.".to_string())));
                    return;
                }
                let be = backend.get().trim().to_string();
                if be.is_empty() {
                    state.flash.set(Some(Flash::err("Pick a backend.".to_string())));
                    return;
                }
                let body = SaveAgent {
                    name: nm.clone(),
                    backend: be,
                    system_prompt: system_prompt.get(),
                    tools: String::new(),
                    model: None,
                    permission_mode: None,
                    temperature: None,
                    max_turns: None,
                    tags: Vec::new(),
                    starred: false,
                    project: Some(id),
                    extra: std::collections::BTreeMap::new(),
                };
                name.set(String::new());
                system_prompt.set(String::new());
                apply_mutation(state, Some(busy), format!("Created agent “{nm}”."),
                    |s: State, a: AgentsState| s.agents.set(Some(a)), fetch::save_agent(body));
            }>
                <TextField id="pagent-name" label="Name" placeholder="athz-solver" mono=true
                    hint="a task tagged this name auto-starts it" value=name />
                <div class="adi-field">
                    <label class="adi-field__label" for="pagent-backend">"Backend"</label>
                    <select class="adi-input" id="pagent-backend"
                        prop:value=move || backend.get()
                        on:change=move |ev| backend.set(event_target_value(&ev))>
                        <option value="">"— pick a backend —"</option>
                        {move || agents.get().map(|a| a.form.backends.into_iter().map(|b| {
                            let id = b.id.clone();
                            view! { <option value=id>{b.label}</option> }
                        }).collect::<Vec<_>>()).unwrap_or_default()}
                    </select>
                </div>
                <TextField id="pagent-prompt" label="System prompt" placeholder="optional seed prompt" wide=true
                    field_style="flex:1 1 260px; min-width:0" value=system_prompt />
                <button class="adi-btn adi-btn--primary" type="submit" prop:disabled=move || busy.get()>
                    "Add agent"
                </button>
            </form>
            <div class="adi-muted" style="padding:0 18px 14px; font-size:12.5px">
                "These appear in the global " <code>"Agents"</code> " list too. Models, permission
                 modes, and other backend params live on the Agents page."
            </div>
        </section>
    }
    .into_any()
}

/// Rows for the project's agent table: this project's agents with the shared Run/View/Stop
/// actions. Loading/empty placeholders otherwise.
fn project_agent_rows(state: State, watch: AgentsWatch) -> AnyView {
    let id = state.current_project.get();
    let Some(st) = state.agents.get() else {
        return placeholder_row("5", "Loading…");
    };
    let mine: Vec<_> = st
        .agents
        .into_iter()
        .filter(|a| a.project.as_deref() == Some(id.as_str()))
        .collect();
    if mine.is_empty() {
        return placeholder_row("5", "No agents in this project yet — add one below.");
    }
    mine.into_iter()
        .map(|a| {
            let name_disp = if a.starred {
                format!("★ {}", a.name)
            } else {
                a.name.clone()
            };
            let backend = a.backend.clone();
            let model = a.model.clone().unwrap_or_default();
            let status = if a.running {
                view! { <span class="adi-tstatus" data-status="ready">"Running"</span> }.into_any()
            } else {
                view! { <span class="adi-muted">"—"</span> }.into_any()
            };
            view! {
                <tr>
                    <td>{name_disp}</td>
                    <td class="adi-mono">{backend}</td>
                    <td class="adi-mono adi-muted">{model}</td>
                    <td>{status}</td>
                    <td style="text-align:right; white-space:nowrap">
                        {agent_actions(state, watch, &a)}
                    </td>
                </tr>
            }
        })
        .collect::<Vec<_>>()
        .into_any()
}

// ---- project triggers (the shared trigger list, filtered to this project) -------------

/// The project detail page's quick trigger create form (name, kind, code; the project is fixed
/// to the open project). Full editing — description, secrets, enable/disable — lives on the
/// Triggers page. `Copy` so it threads into the panel view and its submit handler.
#[derive(Clone, Copy)]
struct QuickTriggerForm {
    name: RwSignal<String>,
    kind: RwSignal<String>,
    code: RwSignal<String>,
    busy: RwSignal<bool>,
}

/// The Triggers panel on a project's detail page: the triggers filed under this project (from
/// the shared list at `/api/triggers`) with live Fire/Log/Enable actions, plus a quick create
/// form pre-scoped to it.
fn triggers_panel(state: State, form: QuickTriggerForm, log: TriggersLogView) -> AnyView {
    let QuickTriggerForm {
        name,
        kind,
        code,
        busy,
    } = form;
    let triggers = state.triggers;
    view! {
        <section class="adi-panel">
            <div class="adi-panel__head">
                <h2 class="adi-panel__title">"Triggers"</h2>
                <span class="adi-updated">"filed under this project"</span>
            </div>
            {data_table(&["Name", "Kind", "Status", "Last fired", ""], move || project_trigger_rows(state, log))}
            <form class="adi-form" on:submit=move |ev| {
                ev.prevent_default();
                let id = state.current_project.get_untracked();
                if id.is_empty() {
                    return;
                }
                let nm = name.get().trim().to_string();
                if nm.is_empty() {
                    state.flash.set(Some(Flash::err("A trigger name is required.".to_string())));
                    return;
                }
                let kd = kind.get().trim().to_string();
                if kd.is_empty() {
                    state.flash.set(Some(Flash::err("Pick a kind.".to_string())));
                    return;
                }
                let body = SaveTrigger {
                    name: nm.clone(),
                    kind: kd,
                    code: code.get(),
                    description: String::new(),
                    enabled: true,
                    project: Some(id),
                    extra: std::collections::BTreeMap::new(),
                };
                name.set(String::new());
                code.set(String::new());
                apply_mutation(state, Some(busy), format!("Created trigger “{nm}”."),
                    |s: State, ts: TriggersState| s.triggers.set(Some(ts)), fetch::save_trigger(body));
            }>
                <TextField id="ptrigger-name" label="Name" placeholder="deploy-hook" mono=true
                    hint="also the webhook URL segment" value=name />
                <div class="adi-field">
                    <label class="adi-field__label" for="ptrigger-kind">"Kind"</label>
                    <select class="adi-input" id="ptrigger-kind"
                        prop:value=move || kind.get()
                        on:change=move |ev| kind.set(event_target_value(&ev))>
                        <option value="">"— pick a kind —"</option>
                        {move || triggers.get().map(|t| t.kinds.into_iter().map(|k| {
                            let id = k.id.clone();
                            view! { <option value=id>{k.label}</option> }
                        }).collect::<Vec<_>>()).unwrap_or_default()}
                    </select>
                </div>
                <TextField id="ptrigger-code" label="Code block" placeholder="echo deployed" mono=true wide=true
                    field_style="flex:1 1 260px; min-width:0"
                    hint="runs as sh -c, detached" value=code />
                <button class="adi-btn adi-btn--primary" type="submit" prop:disabled=move || busy.get()>
                    "Add trigger"
                </button>
            </form>
            <div class="adi-muted" style="padding:0 18px 14px; font-size:12.5px">
                "These appear in the global " <code>"Triggers"</code> " list too. Webhook triggers are "
                "live at " <code>"/api/hooks/<name>"</code> "; secrets, descriptions, and editing live "
                "on the Triggers page."
            </div>
        </section>
    }
    .into_any()
}

/// Rows for the project's trigger table: this project's triggers with the shared
/// Fire/Log/Enable-Disable actions. Loading/empty placeholders otherwise.
fn project_trigger_rows(state: State, log: TriggersLogView) -> AnyView {
    let id = state.current_project.get();
    let Some(st) = state.triggers.get() else {
        return placeholder_row("5", "Loading…");
    };
    let mine: Vec<_> = st
        .triggers
        .into_iter()
        .filter(|t| t.project.as_deref() == Some(id.as_str()))
        .collect();
    if mine.is_empty() {
        return placeholder_row("5", "No triggers in this project yet — add one below.");
    }
    mine.into_iter()
        .map(|t| {
            let kind = t.kind.clone();
            let hook_hint = (t.kind == "webhook").then(|| format!("/api/hooks/{}", t.name));
            let status = if t.enabled { "Enabled" } else { "Disabled" };
            let status_data = if t.enabled { "ready" } else { "archived" };
            let fired = t.last_fired_at.map_or_else(|| "—".to_string(), fmt_date);
            let description = t.description.clone();
            view! {
                <tr>
                    <td title=description>
                        <span>{t.name.clone()}</span>
                        {hook_hint.map(|h| view! {
                            <span class="adi-muted adi-mono" style="font-size:11.5px; display:block">{h}</span>
                        })}
                    </td>
                    <td class="adi-mono">{kind}</td>
                    <td><span class="adi-tstatus" data-status=status_data>{status}</span></td>
                    <td class="adi-mono adi-muted">{fired}</td>
                    <td style="text-align:right; white-space:nowrap">
                        {trigger_actions(state, log, &t)}
                    </td>
                </tr>
            }
        })
        .collect::<Vec<_>>()
        .into_any()
}

// ---- project files (browse + edit the files under a project's own directory) --------

/// Load the listing for directory `path` (relative to the project root) into the browser. On
/// success the current `dir` follows the server's normalized path; on failure it flashes.
pub(crate) async fn load_dir(state: State, id: String, path: String) {
    match fetch::list_files(&id, &path).await {
        Ok(listing) => {
            state.files.dir.set(listing.path.clone());
            state.files.listing.set(Some(listing));
        }
        Err(e) => state.flash.set(Some(Flash::err(e))),
    }
}

/// Navigate the browser into directory `path` (a dir click or the "up" control).
fn open_dir(state: State, path: String) {
    let id = state.current_project.get_untracked();
    if !id.is_empty() {
        spawn_local(load_dir(state, id, path));
    }
}

/// Open file `path` in the editor, loading its content into the buffer (and remembering it as
/// the baseline so edits are detectable).
fn open_file(state: State, path: String) {
    let id = state.current_project.get_untracked();
    if id.is_empty() {
        return;
    }
    state.files.busy.set(true);
    spawn_local(async move {
        match fetch::read_file(&id, &path).await {
            Ok(fc) => {
                state.files.open.set(Some(fc.path.clone()));
                state.files.original.set(fc.content.clone());
                state.files.buffer.set(fc.content);
            }
            Err(e) => state.flash.set(Some(Flash::err(e))),
        }
        state.files.busy.set(false);
    });
}

/// Save the editor buffer back to the open file, then refresh the listing so its size/modified
/// update. Resets the baseline to the saved content so the dirty state clears.
fn save_file(state: State) {
    let id = state.current_project.get_untracked();
    let Some(path) = state.files.open.get_untracked() else {
        return;
    };
    if id.is_empty() {
        return;
    }
    let content = state.files.buffer.get_untracked();
    state.files.busy.set(true);
    spawn_local(async move {
        match fetch::write_file(&id, &path, &content).await {
            Ok(fc) => {
                state.files.original.set(fc.content.clone());
                state.files.buffer.set(fc.content);
                state.flash.set(Some(Flash::ok(format!("Saved {path}."))));
                load_dir(state, id, state.files.dir.get_untracked()).await;
            }
            Err(e) => state.flash.set(Some(Flash::err(e))),
        }
        state.files.busy.set(false);
    });
}

/// Close the editor, discarding the buffer (a fresh open reloads from disk anyway).
fn close_file(state: State) {
    state.files.open.set(None);
    state.files.original.set(String::new());
    state.files.buffer.set(String::new());
}

/// Join a directory path and an entry name into a project-relative path (the root is `""`).
fn join_rel(dir: &str, name: &str) -> String {
    if dir.is_empty() {
        name.to_string()
    } else {
        format!("{dir}/{name}")
    }
}

/// The Files panel on a project's detail page: a breadcrumb + directory listing scoped to the
/// project's own directory (via the isolated jail), plus an in-place editor for the selected
/// text file — so `.adi/hive.yaml` (and anything beside it) is editable here.
fn files_view(state: State) -> AnyView {
    let files = state.files;
    view! {
        <section class="adi-panel">
            <div class="adi-panel__head">
                <h2 class="adi-panel__title">"Files"</h2>
                <span class="adi-spacer"></span>
                <button class="adi-btn adi-btn--ghost" type="button" prop:disabled=move || files.busy.get()
                    on:click=move |_| open_dir(state, files.dir.get_untracked())>"Reload"</button>
            </div>
            <div class="adi-panel__body">
                {move || crumbs_view(state)}
            </div>
            {data_table(&["Name", "Size", "Modified"], move || file_rows(state))}
            {move || match files.open.get() {
                None => view! {
                    <div class="adi-panel__body">
                        <span class="adi-muted">"Select a file above to view or edit it. Directories open in place; there's no going outside this project."</span>
                    </div>
                }.into_any(),
                Some(path) => editor_view(state, path),
            }}
        </section>
    }
    .into_any()
}

/// The breadcrumb trail for the file browser: the project root plus each segment of the current
/// directory, every ancestor clickable to jump straight there.
fn crumbs_view(state: State) -> AnyView {
    let dir = state.files.dir.get();
    let id = state.current_project.get();
    let mut crumbs: Vec<(String, String)> = vec![(id, String::new())]; // (label, target dir)
    let mut acc = String::new();
    if !dir.is_empty() {
        for segment in dir.split('/') {
            acc = join_rel(&acc, segment);
            crumbs.push((segment.to_string(), acc.clone()));
        }
    }
    let last = crumbs.len() - 1;
    view! {
        <div class="adi-crumbs">
            {crumbs.into_iter().enumerate().map(|(i, (label, target))| {
                let sep = (i > 0).then(|| view! { <span class="adi-crumbs__sep">"/"</span> });
                let node = if i == last {
                    view! { <span class="adi-crumbs__here">{label}</span> }.into_any()
                } else {
                    view! {
                        <a class="adi-btn adi-btn--link" href="#"
                            on:click=move |ev: web_sys::MouseEvent| {
                                ev.prevent_default();
                                open_dir(state, target.clone());
                            }>{label}</a>
                    }.into_any()
                };
                view! { {sep}{node} }
            }).collect::<Vec<_>>()}
        </div>
    }
    .into_any()
}

/// Rows for the file listing: an "up" row when not at the root, then directories (which open in
/// place) and files (which open in the editor), with size and modified date.
fn file_rows(state: State) -> AnyView {
    let files = state.files;
    let Some(listing) = files.listing.get() else {
        return placeholder_row("3", "Loading…");
    };
    let dir = listing.path.clone();
    let mut rows: Vec<AnyView> = Vec::new();

    // An "up" row to the parent directory, when there is one.
    if let Some(parent) = listing.parent.clone() {
        rows.push(
            view! {
                <tr>
                    <td>
                        <a class="adi-btn adi-btn--link adi-filerow adi-filerow--dir" href="#"
                            on:click=move |ev: web_sys::MouseEvent| {
                                ev.prevent_default();
                                open_dir(state, parent.clone());
                            }>
                            <span class="adi-filerow__icon">"↑"</span><span>".."</span>
                        </a>
                    </td>
                    <td class="adi-muted">"—"</td>
                    <td class="adi-muted">"—"</td>
                </tr>
            }
            .into_any(),
        );
    }

    if listing.entries.is_empty() && listing.parent.is_none() {
        return placeholder_row("3", "This project directory is empty.");
    }

    for entry in listing.entries {
        let path = join_rel(&dir, &entry.name);
        let modified = entry.modified.map_or_else(|| "—".to_string(), fmt_date);
        let open = state.files.open.get();
        let is_open = open.as_deref() == Some(path.as_str());
        if entry.is_dir {
            rows.push(view! {
                <tr>
                    <td>
                        <a class="adi-btn adi-btn--link adi-filerow adi-filerow--dir" href="#"
                            on:click=move |ev: web_sys::MouseEvent| {
                                ev.prevent_default();
                                open_dir(state, path.clone());
                            }>
                            <span class="adi-filerow__icon">"▸"</span><span>{entry.name}"/"</span>
                        </a>
                    </td>
                    <td class="adi-muted">"—"</td>
                    <td class="adi-mono adi-muted">{modified}</td>
                </tr>
            }.into_any());
        } else {
            let size = fmt_size(entry.size);
            rows.push(
                view! {
                    <tr>
                        <td>
                            <a class="adi-btn adi-btn--link adi-filerow" href="#"
                                aria-current=move || if is_open { "true" } else { "false" }
                                on:click=move |ev: web_sys::MouseEvent| {
                                    ev.prevent_default();
                                    open_file(state, path.clone());
                                }>
                                <span class="adi-filerow__icon">"·"</span><span>{entry.name}</span>
                            </a>
                        </td>
                        <td class="adi-mono adi-muted">{size}</td>
                        <td class="adi-mono adi-muted">{modified}</td>
                    </tr>
                }
                .into_any(),
            );
        }
    }
    rows.into_any()
}

/// The in-place editor for the open file: a toolbar (path, dirty state, Save/Reload/Close) and a
/// monospace textarea bound to the buffer.
fn editor_view(state: State, path: String) -> AnyView {
    let files = state.files;
    let dirty = move || files.buffer.get() != files.original.get();
    let reload_path = path.clone();
    view! {
        <div class="adi-form" style="justify-content:flex-start; align-items:center">
            <span class="adi-chip adi-mono">{path}</span>
            <span class="adi-muted" style="font-size:13px">
                {move || if dirty() { "unsaved changes".to_string() } else { "saved".to_string() }}
            </span>
            <span class="adi-spacer" style="flex:1"></span>
            <button class="adi-btn adi-btn--primary" type="button"
                prop:disabled=move || files.busy.get() || !dirty()
                on:click=move |_| save_file(state)>"Save"</button>
            <button class="adi-btn adi-btn--ghost" type="button"
                prop:disabled=move || files.busy.get()
                on:click=move |_| open_file(state, reload_path.clone())>"Reload"</button>
            <button class="adi-btn adi-btn--link" type="button"
                on:click=move |_| close_file(state)>"Close"</button>
        </div>
        <div class="adi-panel__body">
            <textarea class="adi-textarea" spellcheck="false" autocomplete="off"
                prop:value=move || files.buffer.get()
                on:input=move |ev| files.buffer.set(event_target_value(&ev))></textarea>
        </div>
    }
    .into_any()
}

/// Format a byte count as `N B` / `N.N KB` / `N.N MB`.
fn fmt_size(bytes: u64) -> String {
    #[allow(clippy::cast_precision_loss)]
    let n = bytes as f64;
    if bytes < 1024 {
        format!("{bytes} B")
    } else if bytes < 1024 * 1024 {
        format!("{:.1} KB", n / 1024.0)
    } else {
        format!("{:.1} MB", n / (1024.0 * 1024.0))
    }
}
