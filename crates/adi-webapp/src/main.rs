//! adi-webapp — the adi control-panel UI, a Leptos client-side-rendered app compiled to
//! wasm by Trunk. It talks to the `/api/*` backend using the DTO types from
//! [`adi_webapp_api`], so the wire format is shared with the server rather than duplicated.
//! Trunk's `dist/` output is embedded into [`adi-app`](../adi-app), which serves it at
//! `app.adi`.
//!
//! The crate is split into the shell (this file: [`App`] + navigation/polling), shared
//! infrastructure ([`state`], [`routing`], [`ui`], [`fetch`]), and one module per page under
//! [`pages`].

#![allow(non_snake_case)] // Leptos components are PascalCase by convention.

use std::collections::BTreeMap;

mod fetch;
mod pages;
mod routing;
mod state;
mod tree;
mod ui;

use adi_webapp_api::types::{
    AgentsState, DashboardsState, Health, HiveState, MeshState, PortsState, ProjectDetail,
    ProjectsState,
    TasksState, TriggersState, UsedPorts, WorkspacesState,
};
use gloo_timers::callback::Interval;
use leptos::prelude::*;
use wasm_bindgen::JsCast;
use wasm_bindgen::closure::Closure;
use wasm_bindgen_futures::spawn_local;

use pages::{
    agents_view, dashboards_view, hive_view, load_dir, mesh_view, poll_hook_log, poll_term,
    poll_trigger_log,
    poll_watch, ports_manager_view, project_detail_view, projects_view, tasks_view, triggers_view,
};
use routing::{Route, current_path, open_project, project_id_from_path, replace_state, spa_click};
use state::{
    AgentCodeEditor, AgentsForm, AgentsWatch, DashboardsForm, FilesState, Flash, Form, HookLogView,
    MeshForm, ProjectsForm, State,
    Status, TasksForm, TermWatch, TriggersForm, TriggersLogView, load,
};
use ui::{apply_saved_theme, fmt_uptime, nav_item, toggle_theme};

fn main() {
    console_error_panic_hook::set_once();
    apply_saved_theme();
    mount_to_body(App);
}

