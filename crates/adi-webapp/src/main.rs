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

use std::collections::{BTreeMap, BTreeSet};

mod fetch;
mod highlight;
mod icons;
mod pages;
mod routing;
mod state;
mod store_browser;
mod tree;
mod ui;

use adi_webapp_api::types::{
    AgentsState, DashboardsState, Health, HiveState, MeshState, MetaState, PortsState,
    ProjectDetail, ProjectsState, SecretsState, TasksState, ToolsState, TriggersState, UsedPorts,
    WorkspacesState,
};
use gloo_timers::callback::Interval;
use leptos::prelude::*;
use wasm_bindgen::JsCast;
use wasm_bindgen::closure::Closure;
use wasm_bindgen_futures::spawn_local;

use pages::{
    agents_view, dashboards_view, hive_view, load_dir, load_store_file, mesh_view, meta_view,
    poll_hook_log, poll_term, poll_trigger_log, poll_watch, ports_manager_view,
    project_detail_view, projects_view, secrets_view, store_file_view, tasks_view, tools_view,
    triggers_view,
};
use routing::{
    ProjectSection, Route, current_path, open_project_section, project_id_from_path,
    project_section_from_path, replace_state, spa_click,
};
use state::{
    AgentCodeEditor, AgentsForm, AgentsWatch, DashboardsForm, FilesState, Flash, Form, HookLogView,
    MeshForm, MetaForm, ProjectsForm, SecretsForm, State, Status, TasksForm, TermWatch, ToolEditor,
    ToolRunView, ToolsForm, TriggersForm, TriggersLogView, load,
};
use ui::{apply_saved_theme, fmt_uptime, toggle_theme};

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
    let tools = RwSignal::new(None::<ToolsState>);
    let secrets = RwSignal::new(None::<SecretsState>);
    let meta = RwSignal::new(None::<MetaState>);
    let triggers = RwSignal::new(None::<TriggersState>);
    let hive = RwSignal::new(None::<HiveState>);
    let dashboards = RwSignal::new(None::<DashboardsState>);
    let workspaces = RwSignal::new(None::<WorkspacesState>);
    // The id of the project whose detail page is open ("" when not on one). Drives detail
    // loads so navigating from one project to another (route stays ProjectDetail) still refreshes.
    let current_project = RwSignal::new(project_id_from_path(&current_path()).unwrap_or_default());
    // Which section of that project is showing; the bare project path is its overview.
    let current_section = RwSignal::new(project_section_from_path(&current_path()));
    let files = FilesState::new();
    let store = state::StoreBrowser::new();
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
        current_section,
        tasks,
        agents,
        tools,
        secrets,
        meta,
        triggers,
        hive,
        dashboards,
        workspaces,
        files,
        store,
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
        show_archived: RwSignal::new(false),
    };

    let tasks_form = TasksForm {
        title: RwSignal::new(String::new()),
        project: RwSignal::new(String::new()),
        parent: RwSignal::new(String::new()),
        tag: RwSignal::new(String::new()),
        details: RwSignal::new(String::new()),
        busy: RwSignal::new(false),
        show_done: RwSignal::new(false),
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
        bin_tools: RwSignal::new(BTreeSet::new()),
        secrets: RwSignal::new(BTreeSet::new()),
        system_prompt: RwSignal::new(String::new()),
        starred: RwSignal::new(false),
        arguments: RwSignal::new(BTreeMap::new()),
        argument_values: RwSignal::new(BTreeMap::new()),
        editing: RwSignal::new(None::<String>),
        busy: RwSignal::new(false),
    };

    // The Meta page's setup form for the default `adi-agent`. Seeded from the server's default
    // prompt by an effect below, once `/api/meta` first reports the agent isn't set up yet.
    let meta_form = MetaForm::new();

    let triggers_form = TriggersForm {
        name: RwSignal::new(String::new()),
        kind: RwSignal::new(String::new()),
        runtime: RwSignal::new(String::new()),
        preset: RwSignal::new(None::<String>),
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

    // The Tools page's create/link form, and the run + script-editor panels it shares with a
    // project's Tools panel (page-scoped so they survive re-renders and thread into both).
    let tools_form = ToolsForm::new();
    let tool_editor = ToolEditor::new();
    let tool_run = ToolRunView::new();

    // The Secrets page's create form + reveal cache, shared with a project's Secrets panel.
    let secrets_form = SecretsForm::new();

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
    // Canonicalize the address bar, except where the path carries data `path()` cannot
    // reproduce — a project's id, or a store file's path. Canonicalizing those would rewrite
    // `/files/<path>` to `/files` and lose the file before it is ever read.
    if !matches!(
        route.get_untracked(),
        Route::ProjectDetail | Route::StoreFile
    ) && current_path() != route.get_untracked().path()
    {
        replace_state(route.get_untracked().path());
    }

    // The explorer navigates: a node's id encodes its destination. Guarded on `Some`, so the
    // initial (empty) selection never navigates on load.
    Effect::new(move |_| {
        let Some(id) = explorer.selected.get() else {
            return;
        };
        match node_target(&id) {
            Some(Nav::Global(target)) => {
                state.current_project.set(String::new());
                state.files.reset();
                routing::push_state(target.path());
                route.set(target);
                routing::scroll_top();
            }
            Some(Nav::Project(project, section)) => {
                open_project_section(state, route, project, section);
            }
            // A scope header (`scope:Global`) is a container, not a destination.
            None => {}
        }
    });
    // Follow the browser's back/forward buttons (keeping the active project id in sync).
    let on_pop = Closure::<dyn FnMut()>::new(move || {
        let path = current_path();
        current_project.set(project_id_from_path(&path).unwrap_or_default());
        current_section.set(project_section_from_path(&path));
        // A /files/<path> entry carries the file, so history navigation reloads it. Only when
        // it actually changes, or Back onto the page you are on would discard your edits.
        match routing::store_path_from_path(&path) {
            Some(file)
                if state.store.open_file.get_untracked().as_deref() != Some(file.as_str()) =>
            {
                load_store_file(state, file);
            }
            _ => {}
        }
        route.set(Route::from_path(&path));
    });
    if let Some(w) = web_sys::window() {
        let _ = w.add_event_listener_with_callback("popstate", on_pop.as_ref().unchecked_ref());
    }
    on_pop.forget();

    // A deep link (or a refresh) on /files/<path> loads that file before the first paint.
    if let Some(file) = routing::store_path_from_path(&current_path()) {
        load_store_file(state, file);
    }

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
            Route::Meta
                | Route::Projects
                | Route::ProjectDetail
                | Route::Tasks
                | Route::Agents
                | Route::Tools
                | Route::Secrets
                | Route::Triggers
                | Route::Hive
                | Route::PortsManager
                | Route::Mesh
        ) {
            spawn_local(load(state));
        }
        // Leaving the pages that show the agents live view closes it, so its 1s poll stops
        // (it also renders on a project's detail page, whose Agents panel shares the actions, and
        // on the Meta page, which runs the `adi-agent` through the same live view).
        if !matches!(
            route.get(),
            Route::Agents | Route::ProjectDetail | Route::Meta
        ) {
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
        // The tool run + script-editor panels render on the Tools page and a project's Tools
        // section; leaving both drops their (stale) output/buffers.
        if !matches!(route.get(), Route::Tools | Route::ProjectDetail) {
            tool_run.close();
            tool_editor.close();
        }
        // Leaving the Secrets page (and project details) forgets any revealed values, so a
        // plaintext secret never lingers in memory after the user navigates away.
        if !matches!(route.get(), Route::Secrets | Route::ProjectDetail) {
            secrets_form.clear_revealed();
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

    // Keep the right rail standing at the directory behind the page: opening a project — or
    // moving between its sections — expands the store tree down to that project. Runs on every
    // route change, so history navigation reveals as well as clicks do.
    Effect::new(move |_| {
        let (id, section) = (current_project.get(), current_section.get());
        if matches!(route.get(), Route::ProjectDetail) {
            store_browser::reveal_project(state, &id, section);
        }
    });

    // Seed the Meta setup form the first time `/api/meta` reports the agent isn't set up yet: the
    // prompt from the server's default (so the create form opens prefilled and editable), and the
    // backend to the first option. Guarded on an empty buffer, so it never clobbers the user's edits
    // and never re-seeds after the agent exists.
    Effect::new(move |_| {
        if let Some(m) = meta.get()
            && m.agent.is_none()
            && !meta_form.editing.get_untracked()
            && meta_form.prompt.get_untracked().is_empty()
        {
            meta_form.prompt.set(m.default_prompt.clone());
            if meta_form.backend.get_untracked().is_empty()
                && let Some(first) = m.form.backends.first()
            {
                meta_form.backend.set(first.id.clone());
            }
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

            // The explorer: every scope in one tree — Global, Settings, and the project
            // hierarchy — each expanding into its own sections. This is the app's only
            // navigator; selecting a node routes to it.
            <aside class="adi-explorer">
                <div class="adi-explorer__head">
                    <span class="adi-explorer__title">"Explorer"</span>
                    <span class="adi-spacer"></span>
                    <a class="adi-btn adi-btn--icon-sm" href=Route::Projects.path()
                        title="Manage projects" aria-label="Manage projects"
                        on:click=move |ev| spa_click(&ev, route, Route::Projects)>"+"</a>
                </div>
                <div class="adi-explorer__body">
                    {move || explorer_tree(state, explorer, route)}
                </div>
            </aside>

            <main class="adi-main"
                class:adi-main--flush=move || matches!(route.get(), Route::StoreFile)>
                <div class="adi-container">
                    {move || match route.get() {
                        // These pages render their own headings — no generic page title.
                        // StoreFile is a full-bleed editor: its head carries the file path.
                        Route::PortsManager | Route::ProjectDetail | Route::StoreFile => None,
                        other => Some(view! {
                            <header class="adi-bar">
                                <h1 class="adi-bar__title">{other.title()}</h1>
                            </header>
                        }),
                    }}

                    {move || match route.get() {
                        Route::Meta => meta_view(state, route, meta_form, agents_watch),
                        Route::Projects => projects_view(state, projects_form, route),
                        Route::ProjectDetail => project_detail_view(state, route, triggers_log, agents_watch, agents_form, hook_log, term_watch, tool_editor, tool_run),
                        Route::StoreFile => store_file_view(state),
                        Route::Tasks => tasks_view(state, tasks_form),
                        Route::Agents => agents_view(state, agents_form, agents_watch, agents_code),
                        Route::Tools => tools_view(state, tools_form, tool_editor, tool_run),
                        Route::Secrets => secrets_view(state, secrets_form),
                        Route::Triggers => triggers_view(state, triggers_form, triggers_log),
                        Route::Dashboards => dashboards_view(state, dashboards_form),
                        Route::Hive => hive_view(state, route),
                        Route::PortsManager => ports_manager_view(state, form, managed_only),
                        Route::Mesh => mesh_view(state, mesh_form),
                    }}

                </div>
            </main>

            // The store browser: a file view of ~/.adi/mono beside every page, collapsed by
            // default. The left explorer navigates; this one shows what is on disk.
            {store_browser::store_rail(state, route)}
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

/// The global scopes, each with the sections that live inside it. Kept beside the project
/// scopes in one tree so "where am I working" and "what am I looking at" are the same
/// question, asked once.
const GLOBAL_SCOPES: [(&str, &[Route]); 2] = [
    (
        "Global",
        &[
            Route::Meta,
            Route::Projects,
            Route::Tasks,
            Route::Agents,
            Route::Tools,
            Route::Secrets,
            Route::Triggers,
            Route::Dashboards,
        ],
    ),
    ("Settings", &[Route::Hive, Route::PortsManager, Route::Mesh]),
];

/// The glyph for a top-level scope header.
fn scope_icon(label: &str) -> icons::Icon {
    match label {
        "Settings" => icons::Icon::Gear,
        _ => icons::Icon::Globe,
    }
}

/// A tree node's id doubles as its navigation target. Global sections are `route:<path>`;
/// a project is `proj:<id>`, and one of its sections `proj:<id>:<slug>`.
fn node_target(id: &str) -> Option<Nav> {
    if let Some(path) = id.strip_prefix("route:") {
        return Some(Nav::Global(Route::from_path(path)));
    }
    let rest = id.strip_prefix("proj:")?;
    match rest.split_once(':') {
        Some((project, slug)) => Some(Nav::Project(
            project.to_string(),
            ProjectSection::from_slug(slug),
        )),
        None => Some(Nav::Project(rest.to_string(), ProjectSection::Overview)),
    }
}

/// Where a tree selection points.
enum Nav {
    Global(Route),
    Project(String, ProjectSection),
}

/// The explorer: one tree holding every scope. Global and Settings come first, then the
/// project hierarchy — and every scope expands into its own sections, so a project is
/// browsed like a directory rather than as one page of stacked panels.
fn explorer_tree(state: State, explorer: tree::TreeState, route: RwSignal<Route>) -> AnyView {
    let mut nodes: Vec<tree::TreeNode> = Vec::new();

    for (label, routes) in GLOBAL_SCOPES {
        nodes.push(
            tree::TreeNode::new(format!("scope:{label}"), 0, label)
                .children(true)
                .container(true)
                .icon(scope_icon(label).path()),
        );
        for r in routes {
            nodes.push(
                tree::TreeNode::new(format!("route:{}", r.path()), 1, r.title())
                    .icon(icons::route_icon(*r).path()),
            );
        }
    }

    let Some(projects) = state.projects.get() else {
        return tree::tree_view(nodes, explorer, None, "Loading…");
    };
    let rows = pages::project_tree_rows(
        projects
            .projects
            .into_iter()
            .filter(|p| !p.is_archived())
            .collect(),
    );
    let tasks = state.tasks.get();
    for (i, (depth, p)) in rows.iter().enumerate() {
        // `project_tree_rows` emits a parent immediately followed by its children, so a row
        // one level deeper than the previous one is the first sub-project of that parent.
        let first_child = *depth > 0
            && rows
                .get(i.wrapping_sub(1))
                .is_some_and(|(prev, _)| *prev == depth - 1);
        // Badge each project with its open task count — the one number worth carrying in
        // the rail, so the tree shows where the work is without opening anything.
        let open = tasks.as_ref().map(|t| {
            t.tasks
                .iter()
                .filter(|task| task.project.as_deref() == Some(p.id.as_str()))
                .filter(|task| task.status == "open")
                .count()
        });
        nodes.push(
            tree::TreeNode::new(format!("proj:{}", p.id), *depth, p.name.clone())
                // Always a branch: even a project with no sub-projects holds its sections.
                .children(true)
                .icon(icons::Icon::Folder.path())
                .emphasis(true)
                .separated(first_child)
                .badge(open.filter(|n| *n > 0).map(|n| n.to_string()))
                .title(p.description.clone()),
        );
        for section in ProjectSection::ALL {
            nodes.push(
                tree::TreeNode::new(
                    format!("proj:{}:{}", p.id, section.slug()),
                    depth + 1,
                    section.title(),
                )
                .icon(icons::section_icon(section).path()),
            );
        }
    }

    // Highlight what is actually open, so the rail agrees with the address bar however you
    // got there — a click, a bookmark, or the back button.
    let selected = match state.current_project.get() {
        id if id.is_empty() => Some(format!("route:{}", route.get().path())),
        id => Some(format!(
            "proj:{}:{}",
            id,
            state.current_section.get().slug()
        )),
    };
    tree::tree_view(nodes, explorer, selected, "Nothing here yet.")
}
