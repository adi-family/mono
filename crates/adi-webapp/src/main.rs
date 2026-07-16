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
mod ui;

use adi_webapp_api::types::{
    AgentsState, Health, HiveState, MeshState, PortsState, ProjectDetail, ProjectsState,
    TasksState, UsedPorts,
};
use gloo_timers::callback::Interval;
use leptos::prelude::*;
use wasm_bindgen::JsCast;
use wasm_bindgen::closure::Closure;
use wasm_bindgen_futures::spawn_local;

use pages::{
    agents_view, hive_view, load_dir, mesh_view, overview_view, poll_watch, ports_manager_view,
    project_detail_view, projects_view, tasks_view,
};
use routing::{Route, current_path, project_id_from_path, replace_state, spa_click};
use state::{
    AgentsForm, AgentsWatch, FilesState, Flash, Form, MeshForm, ProjectsForm, State, Status,
    TasksForm, load,
};
use ui::{apply_saved_theme, nav_item, toggle_theme};

fn main() {
    console_error_panic_hook::set_once();
    apply_saved_theme();
    mount_to_body(App);
}

/// The application shell: sidebar navigation, a header, and the routed page body. Shared
/// data (status, ports, health) is polled here regardless of which page is showing.
#[component]
fn App() -> impl IntoView {
    // Reactive state the whole UI reads from. `State` bundles the signals a data refresh
    // writes; `Form` bundles the reserve form's local signals.
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
    let hive = RwSignal::new(None::<HiveState>);
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
        hive,
        files,
    };

    // The Projects page's local form: the create inputs, a busy flag, and the active/archived filter.
    let projects_form = ProjectsForm {
        id: RwSignal::new(String::new()),
        name: RwSignal::new(String::new()),
        description: RwSignal::new(String::new()),
        busy: RwSignal::new(false),
        show_archived: RwSignal::new(false),
    };

    // The Tasks page's local create form.
    let tasks_form = TasksForm {
        title: RwSignal::new(String::new()),
        project: RwSignal::new(String::new()),
        parent: RwSignal::new(String::new()),
        tag: RwSignal::new(String::new()),
        details: RwSignal::new(String::new()),
        busy: RwSignal::new(false),
    };

    // The Agents page's local create/edit form.
    let agents_form = AgentsForm {
        name: RwSignal::new(String::new()),
        backend: RwSignal::new(String::new()),
        model: RwSignal::new(String::new()),
        permission_mode: RwSignal::new(String::new()),
        temperature: RwSignal::new(String::new()),
        max_turns: RwSignal::new(String::new()),
        tags: RwSignal::new(String::new()),
        tools: RwSignal::new(String::new()),
        system_prompt: RwSignal::new(String::new()),
        starred: RwSignal::new(false),
        extra: RwSignal::new(BTreeMap::new()),
        editing: RwSignal::new(None::<String>),
        busy: RwSignal::new(false),
    };

    // The Agents page's live view (a polled read-only capture of an agent's tmux pane).
    let agents_watch = AgentsWatch::new();

    let form = Form {
        svc: RwSignal::new(String::new()),
        key: RwSignal::new(String::new()),
        reserving: RwSignal::new(false),
        reserved: RwSignal::new(String::new()),
    };

    // The Mesh page's local form + copy-field signals.
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

    // "Ports in use" filter: defaults to ADI-managed only; toggle shows all listening ports.
    let managed_only = RwSignal::new(true);

    // The active page, derived from the URL path. Unknown paths (including `/`) resolve to
    // Overview; canonicalize the address bar so a refresh lands on the same page.
    let route = RwSignal::new(Route::from_path(&current_path()));
    // Canonicalize the address bar, except on a project detail page whose path carries the id.
    if !matches!(route.get_untracked(), Route::ProjectDetail)
        && current_path() != route.get_untracked().path()
    {
        replace_state(route.get_untracked().path());
    }
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
                | Route::Hive
                | Route::PortsManager
                | Route::Mesh
        ) {
            spawn_local(load(state));
        }
        // Leaving the Agents page closes the live view, so its 1s poll stops.
        if !matches!(route.get(), Route::Agents) {
            agents_watch.close();
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
        <div class="adi-shell">
            <aside class="adi-sidebar">
                <div class="adi-sidebar__brand">
                    <span class="adi-logo">"adi"<span class="adi-logo__dot">"."</span></span>
                    <span class="adi-bar__sub">"control panel"</span>
                </div>
                <nav class="adi-nav">
                    {nav_item(route, Route::Overview, "Overview")}
                    <a class="adi-nav__item" href=Route::Projects.path()
                        aria-current=move || if matches!(route.get(), Route::Projects | Route::ProjectDetail) { "page" } else { "false" }
                        on:click=move |ev| spa_click(&ev, route, Route::Projects)>
                        <span>"Projects"</span>
                    </a>
                    {nav_item(route, Route::Tasks, "Tasks")}
                    {nav_item(route, Route::Agents, "Agents")}
                    <div class="adi-nav__group">
                        <div class="adi-nav__heading">"Settings"</div>
                        {nav_item(route, Route::Hive, "Hive")}
                        {nav_item(route, Route::PortsManager, "Ports Manager")}
                        {nav_item(route, Route::Mesh, "Mesh")}
                    </div>
                </nav>
                <span class="adi-spacer"></span>
                <div class="adi-sidebar__foot">
                    <span class="adi-status" data-state=move || status.get().data()>
                        <span class="adi-status__led"></span>
                        <span>{move || status.get().label()}</span>
                    </span>
                    <button class="adi-btn adi-btn--icon" title="Toggle theme" aria-label="Toggle theme"
                        on:click=move |_| toggle_theme()>"◐"</button>
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
                        Route::Overview => overview_view(state),
                        Route::Projects => projects_view(state, projects_form, route),
                        Route::ProjectDetail => project_detail_view(state, route),
                        Route::Tasks => tasks_view(state, tasks_form),
                        Route::Agents => agents_view(state, agents_form, agents_watch),
                        Route::Hive => hive_view(state, route),
                        Route::PortsManager => ports_manager_view(state, form, managed_only),
                        Route::Mesh => mesh_view(state, mesh_form),
                    }}

                    <footer class="adi-footer">
                        "The Rust backend serves " <code>"/api"</code> "; this page is what "
                        <code>"app.adi"</code> " shows."
                    </footer>
                </div>
            </main>
        </div>
    }
}