/// The application shell: sidebar navigation, a header, and the routed page body. Shared
/// data (status, ports, health) is polled here regardless of which page is showing.
#[component]
fn App() -> impl IntoView {
    let status = RwSignal::new(Status::Connecting);
    let ports = RwSignal::new(None::<PortsState>);
    let health = RwSignal::new(None::<Health>);
    let flash = RwSignal::new(None::<Flash>);
    let secs_since = RwSignal::new(0u32);
    let used = RwSignal::new(None::<UsedPorts>);
    let mesh = RwSignal::new(None::<MeshState>);
    let projects = RwSignal::new(None::<ProjectsState>);
    let project_detail = RwSignal::new(None::<ProjectDetail>);
    let tasks = RwSignal::new(None::<TasksState>);
    let agents = RwSignal::new(None::<AgentsState>);
    let triggers = RwSignal::new(None::<TriggersState>);
    let hive = RwSignal::new(None::<HiveState>);
    let dashboards = RwSignal::new(None::<DashboardsState>);
    let workspaces = RwSignal::new(None::<WorkspacesState>);
    // The id of the project whose detail page is open ("" when not on one). Drives detail
    // loads so navigating from one project to another (route stays ProjectDetail) still refreshes.
    let current_project = RwSignal::new(project_id_from_path(&current_path()).unwrap_or_default());
    let files = FilesState::new();
    let state = State {
        status,
        ports,
        health,
        flash,
        secs_since,
        used,
        mesh,
        projects,
        project_detail,
        current_project,
        tasks,
        agents,
        triggers,
        hive,
        dashboards,
        workspaces,
        files,
    };

    let projects_form = ProjectsForm {
        name: RwSignal::new(String::new()),
        description: RwSignal::new(String::new()),
        parent: RwSignal::new(String::new()),
        busy: RwSignal::new(false),
        show_archived: RwSignal::new(false),
    };

    // The explorer's own tree state, separate from the Projects page's: the two rails are
    // on screen at once and must not fight over the selection.
    let explorer = tree::TreeState::new();

    let dashboards_form = DashboardsForm {
        name: RwSignal::new(String::new()),
        description: RwSignal::new(String::new()),
        busy: RwSignal::new(false),
    };

    let tasks_form = TasksForm {
        title: RwSignal::new(String::new()),
        project: RwSignal::new(String::new()),
        parent: RwSignal::new(String::new()),
        tag: RwSignal::new(String::new()),
        details: RwSignal::new(String::new()),
        busy: RwSignal::new(false),
    };

    let agents_form = AgentsForm {
        name: RwSignal::new(String::new()),
        backend: RwSignal::new(String::new()),
        project: RwSignal::new(String::new()),
        model: RwSignal::new(String::new()),
        permission_mode: RwSignal::new(String::new()),
        temperature: RwSignal::new(String::new()),
        max_turns: RwSignal::new(String::new()),
        tags: RwSignal::new(String::new()),
        tools: RwSignal::new(String::new()),
        system_prompt: RwSignal::new(String::new()),
        starred: RwSignal::new(false),
        arguments: RwSignal::new(BTreeMap::new()),
        argument_values: RwSignal::new(BTreeMap::new()),
        editing: RwSignal::new(None::<String>),
        busy: RwSignal::new(false),
    };

    let triggers_form = TriggersForm {
        name: RwSignal::new(String::new()),
        kind: RwSignal::new(String::new()),
        project: RwSignal::new(String::new()),
        description: RwSignal::new(String::new()),
        code: RwSignal::new(String::new()),
        enabled: RwSignal::new(true),
        extra: RwSignal::new(BTreeMap::new()),
        editing: RwSignal::new(None::<String>),
        busy: RwSignal::new(false),
    };

    let triggers_log = TriggersLogView::new();
    let hook_log = HookLogView::new();
    let term_watch = TermWatch::new();
    let agents_watch = AgentsWatch::new();
    let agents_code = AgentCodeEditor::new();

    let form = Form {
        svc: RwSignal::new(String::new()),
        key: RwSignal::new(String::new()),
        reserving: RwSignal::new(false),
        reserved: RwSignal::new(String::new()),
    };

    let mesh_form = MeshForm {
        allow_port: RwSignal::new(String::new()),
        peer: RwSignal::new(String::new()),
        fwd_listen: RwSignal::new(String::new()),
        fwd_peer: RwSignal::new(String::new()),
        fwd_port: RwSignal::new(String::new()),
        busy: RwSignal::new(false),
        id_ref: NodeRef::new(),
        ticket_ref: NodeRef::new(),
    };

    let managed_only = RwSignal::new(true);

    // The active page, derived from the URL path. Unknown paths (including `/`) resolve to
    // Projects; canonicalize the address bar so a refresh lands on the same page.
    let route = RwSignal::new(Route::from_path(&current_path()));
    // Canonicalize the address bar, except on a project detail page whose path carries the id.
    if !matches!(route.get_untracked(), Route::ProjectDetail)
        && current_path() != route.get_untracked().path()
    {
        replace_state(route.get_untracked().path());
    }

    // Selecting in the explorer opens that project — the tree navigates. Guarded on `Some`,
    // so the initial (empty) selection never navigates on load.
    Effect::new(move |_| {
        if let Some(id) = explorer.selected.get() {
            open_project(state, route, id);
        }
    });
    // Follow the browser's back/forward buttons (keeping the active project id in sync).
    let on_pop = Closure::<dyn FnMut()>::new(move || {
        let path = current_path();
        current_project.set(project_id_from_path(&path).unwrap_or_default());
        route.set(Route::from_path(&path));
    });
    if let Some(w) = web_sys::window() {
        let _ = w.add_event_listener_with_callback("popstate", on_pop.as_ref().unchecked_ref());
    }
    on_pop.forget();

    // Load now, poll the backend every 4s, and tick the "updated Ns ago" label each second.
    // The same 1s tick refreshes the agents live view while one is open (it no-ops otherwise).
    spawn_local(load(state));
    Interval::new(4_000, move || spawn_local(load(state))).forget();
    Interval::new(1_000, move || {
        secs_since.update(|s| *s = s.saturating_add(1));
        poll_watch(agents_watch);
        poll_trigger_log(triggers_log);
        poll_hook_log(hook_log);
        poll_term(term_watch);
    })
    .forget();

    // Refresh immediately when a page that has page-specific data opens (the port scan on
    // Ports Manager, the mesh state on Mesh), so it isn't stale.
    Effect::new(move |_| {
        // Re-run when the open project changes too, so navigating detail A → B reloads.
        let _ = current_project.get();
        if matches!(
            route.get(),
            Route::Projects
                | Route::ProjectDetail
                | Route::Tasks
                | Route::Agents
                | Route::Triggers
                | Route::Hive
                | Route::PortsManager
                | Route::Mesh
        ) {
            spawn_local(load(state));
        }
        // Leaving the pages that show the agents live view closes it, so its 1s poll stops
        // (it also renders on a project's detail page, whose Agents panel shares the actions).
        if !matches!(route.get(), Route::Agents | Route::ProjectDetail) {
            agents_watch.close();
        }
        // Likewise, leaving the pages that show the fire-log view closes it (it also renders
        // on a project's detail page, whose Triggers panel shares the log actions).
        if !matches!(route.get(), Route::Triggers | Route::ProjectDetail) {
            triggers_log.close();
        }
        // The hook-log and workspace-terminal views only render on a project's detail page.
        // Closing the terminal view never kills the tmux session — it just stops the poll.
        if !matches!(route.get(), Route::ProjectDetail) {
            hook_log.close();
            term_watch.close();
        }
    });

    // Load the project file browser (from the root) whenever the open project changes to one
    // the browser isn't already showing. Kept separate from `load` so the 4s poll never
    // re-fetches over the editor buffer mid-edit.
    Effect::new(move |_| {
        let id = current_project.get();
        if matches!(route.get(), Route::ProjectDetail)
            && !id.is_empty()
            && files.loaded_for.get_untracked() != id
        {
            files.reset();
            files.loaded_for.set(id.clone());
            spawn_local(load_dir(state, id, String::new()));
        }
    });

    view! {
        <div class="adi-workbench">
        // The frame's lid: identity on the left, where you are on the right.
        <header class="adi-titlebar">
            <span class="adi-logo">"adi"<span class="adi-logo__dot">"."</span></span>
            // Where you are, read left to right from the brand — the natural reading order,
            // and it keeps the bar from being two islands with a void between them.
            <nav class="adi-crumbs" aria-label="Breadcrumb">
                <span class="adi-crumbs__sep">"/"</span>
                <span class="adi-crumbs__here">{move || route.get().title()}</span>
                {move || {
                    let id = state.current_project.get();
                    (matches!(route.get(), Route::ProjectDetail) && !id.is_empty()).then(|| view! {
                        <span class="adi-crumbs__sep">"/"</span>
                        <span class="adi-crumbs__here">{id}</span>
                    })
                }}
            </nav>
            <span class="adi-spacer"></span>
            <button class="adi-btn adi-btn--icon-sm" title="Toggle theme" aria-label="Toggle theme"
                on:click=move |_| toggle_theme()>"◐"</button>
        </header>

        <div class="adi-shell">
            <aside class="adi-sidebar">
                <nav class="adi-nav">
                    <a class="adi-nav__item" href=Route::Projects.path()
                        aria-current=move || if matches!(route.get(), Route::Projects | Route::ProjectDetail) { "page" } else { "false" }
                        on:click=move |ev| spa_click(&ev, route, Route::Projects)>
                        <span>"Projects"</span>
                    </a>
                    {nav_item(route, Route::Tasks, "Tasks")}
                    {nav_item(route, Route::Agents, "Agents")}
                    {nav_item(route, Route::Triggers, "Triggers")}
                    {nav_item(route, Route::Dashboards, "Dashboards")}
                    <div class="adi-nav__group">
                        <div class="adi-nav__heading">"Settings"</div>
                        {nav_item(route, Route::Hive, "Hive")}
                        {nav_item(route, Route::PortsManager, "Ports Manager")}
                        {nav_item(route, Route::Mesh, "Mesh")}
                    </div>
                </nav>
            </aside>

            // The explorer: the project hierarchy, always on screen, on every route.
            // Selecting a project opens it — the tree is how you navigate, not a widget on
            // one page.
            <aside class="adi-explorer">
                <div class="adi-explorer__head">
                    <span class="adi-explorer__title">"Projects"</span>
                    <span class="adi-explorer__count">
                        {move || projects.get().map(|p|
                            p.projects.iter().filter(|x| !x.is_archived()).count().to_string())}
                    </span>
                    <span class="adi-spacer"></span>
                    <a class="adi-btn adi-btn--icon-sm" href=Route::Projects.path()
                        title="Manage projects" aria-label="Manage projects"
                        on:click=move |ev| spa_click(&ev, route, Route::Projects)>"+"</a>
                </div>
                <div class="adi-explorer__body">
                    {move || explorer_tree(state, explorer)}
                </div>
            </aside>

            <main class="adi-main">
                <div class="adi-container">
                    {move || match route.get() {
                        // These pages render their own headings — no generic page title.
                        Route::PortsManager | Route::ProjectDetail => None,
                        other => Some(view! {
                            <header class="adi-bar">
                                <h1 class="adi-bar__title">{other.title()}</h1>
                            </header>
                        }),
                    }}

                    {move || match route.get() {
                        Route::Projects => projects_view(state, projects_form, route),
                        Route::ProjectDetail => project_detail_view(state, route, triggers_log, agents_watch, hook_log, term_watch),
                        Route::Tasks => tasks_view(state, tasks_form),
                        Route::Agents => agents_view(state, agents_form, agents_watch, agents_code),
                        Route::Triggers => triggers_view(state, triggers_form, triggers_log),
                        Route::Dashboards => dashboards_view(state, dashboards_form),
                        Route::Hive => hive_view(state, route),
                        Route::PortsManager => ports_manager_view(state, form, managed_only),
                        Route::Mesh => mesh_view(state, mesh_form),
                    }}

                </div>
            </main>
        </div>

        // The status bar, pinned to the foot of the workbench on every route.
        <footer class="adi-statusbar">
            <span class="adi-status" data-state=move || status.get().data()
                title=move || health.get().map(|h| format!("{} v{}", h.service, h.version))>
                <span class="adi-status__led"></span>
                <span>{move || status.get().label()}</span>
                // The backend's uptime, shown only once a health response has landed.
                {move || health.get().map(|h| view! {
                    <span class="adi-status__uptime">{fmt_uptime(h.uptime_secs)}</span>
                })}
            </span>
            <span class="adi-spacer"></span>
            <span>{move || route.get().title()}</span>
        </footer>
        </div>
    }
}

