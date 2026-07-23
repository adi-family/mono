//! The project detail page (`/projects/<id>`): the manifest, its actions, the services read from
//! the project's `.adi/hive.yaml`, and an in-place file browser/editor scoped to the project's own
//! directory (via the isolated `adi-fs` jail).

use adi_webapp_api::types::{ProjectDetail, ProjectsState};
use leptos::prelude::*;
use wasm_bindgen_futures::spawn_local;

use super::agents::live_view as agent_live_view;
use super::tools::{tool_editor_view, tool_run_view};
use super::triggers::log_view;
use super::workspaces::{
    NewHookForm, WorkspaceForm, hook_editor_view, hook_log_view, term_view, workspaces_panel,
};
use crate::fetch;
use crate::routing::{ProjectSection, Route, go_projects, open_project, open_project_section};
use crate::state::{
    AgentsForm, AgentsWatch, Flash, HookEditor, HookLogView, SecretsForm, State, TermWatch,
    ToolEditor, ToolRunView, ToolsForm, TriggersLogView,
};
use crate::ui::{data_table, flash_view, fmt_date};

mod agents_panel;
mod files;
mod secrets_panel;
mod services;
mod subprojects;
mod tasks;
mod tools_panel;
mod triggers;

use agents_panel::{QuickAgentForm, agents_panel};
use files::files_view;
use secrets_panel::secrets_panel;
use services::{QuickServiceForm, service_create_form, service_rows};
use subprojects::{QuickSubprojectForm, subprojects_panel};
use tasks::{TaskForm, tasks_panel};
use tools_panel::tools_panel;
use triggers::{QuickTriggerForm, triggers_panel};

/// The project detail page (`/projects/<id>`): the manifest, its actions, and the services
/// read from the project's `.adi/hive.yaml` — what's "inside" the project.
pub(crate) fn project_detail_view(
    state: State,
    route: RwSignal<Route>,
    triggers_log: TriggersLogView,
    agents_watch: AgentsWatch,
    agents_form: AgentsForm,
    hook_log: HookLogView,
    term: TermWatch,
    tool_editor: ToolEditor,
    tool_run: ToolRunView,
) -> AnyView {
    let State {
        project_detail,
        flash,
        ..
    } = state;
    // Two-step delete confirmation, so a hard delete needs a deliberate second click (no
    // native confirm dialog, which would need an extra web-sys feature).
    let confirm_delete = RwSignal::new(false);
    // Page-scoped form signals survive reactive re-renders without leaking across navigation.
    let task_form = TaskForm {
        title: RwSignal::new(String::new()),
        parent: RwSignal::new(String::new()),
        tag: RwSignal::new(String::new()),
        details: RwSignal::new(String::new()),
        busy: RwSignal::new(false),
    };
    let trigger_form = QuickTriggerForm {
        name: RwSignal::new(String::new()),
        kind: RwSignal::new(String::new()),
        code: RwSignal::new(String::new()),
        busy: RwSignal::new(false),
    };
    let agent_form = QuickAgentForm {
        name: RwSignal::new(String::new()),
        backend: RwSignal::new(String::new()),
        system_prompt: RwSignal::new(String::new()),
        busy: RwSignal::new(false),
    };
    let subproject_form = QuickSubprojectForm {
        name: RwSignal::new(String::new()),
        busy: RwSignal::new(false),
    };
    let service_form = QuickServiceForm {
        name: RwSignal::new(String::new()),
        kind: RwSignal::new("script".to_string()),
        run: RwSignal::new(String::new()),
        host: RwSignal::new(String::new()),
        port: RwSignal::new(String::new()),
        image: RwSignal::new(String::new()),
        container_port: RwSignal::new(String::new()),
        volumes: RwSignal::new(String::new()),
        env: RwSignal::new(String::new()),
        pull: RwSignal::new(String::new()),
        busy: RwSignal::new(false),
    };
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
    let hook_editor = HookEditor::new();
    // The Tools panel's create/link form, pre-scoped to this project on submit.
    let tool_form = ToolsForm::new();
    // The Secrets panel's create form + reveal cache, pre-scoped to this project on submit.
    let secrets_form = SecretsForm::new();
    view! {
        // Only the selected section renders. The explorer nests these under each project,
        // so the page is one thing at a time instead of every panel stacked at once.
        {move || {
            let section = state.current_section.get();
            let loading = project_detail.with(Option::is_none);
            if loading {
                return view! {
                    <section class="adi-panel"><div class="adi-empty">"Loading…"</div></section>
                }
                .into_any();
            }
            match section {
                ProjectSection::Overview => view! {
                    {move || project_detail.get().map(|d|
                        detail_body(state, route, confirm_delete, service_form, d, false))}
                    {subprojects_panel(state, route, subproject_form)}
                }
                .into_any(),
                ProjectSection::Services => view! {
                    {move || project_detail.get().map(|d|
                        detail_body(state, route, confirm_delete, service_form, d, true))}
                }
                .into_any(),
                ProjectSection::Tasks => view! {
                    {tasks_panel(state, route, task_form)}
                }
                .into_any(),
                ProjectSection::Agents => view! {
                    {move || agent_live_view(state, agents_watch)}
                    {agents_panel(state, agent_form, agents_watch, agents_form, route)}
                }
                .into_any(),
                ProjectSection::Triggers => view! {
                    {move || log_view(triggers_log)}
                    {triggers_panel(state, trigger_form, triggers_log)}
                }
                .into_any(),
                ProjectSection::Tools => view! {
                    {move || tool_run_view(state, tool_run)}
                    {move || tool_editor_view(state, tool_editor)}
                    {tools_panel(state, tool_form, tool_editor, tool_run)}
                }
                .into_any(),
                ProjectSection::Secrets => view! {
                    {secrets_panel(state, secrets_form)}
                }
                .into_any(),
                ProjectSection::Workspaces => view! {
                    {move || term_view(state, term)}
                    {move || hook_log_view(hook_log)}
                    {move || hook_editor_view(state, hook_editor)}
                    {workspaces_panel(state, workspace_form, new_hook_form, hook_log,
                        hook_editor, term)}
                }
                .into_any(),
                ProjectSection::Files => view! {
                    {files_view(state)}
                }
                .into_any(),
            }
        }}

        {flash_view(flash)}
    }
    .into_any()
}