/// The explorer's project tree: active projects only, nested by their sub-project links.
/// Selecting one opens its page, so the tree navigates rather than just highlighting.
fn explorer_tree(state: State, explorer: tree::TreeState) -> AnyView {
    let Some(projects) = state.projects.get() else {
        return view! { <div class="adi-empty">"Loading…"</div> }.into_any();
    };
    let rows = pages::project_tree_rows(
        projects
            .projects
            .into_iter()
            .filter(|p| !p.is_archived())
            .collect(),
    );
    // Badge each project with its open task count — the one number worth carrying in the
    // rail, so the tree shows where the work is without opening anything.
    let tasks = state.tasks.get();
    let nodes: Vec<tree::TreeNode> = rows
        .iter()
        .enumerate()
        .map(|(i, (depth, p))| {
            let has_children = rows.get(i + 1).is_some_and(|(next, _)| next > depth);
            let open = tasks.as_ref().map(|t| {
                t.tasks
                    .iter()
                    .filter(|task| task.project.as_deref() == Some(p.id.as_str()))
                    .filter(|task| task.status == "open")
                    .count()
            });
            tree::TreeNode::new(p.id.clone(), *depth, p.name.clone())
                .children(has_children)
                .badge(open.filter(|n| *n > 0).map(|n| n.to_string()))
                .title(p.description.clone())
        })
        .collect();
    // Highlight the project that is actually open, so the rail agrees with the address bar
    // however you got there — a click, a bookmark, or the back button.
    let current = state.current_project.get();
    let selected = (!current.is_empty()).then_some(current);
    tree::tree_view(nodes, explorer, selected, "No projects yet.")
}