/// Render one loaded [`ProjectDetail`]. The header — name, status, archive/delete — is the
/// project's identity and shows on every section; `services` picks which body follows it,
/// since Overview and Services are the two sections drawn from this payload.
fn detail_body(
    state: State,
    route: RwSignal<Route>,
    confirm_delete: RwSignal<bool>,
    service_form: QuickServiceForm,
    d: ProjectDetail,
    services_section: bool,
) -> AnyView {
    let archived = d.is_archived();
    let id = d.id.clone();
    let name = d.name.clone();
    let created = fmt_date(d.created_at);
    let archived_note = d
        .archived_at
        .map_or_else(String::new, |ts| format!("archived {}", fmt_date(ts)));
    let status_label = if archived { "Archived" } else { "Active" };
    // The identity line that used to be a stat-tile strip: dates belong next to the name, not in
    // cards of their own.
    let meta = if archived_note.is_empty() {
        format!("created {created}")
    } else {
        format!("created {created} \u{b7} {archived_note}")
    };
    let description = d.description.clone();
    let has_hive = d.has_hive;
    let services = d.services.clone();
    let reload_id = id.clone();
    let rows_id = id.clone();

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

    let del_id = id.clone();
    let delete_ctrl = move || {
        if confirm_delete.get() {
            let yes_id = del_id.clone();
            view! {
                <span class="adi-muted">"Delete permanently?"</span>
                <button class="adi-btn adi-btn--link" style="color:var(--down)" on:click=move |_| {
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
            <span class="adi-chip adi-mono" title="directory under ~/.adi/mono/projects">{id}</span>
            {parent_link(state, route, d.parent.clone())}
            <span class="adi-spacer"></span>
            <span class="adi-updated">{meta}</span>
            {archive_btn}
            {delete_ctrl}
        </div>

        {(!services_section).then(|| view! {
            {description.map(|text| view! {
                <section class="adi-panel">
                    <div class="adi-panel__head"><h2 class="adi-panel__title">"Description"</h2></div>
                    <p class="adi-muted">{text}</p>
                </section>
            })}
        })}

        {services_section.then(|| view! {
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
            <div class="adi-hint">
                "Written to the project's " <code>".adi/hive.yaml"</code> " — the front door picks the "
                "service up from there. Edit or remove services by editing that file in the Files panel."
            </div>
        </section>
        })}
    }
    .into_any()
}

/// Every transitive sub-project of `root`, as a map from project id to its display name, read
/// from the loaded project list. The Agents and Tasks panels fold these projects' items into the
/// parent's view — so opening "NY" also surfaces the agents and tasks filed under its nested
/// projects, each marked with its owning sub-project. Empty when the list hasn't loaded yet or
/// `root` has no sub-projects. Walks the `parent` links breadth-first, guarding against a
/// malformed cycle so a bad link can't spin forever.
pub(super) fn descendant_projects(
    state: State,
    root: &str,
) -> std::collections::HashMap<String, String> {
    let mut out = std::collections::HashMap::new();
    let Some(ps) = state.projects.get() else {
        return out;
    };
    let mut stack = vec![root.to_string()];
    while let Some(cur) = stack.pop() {
        for p in &ps.projects {
            if p.parent.as_deref() != Some(cur.as_str()) || out.contains_key(&p.id) {
                continue;
            }
            let display = if p.name.trim().is_empty() {
                p.id.clone()
            } else {
                p.name.clone()
            };
            out.insert(p.id.clone(), display);
            stack.push(p.id.clone());
        }
    }
    out
}

/// A "belongs to a nested sub-project" marker chip for an item surfaced in a parent project's
/// panel. Clicking it opens the owning sub-project (`owner_id`/`owner_name`) at `section` — the
/// Agents section for an agent, Tasks for a task — so the item is one click from where it lives.
pub(super) fn sub_marker(
    state: State,
    route: RwSignal<Route>,
    owner_id: String,
    owner_name: String,
    section: ProjectSection,
) -> AnyView {
    let open_id = owner_id.clone();
    let href = section.path(&owner_id);
    let title = format!("filed under sub-project {owner_name}");
    view! {
        <a class="adi-subchip" href=href title=title
            on:click=move |ev: web_sys::MouseEvent| {
                if ev.meta_key() || ev.ctrl_key() || ev.shift_key() || ev.button() != 0 { return; }
                ev.prevent_default();
                open_project_section(state, route, open_id.clone(), section);
            }>{format!("↳ {owner_name}")}</a>
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

/// Run a detail-page mutation (archive/restore, sub-project create) that returns the fresh
/// project list, then re-fetch this project's detail so the page reflects the change; flashes
/// success or error. Toggles `busy` around the request when a form is driving it.
fn apply_detail_mutation<F>(
    state: State,
    id: String,
    busy: Option<RwSignal<bool>>,
    ok_msg: String,
    fut: F,
) where
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
