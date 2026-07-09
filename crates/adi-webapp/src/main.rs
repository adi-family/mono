//! adi-webapp — the adi control-panel UI, a Leptos client-side-rendered app compiled to
//! wasm by Trunk. It talks to the `/api/*` backend using the DTO types from
//! [`adi_webapp_api`], so the wire format is shared with the server rather than duplicated.
//! Trunk's `dist/` output is embedded into [`adi-app`](../adi-app), which serves it at
//! `app.adi`.

#![allow(non_snake_case)] // Leptos components are PascalCase by convention.

use adi_webapp_api::types::{
    AgentDto, AgentsState, DirListing, Health, HiveState, LeaseRef, MeshForwardRef, MeshState,
    NewProject, NewTask, PortsState, Project, ProjectDetail, ProjectsState, SaveAgent, TaskRow,
    TasksState, UsedPorts,
};
use gloo_timers::callback::Interval;
use leptos::prelude::*;
use wasm_bindgen::JsCast;
use wasm_bindgen::closure::Closure;
use wasm_bindgen_futures::spawn_local;

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
        editing: RwSignal::new(None::<String>),
        busy: RwSignal::new(false),
    };

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
    spawn_local(load(state));
    Interval::new(4_000, move || spawn_local(load(state))).forget();
    Interval::new(1_000, move || {
        secs_since.update(|s| *s = s.saturating_add(1));
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
                        Route::Agents => agents_view(state, agents_form),
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

/// A single full-width placeholder row spanning `colspan` columns — the
/// `<tr><td class="adi-empty">…</td></tr>` every table body falls back to for its loading, empty,
/// or error state.
fn placeholder_row(colspan: &'static str, msg: &str) -> AnyView {
    view! { <tr><td class="adi-empty" colspan=colspan>{msg.to_string()}</td></tr> }.into_any()
}

// ---- shared view helpers ------------------------------------------------------------
// Small building blocks the pages compose from, so the repeated markup (stat tiles, table
// shells, the flash line, segmented filters) lives in one place instead of at every call site.

/// One stat tile in an `adi-tiles` strip: a label, a big value, and a sub-note. `value`/`note`
/// take any view, so a caller passes either a literal or a reactive `move || …` closure.
fn tile(label: &'static str, value: impl IntoView + 'static, note: impl IntoView + 'static) -> impl IntoView {
    view! {
        <div class="adi-tile">
            <div class="adi-tile__label">{label}</div>
            <div class="adi-tile__value">{value}</div>
            <div class="adi-tile__note">{note}</div>
        </div>
    }
}

/// The standard table shell: the `adi-tablewrap` scroll box, a header row built from `headers`
/// (an empty string yields a blank action column), and `body` as the `<tbody>`.
fn data_table(headers: &'static [&'static str], body: impl IntoView + 'static) -> impl IntoView {
    view! {
        <div class="adi-tablewrap">
            <table class="adi-table">
                <thead>
                    <tr>{headers.iter().map(|h| view! { <th>{*h}</th> }).collect::<Vec<_>>()}</tr>
                </thead>
                <tbody>{body}</tbody>
            </table>
        </div>
    }
}

/// The one-line status message shown under a form: reads the shared `flash` signal, colouring
/// itself via `data-kind`.
fn flash_view(flash: RwSignal<Option<Flash>>) -> impl IntoView {
    view! {
        <div class="adi-flash" data-kind=move || flash.get().map_or("none", |f| f.kind)>
            {move || flash.get().map(|f| f.msg).unwrap_or_default()}
        </div>
    }
}

/// A two-option segmented toggle bound to a `bool` signal: the left button selects `false`, the
/// right selects `true`, each reflecting the state through `aria-pressed`.
fn segmented(
    aria_label: &'static str,
    signal: RwSignal<bool>,
    left: &'static str,
    right: &'static str,
) -> impl IntoView {
    view! {
        <div class="adi-segmented" role="group" aria-label=aria_label>
            <button class="adi-segmented__option" type="button"
                aria-pressed=move || (!signal.get()).to_string()
                on:click=move |_| signal.set(false)>{left}</button>
            <button class="adi-segmented__option" type="button"
                aria-pressed=move || signal.get().to_string()
                on:click=move |_| signal.set(true)>{right}</button>
        </div>
    }
}

/// A read-only field with a Copy button (the mesh id/ticket rows): selects on focus and copies to
/// the clipboard. `node` lets the button reach the input's live text.
fn copy_row(node: NodeRef<leptos::html::Input>, value: impl Fn() -> String + Send + 'static) -> impl IntoView {
    view! {
        <div class="adi-copyrow">
            <input class="adi-input adi-input--wide adi-mono" readonly=true node_ref=node
                prop:value=value
                on:focus=move |ev| select_target(&ev) />
            <button class="adi-btn adi-btn--ghost" type="button"
                on:click=move |_| copy_field(node)>"Copy"</button>
        </div>
    }
}

/// One sidebar nav link that navigates client-side and marks itself `aria-current` when active.
/// (The Projects link stays inline — it is also current on the project-detail route.)
fn nav_item(route: RwSignal<Route>, target: Route, label: &'static str) -> impl IntoView {
    view! {
        <a class="adi-nav__item" href=target.path()
            aria-current=move || aria_current(route, target)
            on:click=move |ev| spa_click(&ev, route, target)>
            <span>{label}</span>
        </a>
    }
}

/// A labeled text input bound to a `String` signal — the `adi-field` wrapper the forms repeat.
/// Optional props toggle the mono/wide input classes, a numeric input mode, a trailing hint line,
/// and the field wrapper's flex style.
#[component]
fn TextField(
    /// The input's `id` (also the label's `for`).
    id: &'static str,
    /// The field's label text.
    label: &'static str,
    /// The bound value signal.
    value: RwSignal<String>,
    #[prop(optional)] placeholder: &'static str,
    #[prop(optional)] hint: &'static str,
    #[prop(optional)] mono: bool,
    #[prop(optional)] wide: bool,
    #[prop(optional)] numeric: bool,
    #[prop(optional)] field_style: &'static str,
) -> impl IntoView {
    let mut class = String::from("adi-input");
    if wide {
        class.push_str(" adi-input--wide");
    }
    if mono {
        class.push_str(" adi-mono");
    }
    let inputmode = if numeric { "numeric" } else { "text" };
    view! {
        <div class="adi-field" style=field_style>
            <label class="adi-field__label" for=id>{label}</label>
            <input class=class id=id placeholder=placeholder autocomplete="off" inputmode=inputmode
                prop:value=move || value.get()
                on:input=move |ev| value.set(event_target_value(&ev)) />
            {(!hint.is_empty()).then(|| view! { <span class="adi-field__hint">{hint}</span> })}
        </div>
    }
}

/// The Overview page: system liveness at a glance.
fn overview_view(state: State) -> AnyView {
    let State { health, .. } = state;
    view! {
        <section class="adi-tiles">
            {tile("Uptime",
                move || health.get().map_or_else(|| "—".to_string(), |h| fmt_uptime(h.uptime_secs)),
                move || health.get().map_or_else(|| "adi-app".to_string(),
                    |h| format!("{} v{}", h.service, h.version)))}
        </section>
    }
    .into_any()
}

/// The Projects page: the registry of project metadata manifests, with a create form and
/// per-project archive/restore controls. A project's runtime config lives in its own
/// `.adi/hive.yaml`; this page manages only the `config.toml` manifest.
fn projects_view(state: State, form: ProjectsForm, route: RwSignal<Route>) -> AnyView {
    let State {
        projects,
        flash,
        secs_since,
        ..
    } = state;
    let ProjectsForm {
        id,
        name,
        description,
        busy,
        show_archived,
    } = form;
    view! {
        <section class="adi-tiles">
            {tile("Projects",
                move || projects.get().map_or_else(|| "—".to_string(), |p| p.projects.len().to_string()),
                "registered manifests")}
            {tile("Active",
                move || projects.get().map_or_else(|| "—".to_string(),
                    |p| p.projects.iter().filter(|x| !x.is_archived()).count().to_string()),
                move || projects.get().map_or_else(|| "not archived".to_string(),
                    |p| format!("{} archived", p.projects.iter().filter(|x| x.is_archived()).count())))}
        </section>

        <section class="adi-panel">
            <div class="adi-panel__head">
                <h2 class="adi-panel__title">"Registered projects"</h2>
                <span class="adi-updated">{move || updated_text(state.ports, secs_since)}</span>
                <span class="adi-spacer"></span>
                {segmented("Filter projects", show_archived, "Active", "All")}
            </div>

            {data_table(&["Name", "ID", "Created", "Status", ""],
                move || project_rows(state, show_archived, route))}

            <form class="adi-form" on:submit=move |ev| {
                ev.prevent_default();
                let pid = id.get().trim().to_string();
                if pid.is_empty() {
                    flash.set(Some(Flash::err("A project id is required.".to_string())));
                    return;
                }
                let display = name.get().trim().to_string();
                let desc = description.get().trim().to_string();
                let body = NewProject {
                    id: pid.clone(),
                    name: (!display.is_empty()).then_some(display),
                    description: (!desc.is_empty()).then_some(desc),
                };
                id.set(String::new());
                name.set(String::new());
                description.set(String::new());
                apply_projects(state, Some(busy), format!("Registered project {pid}."),
                    fetch::create_project(body));
            }>
                <TextField id="proj-id" label="Project id" placeholder="my-app" mono=true value=id />
                <TextField id="proj-name" label="Name" placeholder="My App (defaults to the id)" value=name />
                <TextField id="proj-desc" label="Description" placeholder="optional one-liner" wide=true
                    field_style="flex:1 1 240px; min-width:0" value=description />
                <button class="adi-btn adi-btn--primary" type="submit" prop:disabled=move || busy.get()>
                    "Add project"
                </button>
            </form>
            {flash_view(flash)}
        </section>
    }
    .into_any()
}

/// Render the projects table body: a loading/empty placeholder, or one row per project
/// (filtered to active-only unless `show_archived`). The name opens the project's detail
/// page; the trailing action archives/restores it.
fn project_rows(state: State, show_archived: RwSignal<bool>, route: RwSignal<Route>) -> AnyView {
    let Some(state_projects) = state.projects.get() else {
        return placeholder_row("5", "Loading…");
    };
    let show_all = show_archived.get();
    let rows: Vec<Project> = state_projects
        .projects
        .into_iter()
        .filter(|p| show_all || !p.is_archived())
        .collect();

    if rows.is_empty() {
        let msg = if show_all {
            "No projects yet — register one below."
        } else {
            "No active projects. Add one below, or switch to All to see archived ones."
        };
        return placeholder_row("5", msg);
    }

    rows.into_iter()
        .map(|p| {
            let archived = p.is_archived();
            let id = p.id.clone();
            let action = if archived {
                let id = id.clone();
                view! {
                    <button class="adi-btn adi-btn--link" on:click=move |_| {
                        apply_projects(state, None, format!("Restored {id}."),
                            fetch::unarchive_project(id.clone()));
                    }>"Restore"</button>
                }
                .into_any()
            } else {
                let id = id.clone();
                view! {
                    <button class="adi-btn adi-btn--link" on:click=move |_| {
                        apply_projects(state, None, format!("Archived {id}."),
                            fetch::archive_project(id.clone()));
                    }>"Archive"</button>
                }
                .into_any()
            };
            let status = if archived {
                view! { <span class="adi-chip">"Archived"</span> }.into_any()
            } else {
                view! { <span class="adi-muted">"Active"</span> }.into_any()
            };
            let created = fmt_date(p.created_at);
            let title = p.description.clone().unwrap_or_default();
            let open_id = id.clone();
            let href = format!("/projects/{id}");
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
                    <td class="adi-mono">{p.id}</td>
                    <td class="adi-mono adi-muted">{created}</td>
                    <td>{status}</td>
                    <td style="text-align:right">{action}</td>
                </tr>
            }
        })
        .collect::<Vec<_>>()
        .into_any()
}

/// Run a mutation that returns fresh state `T`, hand the result to `store`, and flash success or
/// the error; toggles `busy` around the request when a form is driving it. The `apply_projects` /
/// `apply_tasks` / `apply_agents` / `apply_mesh` helpers are thin typed wrappers over this — each
/// differs only in which page-state signal receives the result.
fn apply_mutation<T, S, F>(
    state: State,
    busy: Option<RwSignal<bool>>,
    ok_msg: String,
    store: S,
    fut: F,
) where
    S: Fn(State, T) + 'static,
    F: std::future::Future<Output = Result<T, String>> + 'static,
{
    if let Some(b) = busy {
        b.set(true);
    }
    spawn_local(async move {
        match fut.await {
            Ok(v) => {
                store(state, v);
                state.flash.set(Some(Flash::ok(ok_msg)));
            }
            Err(e) => state.flash.set(Some(Flash::err(e))),
        }
        if let Some(b) = busy {
            b.set(false);
        }
    });
}

/// Run a projects mutation: set the returned state and a success flash, or an error flash;
/// toggles `busy` around the request when a form is driving it.
fn apply_projects<F>(state: State, busy: Option<RwSignal<bool>>, ok_msg: String, fut: F)
where
    F: std::future::Future<Output = Result<ProjectsState, String>> + 'static,
{
    apply_mutation(state, busy, ok_msg, |s, p| s.projects.set(Some(p)), fut);
}

/// The Tasks page: a read-only view of the task tree (`~/.adi/mono/mcp/tasks.json`), shared with
/// the `adi-task` CLI and the `tasks_*` MCP tools. Stat tiles plus a nested table; mutations stay
/// in the CLI/MCP surface.
fn tasks_view(state: State, form: TasksForm) -> AnyView {
    let tasks = state.tasks;
    let secs_since = state.secs_since;
    let flash = state.flash;
    let TasksForm {
        title,
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

            {data_table(&["Task", "ID", "Tag", "Status", "Subtasks"], move || task_rows(tasks))}
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
                let body = NewTask {
                    title: t.clone(),
                    details: (!det.is_empty()).then_some(det),
                    project: None,
                    tag: (!tg.is_empty()).then_some(tg),
                    parent: (!par.is_empty()).then_some(par),
                };
                title.set(String::new());
                details.set(String::new());
                parent.set(String::new());
                tag.set(String::new());
                apply_tasks(state, Some(busy), format!("Created task “{t}”."),
                    fetch::create_task(body));
            }>
                <TextField id="task-title" label="Title" placeholder="What needs doing?" wide=true
                    field_style="flex:1 1 220px; min-width:0" value=title />
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
                "Completing, archiving, editing, and deleting stay in the " <code>"adi-task"</code>
                " CLI and the " <code>"tasks_*"</code> " MCP tools."
            </div>
        </section>
    }
    .into_any()
}

/// Count tasks whose computed effective status equals `effective` (`ready`/`blocked`/`done`/`archived`).
fn task_count(state: &TasksState, effective: &str) -> usize {
    state.tasks.iter().filter(|t| t.effective == effective).count()
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
        return placeholder_row("5", "Loading…");
    };
    if state_tasks.tasks.is_empty() {
        return placeholder_row(
            "5",
            "No tasks yet — add one below, or use the adi-task CLI or the tasks_create MCP tool.",
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

/// Flatten the flat task list into depth-annotated tree order: each task is immediately followed
/// by its subtree (children in their incoming order). A task whose `parent` isn't in the set is
/// treated as a root, so no task is ever dropped.
fn task_tree_rows(rows: Vec<TaskRow>) -> Vec<(usize, TaskRow)> {
    use std::collections::{HashMap, HashSet};

    let ids: HashSet<String> = rows.iter().map(|r| r.id.clone()).collect();
    let mut children: HashMap<String, Vec<TaskRow>> = HashMap::new();
    let mut roots: Vec<TaskRow> = Vec::new();
    for r in rows {
        match &r.parent {
            Some(p) if ids.contains(p) => children.entry(p.clone()).or_default().push(r),
            _ => roots.push(r),
        }
    }

    fn walk(
        node: TaskRow,
        depth: usize,
        children: &mut HashMap<String, Vec<TaskRow>>,
        out: &mut Vec<(usize, TaskRow)>,
    ) {
        let id = node.id.clone();
        out.push((depth, node));
        if let Some(kids) = children.remove(&id) {
            for kid in kids {
                walk(kid, depth + 1, children, out);
            }
        }
    }

    let mut out = Vec::new();
    for root in roots {
        walk(root, 0, &mut children, &mut out);
    }
    out
}

/// The capitalized display label for a computed effective status.
fn effective_label_title(effective: &str) -> &'static str {
    match effective {
        "ready" => "Ready",
        "blocked" => "Blocked",
        "done" => "Done",
        "archived" => "Archived",
        _ => "—",
    }
}

/// The Agents page: create, edit, and delete agent definitions (docs/adi-agents.md §5) — pick a
/// backend, a system prompt, a tool scope, and backend-specific params. No run/orchestration here;
/// this only edits the stored spec. The form adapts its params to the chosen backend kind.
fn agents_view(state: State, form: AgentsForm) -> AnyView {
    let agents = state.agents;
    let secs_since = state.secs_since;
    let flash = state.flash;
    let AgentsForm {
        name,
        backend,
        model,
        permission_mode,
        temperature,
        max_turns,
        tags,
        tools,
        system_prompt,
        starred,
        editing,
        busy,
    } = form;
    view! {
        <section class="adi-tiles">
            {tile("Agents",
                move || agents.get().map_or_else(|| "—".to_string(), |a| a.agents.len().to_string()),
                "defined")}
            {tile("CLI",
                move || agents.get().map_or_else(|| "—".to_string(), |a| agent_count_kind(&a, "cli").to_string()),
                "shell a vendor CLI")}
            {tile("API",
                move || agents.get().map_or_else(|| "—".to_string(), |a| agent_count_kind(&a, "api").to_string()),
                "in-loop provider API")}
            {tile("Starred",
                move || agents.get().map_or_else(|| "—".to_string(), |a| agent_starred(&a).to_string()),
                "pinned")}
        </section>

        <section class="adi-panel">
            <div class="adi-panel__head">
                <h2 class="adi-panel__title">"Agent definitions"</h2>
                <span class="adi-updated">{move || updated_text(state.ports, secs_since)}</span>
            </div>

            {data_table(&["Name", "Backend", "Model", "Tags", ""], move || agent_rows(state, form))}

            <div class="adi-panel__head" style="border-top:1px solid var(--border)">
                <h2 class="adi-panel__title">
                    {move || match editing.get() {
                        Some(n) => format!("Editing “{n}”"),
                        None => "New agent".to_string(),
                    }}
                </h2>
                <span class="adi-spacer"></span>
                <button class="adi-btn adi-btn--link" type="button"
                    on:click=move |_| clear_agent_form(form)>"New agent"</button>
            </div>

            <form class="adi-form" on:submit=move |ev| {
                ev.prevent_default();
                let nm = name.get().trim().to_string();
                if nm.is_empty() {
                    flash.set(Some(Flash::err("An agent name is required.".to_string())));
                    return;
                }
                let be = backend.get();
                if be.trim().is_empty() {
                    flash.set(Some(Flash::err("Pick a backend.".to_string())));
                    return;
                }
                let kind = agent_backend_kind(&be);
                let body = SaveAgent {
                    name: nm.clone(),
                    backend: be.clone(),
                    system_prompt: system_prompt.get(),
                    tools: tools.get().trim().to_string(),
                    model: opt_str(model.get()),
                    permission_mode: if kind == "cli" { opt_str(permission_mode.get()) } else { None },
                    temperature: if kind == "api" { temperature.get().trim().parse::<f64>().ok() } else { None },
                    max_turns: max_turns.get().trim().parse::<u32>().ok(),
                    tags: tags.get().split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect(),
                    starred: starred.get(),
                };
                editing.set(Some(nm.clone()));
                apply_agents(state, Some(busy), format!("Saved agent “{nm}”."), fetch::save_agent(body));
            }>
                <TextField id="agent-name" label="Name" placeholder="athz-solver" mono=true
                    hint="a task tagged this name auto-starts it" value=name />
                <div class="adi-field">
                    <label class="adi-field__label" for="agent-backend">"Backend"</label>
                    <select class="adi-input" id="agent-backend"
                        prop:value=move || backend.get()
                        on:change=move |ev| backend.set(event_target_value(&ev))>
                        <option value="">"— pick a backend —"</option>
                        <option value="cli:claude">"Claude (CLI)"</option>
                        <option value="cli:codex">"Codex (CLI)"</option>
                        <option value="api:anthropic">"Anthropic (API)"</option>
                        <option value="api:openai">"OpenAI (API)"</option>
                        <option value="api:gemini">"Gemini (API)"</option>
                        <option value="api:ollama">"Ollama (local)"</option>
                    </select>
                </div>
                <div class="adi-field">
                    <label class="adi-field__label" for="agent-model">"Model"</label>
                    <input class="adi-input adi-mono" id="agent-model" autocomplete="off"
                        placeholder=move || backend_model_placeholder(&backend.get())
                        prop:value=move || model.get()
                        on:input=move |ev| model.set(event_target_value(&ev)) />
                </div>
                {move || match agent_backend_kind(&backend.get()) {
                    "cli" => Some(view! {
                        <div class="adi-field">
                            <label class="adi-field__label" for="agent-perm">"Permission mode"</label>
                            <select class="adi-input" id="agent-perm"
                                prop:value=move || permission_mode.get()
                                on:change=move |ev| permission_mode.set(event_target_value(&ev))>
                                <option value="">"— default —"</option>
                                <option value="default">"default"</option>
                                <option value="acceptEdits">"acceptEdits"</option>
                                <option value="plan">"plan"</option>
                                <option value="bypassPermissions">"bypassPermissions"</option>
                            </select>
                        </div>
                    }.into_any()),
                    "api" => Some(view! {
                        <TextField id="agent-temp" label="Temperature" placeholder="0.0 – 2.0" value=temperature />
                    }.into_any()),
                    _ => None,
                }}
                <TextField id="agent-turns" label="Max turns" placeholder="optional" value=max_turns />
                <label class="adi-field" style="flex-direction:row; align-items:center; gap:7px; align-self:center">
                    <input type="checkbox" prop:checked=move || starred.get()
                        on:change=move |ev| starred.set(event_target_checked(&ev)) />
                    <span class="adi-field__label" style="margin:0">"Starred"</span>
                </label>
                <TextField id="agent-tags" label="Tags" placeholder="comma-separated (dispatch / filtering)"
                    wide=true field_style="flex:1 1 100%; min-width:0" value=tags />
                <TextField id="agent-tools" label="Tool scope" placeholder="adi-mcp features, e.g. tasks,files[read]"
                    wide=true mono=true hint="which adi-mcp tools this agent may use"
                    field_style="flex:1 1 100%; min-width:0" value=tools />
                <div class="adi-field" style="flex:1 1 100%; min-width:0">
                    <label class="adi-field__label" for="agent-prompt">"System prompt"</label>
                    <textarea class="adi-textarea" id="agent-prompt" placeholder="The system prompt that seeds this agent…"
                        prop:value=move || system_prompt.get()
                        on:input=move |ev| system_prompt.set(event_target_value(&ev))></textarea>
                </div>
                <button class="adi-btn adi-btn--primary" type="submit" prop:disabled=move || busy.get()>
                    {move || if editing.get().is_some() { "Update agent" } else { "Create agent" }}
                </button>
            </form>
            {flash_view(flash)}
            <div class="adi-muted" style="padding:0 18px 14px; font-size:12.5px">
                "Definitions only — spawning/running agents (backends, sessions, auto-start) is future
                 work per " <code>"docs/adi-agents.md"</code> "."
            </div>
        </section>
    }
    .into_any()
}

/// Count agents whose backend kind (`cli`/`api`) matches.
fn agent_count_kind(st: &AgentsState, kind: &str) -> usize {
    st.agents.iter().filter(|a| a.backend_kind == kind).count()
}

/// Count starred agents.
fn agent_starred(st: &AgentsState) -> usize {
    st.agents.iter().filter(|a| a.starred).count()
}

/// Render the agents table body: a loading/empty placeholder, or one row per agent with Edit
/// (loads it into the form) and Delete actions.
fn agent_rows(state: State, form: AgentsForm) -> AnyView {
    let Some(st) = state.agents.get() else {
        return placeholder_row("5", "Loading…");
    };
    if st.agents.is_empty() {
        return placeholder_row("5", "No agents yet — define one below.");
    }
    st.agents
        .into_iter()
        .map(|a| {
            let name_disp = if a.starred { format!("★ {}", a.name) } else { a.name.clone() };
            let backend = a.backend.clone();
            let model = a.model.clone().unwrap_or_default();
            let tags = a.tags.join(", ");
            let del_name = a.name.clone();
            let a_edit = a.clone();
            view! {
                <tr>
                    <td>{name_disp}</td>
                    <td class="adi-mono">{backend}</td>
                    <td class="adi-mono adi-muted">{model}</td>
                    <td class="adi-muted">{tags}</td>
                    <td style="text-align:right; white-space:nowrap">
                        <button class="adi-btn adi-btn--link"
                            on:click=move |_| load_agent_into_form(form, &a_edit)>"Edit"</button>
                        " "
                        <button class="adi-btn adi-btn--link" on:click=move |_| {
                            apply_agents(state, None, format!("Deleted {del_name}."),
                                fetch::delete_agent(del_name.clone()));
                        }>"Delete"</button>
                    </td>
                </tr>
            }
        })
        .collect::<Vec<_>>()
        .into_any()
}

/// Run an agents mutation: set the returned list and a success flash, or an error flash; toggles
/// `busy` around the request when a form is driving it.
fn apply_agents<F>(state: State, busy: Option<RwSignal<bool>>, ok_msg: String, fut: F)
where
    F: std::future::Future<Output = Result<AgentsState, String>> + 'static,
{
    apply_mutation(state, busy, ok_msg, |s, a| s.agents.set(Some(a)), fut);
}

/// Load an existing agent into the create/edit form (the Edit action).
fn load_agent_into_form(form: AgentsForm, a: &AgentDto) {
    form.name.set(a.name.clone());
    form.backend.set(a.backend.clone());
    form.model.set(a.model.clone().unwrap_or_default());
    form.permission_mode.set(a.permission_mode.clone().unwrap_or_default());
    form.temperature.set(a.temperature.map(|t| t.to_string()).unwrap_or_default());
    form.max_turns.set(a.max_turns.map(|n| n.to_string()).unwrap_or_default());
    form.tags.set(a.tags.join(", "));
    form.tools.set(a.tools.clone());
    form.system_prompt.set(a.system_prompt.clone());
    form.starred.set(a.starred);
    form.editing.set(Some(a.name.clone()));
    scroll_top();
}

/// Reset the create/edit form back to a blank "New agent" state.
fn clear_agent_form(form: AgentsForm) {
    form.name.set(String::new());
    form.backend.set(String::new());
    form.model.set(String::new());
    form.permission_mode.set(String::new());
    form.temperature.set(String::new());
    form.max_turns.set(String::new());
    form.tags.set(String::new());
    form.tools.set(String::new());
    form.system_prompt.set(String::new());
    form.starred.set(false);
    form.editing.set(None);
}

/// The backend kind (`cli`/`api`) — the part before the `:` in a backend id; `""` if none.
fn agent_backend_kind(backend: &str) -> &str {
    match backend.split_once(':') {
        Some((kind, _)) => kind,
        None => "",
    }
}

/// A per-backend placeholder for the model field, hinting the expected alias.
fn backend_model_placeholder(backend: &str) -> &'static str {
    match backend {
        "cli:claude" => "opus / sonnet / fable / haiku",
        "cli:codex" => "gpt-5-codex",
        "api:anthropic" => "claude-opus-4-8",
        "api:openai" => "gpt-5-codex / o3",
        "api:gemini" => "gemini-2.5-pro / gemini-2.5-flash",
        "api:ollama" => "llama3.1 / qwen2.5-coder",
        _ => "model alias",
    }
}

/// Trim a form string into an optional, dropping it when blank.
fn opt_str(value: String) -> Option<String> {
    let trimmed = value.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

/// The project detail page (`/projects/<id>`): the manifest, its actions, and the services
/// read from the project's `.adi/hive.yaml` — what's "inside" the project.
fn project_detail_view(state: State, route: RwSignal<Route>) -> AnyView {
    let State {
        project_detail,
        flash,
        ..
    } = state;
    // Two-step delete confirmation, so a hard delete needs a deliberate second click (no
    // native confirm dialog, which would need an extra web-sys feature).
    let confirm_delete = RwSignal::new(false);
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
            Some(d) => detail_body(state, route, confirm_delete, d),
        }}

        {files_view(state)}

        {flash_view(flash)}
    }
    .into_any()
}

/// Render one loaded [`ProjectDetail`]: header + actions, key facts, description, and the
/// services table. Rebuilt whenever the `project_detail` signal changes.
fn detail_body(
    state: State,
    route: RwSignal<Route>,
    confirm_delete: RwSignal<bool>,
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
                apply_detail_mutation(state, toggle_id.clone(),
                    format!("Restored {}.", toggle_id), fetch::unarchive_project(toggle_id.clone()));
            }>"Restore"</button>
        }.into_any()
    } else {
        view! {
            <button class="adi-btn" on:click=move |_| {
                apply_detail_mutation(state, toggle_id.clone(),
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
        </section>
    }
    .into_any()
}

/// Rows for the services table: a message when there's no hive / no services, else one row per
/// service (host, ports as `key:port`, run command, restart policy, and a Start action for
/// services that declare a runner).
fn service_rows(
    state: State,
    project: String,
    services: Vec<adi_webapp_api::types::ProjectService>,
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

/// Run a detail-page mutation (archive/restore) that returns the fresh project list, then
/// re-fetch this project's detail so the page reflects the change; flashes success or error.
fn apply_detail_mutation<F>(state: State, id: String, ok_msg: String, fut: F)
where
    F: std::future::Future<Output = Result<ProjectsState, String>> + 'static,
{
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
    });
}

// ---- project files (browse + edit the files under a project's own directory) --------

/// Load the listing for directory `path` (relative to the project root) into the browser. On
/// success the current `dir` follows the server's normalized path; on failure it flashes.
async fn load_dir(state: State, id: String, path: String) {
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

/// The Hive settings page: every service declared across all projects' `.adi/hive.yaml` plus
/// the global front-door hive, each with a live running/stopped indicator.
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
                state.flash.set(Some(Flash::ok("Reloaded project config.".to_string())));
            }
            Err(e) => state
                .flash
                .set(Some(Flash::err(format!("Couldn't reload project config: {e}")))),
        }
    });
}

/// Re-fetch `/api/hive` — which re-reads every project's `.adi/hive.yaml` and the global hive
/// from disk (re-running any `bash`…`` port commands) — and refresh the Services view.
fn reload_hive(state: State) {
    spawn_local(async move {
        match fetch::hive().await {
            Ok(h) => {
                state.hive.set(Some(h));
                state.flash.set(Some(Flash::ok("Reloaded hive config.".to_string())));
            }
            Err(e) => state
                .flash
                .set(Some(Flash::err(format!("Couldn't reload hive config: {e}")))),
        }
    });
}

fn hive_view(state: State, route: RwSignal<Route>) -> AnyView {
    let State { hive, .. } = state;
    view! {
        <section class="adi-tiles">
            {tile("Services",
                move || hive.get().map_or_else(|| "—".to_string(), |h| h.services.len().to_string()),
                "across all projects + front-door")}
            {tile("Running",
                move || hive.get().map_or_else(|| "—".to_string(),
                    |h| h.services.iter().filter(|s| s.running).count().to_string()),
                move || hive.get().map_or_else(|| "primary port listening".to_string(),
                    |h| format!("{} stopped", h.services.iter().filter(|s| !s.running).count())))}
            {tile("Projects",
                move || hive.get().map_or_else(|| "—".to_string(), |h| {
                    let mut ids: Vec<&String> = h.services.iter().filter_map(|s| s.project.as_ref()).collect();
                    ids.sort_unstable();
                    ids.dedup();
                    ids.len().to_string()
                }),
                "contributing services (+ front-door)")}
        </section>

        <section class="adi-panel">
            <div class="adi-panel__head">
                <h2 class="adi-panel__title">"Hive services"</h2>
                <span class="adi-spacer"></span>
                <button class="adi-btn adi-btn--ghost" type="button"
                    title="Re-read every project's .adi/hive.yaml and the global hive from disk"
                    on:click=move |_| reload_hive(state)>"Reload config"</button>
                <span class="adi-updated">
                    {move || hive.get().map_or(String::new(), |h| format!("{} services", h.services.len()))}
                </span>
            </div>
            {data_table(&["Source", "Service", "Host", "Ports", "Command", "Restart", "Status"],
                move || hive_rows(state, route))}
            <footer class="adi-footer">
                "Read from each project's " <code>".adi/hive.yaml"</code> " and the global "
                <code>"~/.adi/mono/hive/hive.yaml"</code> ". Status = the service's primary port is listening."
            </footer>
        </section>
    }
    .into_any()
}

/// Rows for the aggregated hive table: global (front-door) services first, then per project;
/// the source cell links into the owning project's detail page.
fn hive_rows(state: State, route: RwSignal<Route>) -> AnyView {
    let Some(h) = state.hive.get() else {
        return placeholder_row("7", "Loading…");
    };
    if h.services.is_empty() {
        return placeholder_row(
            "7",
            "No hive services declared in any project or the global hive.",
        );
    }
    let mut services = h.services;
    // Global (project == None) sorts first (None < Some), then by project id, then service name.
    services.sort_by(|a, b| a.project.cmp(&b.project).then_with(|| a.name.cmp(&b.name)));
    services
        .into_iter()
        .map(|s| {
            let source = match &s.project {
                None => view! { <span class="adi-chip">"front-door"</span> }.into_any(),
                Some(id) => {
                    let open_id = id.clone();
                    let href = format!("/projects/{id}");
                    view! {
                        <a class="adi-btn adi-btn--link adi-mono" href=href
                            on:click=move |ev: web_sys::MouseEvent| {
                                if ev.meta_key() || ev.ctrl_key() || ev.shift_key() || ev.button() != 0 { return; }
                                ev.prevent_default();
                                open_project(state, route, open_id.clone());
                            }>{id.clone()}</a>
                    }.into_any()
                }
            };
            let host = dash(s.host);
            let ports = fmt_ports(&s.ports);
            let run = dash(s.run);
            let restart = dash(s.restart);
            let (state_attr, label) = if s.running { ("online", "Running") } else { ("down", "Stopped") };
            view! {
                <tr>
                    <td>{source}</td>
                    <td class="adi-mono">{s.name}</td>
                    <td class="adi-mono">{host}</td>
                    <td class="adi-mono adi-table__port">{ports}</td>
                    <td class="adi-mono adi-muted">{run}</td>
                    <td class="adi-muted">{restart}</td>
                    <td>
                        <span class="adi-status" data-state=state_attr>
                            <span class="adi-status__led"></span><span>{label}</span>
                        </span>
                    </td>
                </tr>
            }
        })
        .collect::<Vec<_>>()
        .into_any()
}

/// Format a service's declared port bindings as `key:port, key:port`, or `—` when it declares none.
fn fmt_ports(ports: &[adi_webapp_api::types::ServicePort]) -> String {
    if ports.is_empty() {
        "—".to_string()
    } else {
        ports
            .iter()
            .map(|p| format!("{}:{}", p.key, p.port))
            .collect::<Vec<_>>()
            .join(", ")
    }
}

/// An optional string for a table cell, falling back to an em dash when it's absent.
fn dash(value: Option<String>) -> String {
    value.unwrap_or_else(|| "—".to_string())
}

/// Format a Unix timestamp (seconds) as a `YYYY-MM-DD` UTC date; `0` renders as `—`. Pure
/// integer arithmetic (Howard Hinnant's `civil_from_days`), so no date crate is pulled into wasm.
fn fmt_date(secs: u64) -> String {
    if secs == 0 {
        return "—".to_string();
    }
    let days = (secs / 86_400) as i64;
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if month <= 2 { year + 1 } else { year };
    format!("{year:04}-{month:02}-{day:02}")
}

/// The Ports Manager page: the live registry table plus the reserve/release controls.
fn ports_manager_view(state: State, form: Form, managed_only: RwSignal<bool>) -> AnyView {
    let State {
        ports,
        flash,
        secs_since,
        used,
        ..
    } = state;
    let Form {
        svc,
        key,
        reserving,
        reserved,
    } = form;
    view! {
        <section class="adi-tiles">
            {tile("Active leases",
                move || ports.get().map_or_else(|| "—".to_string(), |p| p.leases.len().to_string()),
                "reserved static ports")}
            {tile("Allocatable range",
                move || ports.get().map_or_else(|| "—".to_string(),
                    |p| format!("{}–{}", p.range.start, p.range.end)),
                move || ports.get().map_or_else(|| "ports handed out from here".to_string(), |p| {
                    let span = u32::from(p.range.end) - u32::from(p.range.start) + 1;
                    format!("{span} ports · {} reserved bands", p.reserved.len())
                }))}
        </section>

        <section class="adi-panel">
            <div class="adi-panel__head">
                <h2 class="adi-panel__title">"Port registry"</h2>
                <span class="adi-spacer"></span>
                <span class="adi-updated">{move || updated_text(ports, secs_since)}</span>
            </div>

            {data_table(&["Service", "Key", "Port", ""], move || rows_view(state))}

            <form class="adi-form" on:submit=move |ev| {
                ev.prevent_default();
                let service = svc.get().trim().to_string();
                let k = key.get().trim().to_string();
                if service.is_empty() || k.is_empty() {
                    return;
                }
                reserving.set(true);
                spawn_local(async move {
                    match fetch::reserve(&LeaseRef { service: service.clone(), key: k.clone() }).await {
                        Ok(r) => {
                            reserved.set(format!("{}/{} → :{}", r.service, r.key, r.port));
                            flash.set(Some(Flash::ok(
                                format!("Reserved port {} for {}/{}.", r.port, r.service, r.key),
                            )));
                            load(state).await;
                        }
                        Err(e) => flash.set(Some(Flash::err(e))),
                    }
                    reserving.set(false);
                });
            }>
                <TextField id="svc" label="Service" placeholder="frontend" value=svc />
                <TextField id="key" label="Port key" placeholder="http" value=key />
                <button class="adi-btn adi-btn--primary" type="submit"
                    prop:disabled=move || reserving.get()>
                    "Reserve port"
                </button>
                <span class="adi-spacer" style="flex:1"></span>
                <span class="adi-chip adi-mono">{move || reserved.get()}</span>
            </form>
            {flash_view(flash)}
        </section>

        <section class="adi-panel">
            <div class="adi-panel__head">
                <h2 class="adi-panel__title">"Ports in use"</h2>
                <span class="adi-updated">
                    {move || used.get().map_or(String::new(), |u| format!("{} listening", u.ports.len()))}
                </span>
                <span class="adi-spacer"></span>
                {segmented("Filter ports", managed_only, "All", "ADI managed")}
            </div>
            {data_table(&["Port", "Process", "PID", "Owner"], move || used_rows_view(state, managed_only))}
        </section>
    }
    .into_any()
}

/// The Mesh page: this machine's id/ticket to share, the ports it exposes to peers, the
/// peers authorized to reach them, and the local→peer forwards.
fn mesh_view(state: State, form: MeshForm) -> AnyView {
    let mesh = state.mesh;
    view! {
        <section class="adi-tiles">
            {tile("Daemon",
                move || mesh.get().map_or_else(|| "—".to_string(),
                    |m| if m.running { "running".to_string() } else { "stopped".to_string() }),
                "runs adi-mesh; publishes a ticket while up")}
            {tile("Ports exposed",
                move || mesh.get().map_or_else(|| "—".to_string(), |m| m.allow.len().to_string()),
                "reachable by peers")}
            {tile("Forwards",
                move || mesh.get().map_or_else(|| "—".to_string(), |m| m.forwards.len().to_string()),
                "local → peer tunnels")}
        </section>

        {move || state.flash.get().map(|f| view! {
            <div class="adi-flash adi-flash--card" data-kind=f.kind>{f.msg}</div>
        })}

        <section class="adi-panel">
            <div class="adi-panel__head">
                <h2 class="adi-panel__title">"This machine"</h2>
                <span class="adi-spacer"></span>
                <span class="adi-status" data-state=move || mesh_state_data(mesh)>
                    <span class="adi-status__led"></span>
                    <span>{move || mesh.get().map_or_else(|| "…".to_string(),
                        |m| if m.running { "daemon up".to_string() } else { "daemon down".to_string() })}</span>
                </span>
                {move || {
                    let running = mesh.get().is_some_and(|m| m.running);
                    let busy = form.busy.get();
                    if running {
                        view! {
                            <button class="adi-btn adi-btn--ghost" type="button" prop:disabled=busy
                                on:click=move |_| apply_mesh(state, Some(form.busy),
                                    "Stopped the mesh daemon.".to_string(), fetch::mesh_stop())>
                                "Stop mesh"
                            </button>
                        }.into_any()
                    } else {
                        view! {
                            <button class="adi-btn adi-btn--primary" type="button" prop:disabled=busy
                                on:click=move |_| apply_mesh(state, Some(form.busy),
                                    "Started the mesh daemon.".to_string(), fetch::mesh_start())>
                                "Start mesh"
                            </button>
                        }.into_any()
                    }
                }}
            </div>
            <div class="adi-panel__body">
                <div class="adi-field">
                    <label class="adi-field__label">"Endpoint ID"</label>
                    {copy_row(form.id_ref, move || mesh.get().map(|m| m.id).unwrap_or_default())}
                    <div class="adi-field__hint">"The minimal token a peer can dial (resolved via discovery)."</div>
                </div>
                <div class="adi-field">
                    <label class="adi-field__label">"Ticket"</label>
                    {move || match mesh.get().and_then(|m| m.ticket) {
                        Some(ticket) => copy_row(form.ticket_ref, move || ticket.clone()).into_any(),
                        None => view! {
                            <div class="adi-field__hint adi-muted">
                                "Start the mesh daemon (the "<strong>"Start mesh"</strong>" button above) to publish a ticket a peer can dial without discovery."
                            </div>
                        }.into_any(),
                    }}
                    <div class="adi-field__hint">"id + relay + direct addresses — the reliable token to hand a peer."</div>
                </div>
            </div>
        </section>

        <section class="adi-panel">
            <div class="adi-panel__head">
                <h2 class="adi-panel__title">"Ports exposed to peers"</h2>
            </div>
            {data_table(&["Port", ""], move || mesh_allow_rows(state))}
            <form class="adi-form" on:submit=move |ev| {
                ev.prevent_default();
                if let Some(port) = parse_port(&form.allow_port.get()) {
                    form.allow_port.set(String::new());
                    apply_mesh(state, Some(form.busy), format!("Exposed port {port} to peers."),
                        fetch::mesh_allow(port));
                }
            }>
                <TextField id="mesh-allow-port" label="Local port" placeholder="3000" numeric=true
                    value=form.allow_port />
                <button class="adi-btn adi-btn--primary" type="submit" prop:disabled=move || form.busy.get()>
                    "Expose port"
                </button>
            </form>
        </section>

        <section class="adi-panel">
            <div class="adi-panel__head">
                <h2 class="adi-panel__title">"Authorized peers"</h2>
                <span class="adi-spacer"></span>
                <span class="adi-updated">{move || mesh.get().map_or_else(String::new,
                    |m| if m.authorized_peers.is_empty() { "any peer allowed".to_string() }
                        else { format!("{} allowed", m.authorized_peers.len()) })}</span>
            </div>
            {data_table(&["Endpoint ID", ""], move || mesh_peer_rows(state))}
            <form class="adi-form" on:submit=move |ev| {
                ev.prevent_default();
                let peer = form.peer.get().trim().to_string();
                if !peer.is_empty() {
                    form.peer.set(String::new());
                    apply_mesh(state, Some(form.busy), "Authorized the peer.".to_string(),
                        fetch::mesh_allow_peer(peer));
                }
            }>
                <TextField id="mesh-peer" label="Peer id or ticket" placeholder="an EndpointId or adimesh: ticket"
                    wide=true mono=true field_style="flex:1 1 240px; min-width:0" value=form.peer />
                <button class="adi-btn adi-btn--primary" type="submit" prop:disabled=move || form.busy.get()>
                    "Authorize peer"
                </button>
            </form>
        </section>

        <section class="adi-panel">
            <div class="adi-panel__head">
                <h2 class="adi-panel__title">"Forwards"</h2>
                <span class="adi-spacer"></span>
                <span class="adi-updated">"local 127.0.0.1:port → a peer's port"</span>
            </div>
            {data_table(&["Name", "Local", "Peer", "Remote", ""], move || mesh_forward_rows(state))}
            <form class="adi-form" on:submit=move |ev| {
                ev.prevent_default();
                let peer = form.fwd_peer.get().trim().to_string();
                match (parse_port(&form.fwd_listen.get()), parse_port(&form.fwd_port.get())) {
                    (Some(listen), Some(port)) if !peer.is_empty() => {
                        form.fwd_listen.set(String::new());
                        form.fwd_peer.set(String::new());
                        form.fwd_port.set(String::new());
                        apply_mesh(state, Some(form.busy),
                            format!("Forwarding 127.0.0.1:{listen} to the peer's {port}."),
                            fetch::mesh_add_forward(MeshForwardRef { listen, peer, port, name: None }));
                    }
                    _ => {}
                }
            }>
                <TextField id="fwd-listen" label="Local port" placeholder="5000" numeric=true value=form.fwd_listen />
                <TextField id="fwd-peer" label="Peer id or ticket" placeholder="peer to reach" wide=true mono=true
                    field_style="flex:1 1 220px; min-width:0" value=form.fwd_peer />
                <TextField id="fwd-port" label="Remote port" placeholder="3000" numeric=true value=form.fwd_port />
                <button class="adi-btn adi-btn--primary" type="submit" prop:disabled=move || form.busy.get()>
                    "Add forward"
                </button>
            </form>
        </section>
    }
    .into_any()
}

/// The `data-state` value for the "This machine" status pill.
fn mesh_state_data(mesh: RwSignal<Option<MeshState>>) -> &'static str {
    match mesh.get() {
        Some(m) if m.running => "online",
        Some(_) => "down",
        None => "unknown",
    }
}

/// Rows for the exposed-ports table: a placeholder, or one row per allowed port with a
/// button to stop exposing it.
fn mesh_allow_rows(state: State) -> AnyView {
    let Some(mesh) = state.mesh.get() else {
        return placeholder_row("2", "Loading…");
    };
    if mesh.allow.is_empty() {
        return placeholder_row("2", "No ports exposed — add one below to let peers reach it.");
    }
    let mut ports = mesh.allow;
    ports.sort_unstable();
    ports
        .into_iter()
        .map(|port| {
            view! {
                <tr>
                    <td class="adi-mono adi-table__port">{port.to_string()}</td>
                    <td style="text-align:right">
                        <button class="adi-btn adi-btn--link" on:click=move |_| {
                            apply_mesh(state, None, format!("Stopped exposing port {port}."),
                                fetch::mesh_deny(port));
                        }>"Remove"</button>
                    </td>
                </tr>
            }
        })
        .collect::<Vec<_>>()
        .into_any()
}

/// Rows for the authorized-peers table: a note when open to any peer, else one row per id.
fn mesh_peer_rows(state: State) -> AnyView {
    let Some(mesh) = state.mesh.get() else {
        return placeholder_row("2", "Loading…");
    };
    if mesh.authorized_peers.is_empty() {
        return placeholder_row(
            "2",
            "Any peer may use the exposed ports. Add one to restrict access.",
        );
    }
    mesh.authorized_peers
        .into_iter()
        .map(|peer| {
            let full = peer.clone();
            view! {
                <tr>
                    <td class="adi-mono" title=full.clone()>{short_id(&peer)}</td>
                    <td style="text-align:right">
                        <button class="adi-btn adi-btn--link" on:click=move |_| {
                            apply_mesh(state, None, "Revoked the peer.".to_string(),
                                fetch::mesh_deny_peer(full.clone()));
                        }>"Revoke"</button>
                    </td>
                </tr>
            }
        })
        .collect::<Vec<_>>()
        .into_any()
}

/// Rows for the forwards table: a placeholder, or one row per forward with a remove button.
fn mesh_forward_rows(state: State) -> AnyView {
    let Some(mesh) = state.mesh.get() else {
        return placeholder_row("5", "Loading…");
    };
    if mesh.forwards.is_empty() {
        return placeholder_row(
            "5",
            "No forwards — add one below to reach a peer's port locally.",
        );
    }
    mesh.forwards
        .into_iter()
        .map(|f| {
            let listen = f.listen;
            view! {
                <tr>
                    <td>{f.name}</td>
                    <td class="adi-mono adi-table__port">{format!("127.0.0.1:{}", f.listen)}</td>
                    <td class="adi-mono" title=f.peer.clone()>{short_id(&f.peer)}</td>
                    <td class="adi-mono">{format!(":{}", f.port)}</td>
                    <td style="text-align:right">
                        <button class="adi-btn adi-btn--link" on:click=move |_| {
                            apply_mesh(state, None, format!("Removed the forward on 127.0.0.1:{listen}."),
                                fetch::mesh_remove_forward(listen));
                        }>"Remove"</button>
                    </td>
                </tr>
            }
        })
        .collect::<Vec<_>>()
        .into_any()
}

/// Run a mesh mutation: set the returned state and a success flash, or an error flash;
/// toggles `busy` around the request when a form is driving it.
fn apply_mesh<F>(state: State, busy: Option<RwSignal<bool>>, ok_msg: String, fut: F)
where
    F: std::future::Future<Output = Result<MeshState, String>> + 'static,
{
    apply_mutation(state, busy, ok_msg, |s, m| s.mesh.set(Some(m)), fut);
}

/// Parse a `1..=65535` port from user input, rejecting blanks and `0`.
fn parse_port(raw: &str) -> Option<u16> {
    match raw.trim().parse::<u16>() {
        Ok(p) if p != 0 => Some(p),
        _ => None,
    }
}

/// A compact display for a peer token: `ticket` for a ticket, else a shortened id.
fn short_id(s: &str) -> String {
    if s.starts_with("adimesh:") {
        "ticket".to_string()
    } else if s.len() > 16 {
        format!("{}…{}", &s[..8], &s[s.len() - 4..])
    } else {
        s.to_string()
    }
}

/// Copy a read-only field's text to the clipboard: select it (a visible affordance and a
/// manual-copy fallback), then write it via `navigator.clipboard` on wasm. Best-effort.
fn copy_field(node: NodeRef<leptos::html::Input>) {
    if let Some(input) = node.get() {
        input.select();
        #[cfg(target_arch = "wasm32")]
        clipboard_write(&input.value());
    }
}

/// One-click clipboard write via `navigator.clipboard.writeText`, as a tiny JS shim — so it
/// needs neither the unstable web-sys Clipboard API nor its cfg flag. wasm target only.
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen::prelude::wasm_bindgen(
    inline_js = "export function adiClipboardWrite(t){ try { if (navigator.clipboard) navigator.clipboard.writeText(t); } catch (e) {} }"
)]
extern "C" {
    #[wasm_bindgen(js_name = adiClipboardWrite)]
    fn clipboard_write(text: &str);
}

/// Select all text of the focused input, so clicking the id/ticket field readies a manual copy.
fn select_target(ev: &web_sys::FocusEvent) {
    if let Some(input) = ev
        .target()
        .and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok())
    {
        input.select();
    }
}

// ---- client-side routing ------------------------------------------------------------

/// The pages the sidebar navigates between, each mapped to a URL path.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Route {
    Overview,
    Projects,
    /// A single project's detail page (`/projects/<id>`); the id lives in `State::current_project`.
    ProjectDetail,
    /// The read-only task tree (`/tasks`).
    Tasks,
    /// Agent definitions (`/agents`).
    Agents,
    Hive,
    PortsManager,
    Mesh,
}

impl Route {
    /// The page for a URL path; `/`, `/overview`, and anything unknown resolve to Overview.
    fn from_path(path: &str) -> Self {
        if project_id_from_path(path).is_some() {
            return Route::ProjectDetail;
        }
        match path {
            "/projects" => Route::Projects,
            "/tasks" => Route::Tasks,
            "/agents" => Route::Agents,
            "/settings/hive" => Route::Hive,
            "/settings/ports-manager" => Route::PortsManager,
            "/settings/mesh" => Route::Mesh,
            _ => Route::Overview,
        }
    }

    /// The canonical URL path for this page. `ProjectDetail`'s real path carries an id, so this
    /// returns the list base for it (used only for nav; detail canonicalization is skipped).
    fn path(self) -> &'static str {
        match self {
            Route::Overview => "/overview",
            Route::Projects | Route::ProjectDetail => "/projects",
            Route::Tasks => "/tasks",
            Route::Agents => "/agents",
            Route::Hive => "/settings/hive",
            Route::PortsManager => "/settings/ports-manager",
            Route::Mesh => "/settings/mesh",
        }
    }

    /// The page title shown in the header.
    fn title(self) -> &'static str {
        match self {
            Route::Overview => "Overview",
            Route::Projects => "Projects",
            Route::ProjectDetail => "Project",
            Route::Tasks => "Tasks",
            Route::Agents => "Agents",
            Route::Hive => "Hive",
            Route::PortsManager => "Ports Manager",
            Route::Mesh => "Mesh",
        }
    }
}

/// `aria-current` for a nav link: `"page"` when it points at the active route.
fn aria_current(route: RwSignal<Route>, target: Route) -> &'static str {
    if route.get() == target {
        "page"
    } else {
        "false"
    }
}

/// Handle a click on a nav link: navigate client-side for a plain left-click, but let
/// modified clicks (new tab/window, etc.) fall through to a normal browser navigation.
fn spa_click(ev: &web_sys::MouseEvent, route: RwSignal<Route>, target: Route) {
    if ev.default_prevented()
        || ev.button() != 0
        || ev.meta_key()
        || ev.ctrl_key()
        || ev.shift_key()
        || ev.alt_key()
    {
        return;
    }
    ev.prevent_default();
    if route.get_untracked() != target {
        push_state(target.path());
        route.set(target);
        scroll_top();
    }
}

/// Push a new history entry for `path` without reloading the page.
fn push_state(path: &str) {
    if let Some(h) = web_sys::window().and_then(|w| w.history().ok()) {
        let _ = h.push_state_with_url(&wasm_bindgen::JsValue::NULL, "", Some(path));
    }
}

/// Replace the current history entry's URL (canonicalizes the address bar on first load).
fn replace_state(path: &str) {
    if let Some(h) = web_sys::window().and_then(|w| w.history().ok()) {
        let _ = h.replace_state_with_url(&wasm_bindgen::JsValue::NULL, "", Some(path));
    }
}

/// Scroll back to the top after a page change.
fn scroll_top() {
    if let Some(w) = web_sys::window() {
        w.scroll_to_with_x_and_y(0.0, 0.0);
    }
}

/// The current URL path, e.g. `/settings/ports-manager`.
fn current_path() -> String {
    web_sys::window()
        .and_then(|w| w.location().pathname().ok())
        .unwrap_or_default()
}

/// The project id in a `/projects/<id>` detail path, or `None` for any other path (including
/// the bare `/projects` list). The trailing segment must be non-empty and slash-free.
fn project_id_from_path(path: &str) -> Option<String> {
    let rest = path.strip_prefix("/projects/")?;
    if rest.is_empty() || rest.contains('/') {
        None
    } else {
        Some(rest.to_string())
    }
}

/// Navigate to a project's detail page, clearing any stale detail so it shows a loading state.
fn open_project(state: State, route: RwSignal<Route>, id: String) {
    state.project_detail.set(None);
    // Clear the file browser so the load effect re-fetches from this project's root.
    state.files.reset();
    state.current_project.set(id.clone());
    push_state(&format!("/projects/{id}"));
    route.set(Route::ProjectDetail);
    scroll_top();
}

/// Navigate back to the projects list.
fn go_projects(state: State, route: RwSignal<Route>) {
    state.current_project.set(String::new());
    state.files.reset();
    push_state(Route::Projects.path());
    route.set(Route::Projects);
    scroll_top();
}

/// Signals a data refresh writes to; `Copy` (each field is an arena handle) so it threads
/// cheaply through async tasks and event handlers.
#[derive(Clone, Copy)]
struct State {
    status: RwSignal<Status>,
    ports: RwSignal<Option<PortsState>>,
    health: RwSignal<Option<Health>>,
    flash: RwSignal<Option<Flash>>,
    secs_since: RwSignal<u32>,
    used: RwSignal<Option<UsedPorts>>,
    mesh: RwSignal<Option<MeshState>>,
    projects: RwSignal<Option<ProjectsState>>,
    project_detail: RwSignal<Option<ProjectDetail>>,
    current_project: RwSignal<String>,
    /// The read-only task tree (`/api/tasks`), shown on the Tasks page.
    tasks: RwSignal<Option<TasksState>>,
    /// Agent definitions (`/api/agents`), shown on the Agents page.
    agents: RwSignal<Option<AgentsState>>,
    hive: RwSignal<Option<HiveState>>,
    /// The project file browser/editor state (the Files panel on the detail page).
    files: FilesState,
}

/// The project detail page's file browser + editor state, scoped to the open project's own
/// directory (served through the isolated `adi-fs` jail). `Copy` (arena handles) so it threads
/// into the view and async handlers. Loading is navigation-driven, not part of the 4s poll, so
/// the poll never clobbers the editor buffer.
#[derive(Clone, Copy)]
struct FilesState {
    /// The directory currently being browsed, relative to the project root (`""` is the root).
    dir: RwSignal<String>,
    /// The listing of `dir`, or `None` while loading.
    listing: RwSignal<Option<DirListing>>,
    /// The file open in the editor (its path relative to the project root), or `None`.
    open: RwSignal<Option<String>>,
    /// The open file's last-loaded/saved content — compared against `buffer` to detect edits.
    original: RwSignal<String>,
    /// The editable textarea buffer.
    buffer: RwSignal<String>,
    /// Whether a read/write is in flight (disables the editor's buttons).
    busy: RwSignal<bool>,
    /// Which project id the browser currently reflects — so re-entering a fresh project reloads.
    loaded_for: RwSignal<String>,
}

impl FilesState {
    /// Fresh signals for the file browser (root dir, nothing loaded or open).
    fn new() -> Self {
        Self {
            dir: RwSignal::new(String::new()),
            listing: RwSignal::new(None),
            open: RwSignal::new(None),
            original: RwSignal::new(String::new()),
            buffer: RwSignal::new(String::new()),
            busy: RwSignal::new(false),
            loaded_for: RwSignal::new(String::new()),
        }
    }

    /// Clear the browser back to "nothing loaded" (used when leaving a project or switching to
    /// another), so the load effect re-fetches from the root next time.
    fn reset(self) {
        self.dir.set(String::new());
        self.listing.set(None);
        self.open.set(None);
        self.original.set(String::new());
        self.buffer.set(String::new());
        self.loaded_for.set(String::new());
    }
}

/// The Projects page's local signals: the create-form inputs, a busy flag, and the
/// active/archived filter. `Copy` so it threads into the page view and handlers.
#[derive(Clone, Copy)]
struct ProjectsForm {
    id: RwSignal<String>,
    name: RwSignal<String>,
    description: RwSignal<String>,
    busy: RwSignal<bool>,
    show_archived: RwSignal<bool>,
}

/// The Tasks page's local signals: the create-form inputs (title, optional parent id, optional
/// tag, optional details) and a busy flag. The `tag` matters — a tag matching an agent name is
/// what auto-assigns/auto-starts the task (see docs/adi-agents.md). `Copy` so it threads into
/// the page view and handlers.
#[derive(Clone, Copy)]
struct TasksForm {
    title: RwSignal<String>,
    parent: RwSignal<String>,
    tag: RwSignal<String>,
    details: RwSignal<String>,
    busy: RwSignal<bool>,
}

/// The Agents page's local create/edit form. Numeric fields (`temperature`, `max_turns`) are held
/// as strings and parsed on submit; `editing` is `Some(name)` while an existing agent is loaded
/// into the form (drives the header + a "New agent" reset). `Copy` so it threads into handlers.
#[derive(Clone, Copy)]
struct AgentsForm {
    name: RwSignal<String>,
    backend: RwSignal<String>,
    model: RwSignal<String>,
    permission_mode: RwSignal<String>,
    temperature: RwSignal<String>,
    max_turns: RwSignal<String>,
    tags: RwSignal<String>,
    tools: RwSignal<String>,
    system_prompt: RwSignal<String>,
    starred: RwSignal<bool>,
    editing: RwSignal<Option<String>>,
    busy: RwSignal<bool>,
}

/// The reserve form's local signals; `Copy` so it threads into the page view and handlers.
#[derive(Clone, Copy)]
struct Form {
    svc: RwSignal<String>,
    key: RwSignal<String>,
    reserving: RwSignal<bool>,
    reserved: RwSignal<String>,
}

/// The Mesh page's local signals: the three add-forms' inputs, a shared busy flag, and node
/// refs to the id/ticket fields so the Copy buttons can select their text. `Copy` so it
/// threads into the page view and handlers.
#[derive(Clone, Copy)]
struct MeshForm {
    allow_port: RwSignal<String>,
    peer: RwSignal<String>,
    fwd_listen: RwSignal<String>,
    fwd_peer: RwSignal<String>,
    fwd_port: RwSignal<String>,
    busy: RwSignal<bool>,
    id_ref: NodeRef<leptos::html::Input>,
    ticket_ref: NodeRef<leptos::html::Input>,
}

/// Fetch `/api/health` + `/api/ports` together and fan the result into the signals.
async fn load(s: State) {
    match (fetch::health().await, fetch::ports().await) {
        (Ok(h), Ok(p)) => {
            s.health.set(Some(h));
            s.ports.set(Some(p));
            s.status.set(Status::Online);
            s.secs_since.set(0);
        }
        (Err(e), _) | (_, Err(e)) => {
            s.status.set(Status::Down);
            s.flash
                .set(Some(Flash::err(format!("Couldn't reach the backend: {e}"))));
        }
    }
    // Page-specific data, fetched only where it's shown.
    let path = current_path();
    if path == Route::Projects.path()
        && let Ok(p) = fetch::projects().await
    {
        s.projects.set(Some(p));
    }
    if let Some(id) = project_id_from_path(&path)
        && let Ok(d) = fetch::project_detail(&id).await
    {
        s.project_detail.set(Some(d));
    }
    if path == Route::Tasks.path()
        && let Ok(t) = fetch::tasks().await
    {
        s.tasks.set(Some(t));
    }
    if path == Route::Agents.path()
        && let Ok(a) = fetch::agents().await
    {
        s.agents.set(Some(a));
    }
    if path == Route::Hive.path()
        && let Ok(h) = fetch::hive().await
    {
        s.hive.set(Some(h));
    }
    if path == Route::PortsManager.path()
        && let Ok(u) = fetch::used().await
    {
        s.used.set(Some(u));
    }
    if path == Route::Mesh.path()
        && let Ok(m) = fetch::mesh().await
    {
        s.mesh.set(Some(m));
    }
}

/// Render the port table body: a loading/empty placeholder, or one row per lease sorted
/// by port. Reads `ports` reactively, so it re-renders on every refresh.
fn rows_view(state: State) -> AnyView {
    match state.ports.get() {
        None => placeholder_row("4", "Loading…"),
        Some(p) if p.leases.is_empty() => {
            placeholder_row("4", "No ports reserved yet — reserve one below.")
        }
        Some(p) => {
            let mut leases = p.leases;
            leases.sort_by_key(|l| l.port);
            leases
                .into_iter()
                .map(|l| {
                    let service = l.service.clone();
                    let key = l.key.clone();
                    view! {
                        <tr>
                            <td class="adi-mono">{l.service}</td>
                            <td class="adi-mono">{l.key}</td>
                            <td class="adi-mono adi-table__port">{l.port.to_string()}</td>
                            <td style="text-align:right">
                                <button class="adi-btn adi-btn--link" on:click=move |_| {
                                    let service = service.clone();
                                    let key = key.clone();
                                    spawn_local(async move {
                                        let req = LeaseRef { service, key };
                                        match fetch::release(&req).await {
                                            Ok(r) => {
                                                let msg = match r.freed {
                                                    Some(port) => format!("Released port {port}."),
                                                    None => "Nothing to release.".to_string(),
                                                };
                                                state.flash.set(Some(Flash::ok(msg)));
                                                load(state).await;
                                            }
                                            Err(e) => state.flash.set(Some(Flash::err(e))),
                                        }
                                    });
                                }>"Release"</button>
                            </td>
                        </tr>
                    }
                })
                .collect::<Vec<_>>()
                .into_any()
        }
    }
}

/// Render the "ports in use" table body: every listening port, or only the ADI-managed
/// ones when `managed_only`. A port is ADI-managed when a registry lease binds it.
fn used_rows_view(state: State, managed_only: RwSignal<bool>) -> AnyView {
    let Some(used) = state.used.get() else {
        return placeholder_row("4", "Scanning…");
    };
    let leases = state.ports.get().map(|p| p.leases).unwrap_or_default();
    let managed = managed_only.get();

    let rows: Vec<_> = used
        .ports
        .into_iter()
        .filter_map(|u| {
            let lease = leases.iter().find(|l| l.port == u.port).cloned();
            // ADI-managed: bound by a registry lease, or owned by an `adi-*` service process.
            let is_adi =
                lease.is_some() || u.process.as_deref().is_some_and(|p| p.starts_with("adi"));
            if managed && !is_adi {
                return None;
            }
            Some((u, lease))
        })
        .collect();

    if rows.is_empty() {
        let msg = if managed {
            "No ADI-managed ports are listening."
        } else {
            "No listening ports found."
        };
        return placeholder_row("4", msg);
    }

    rows.into_iter()
        .map(|(u, lease)| {
            let owner = match lease {
                Some(l) => view! {
                    <td><span class="adi-chip">{format!("{}/{}", l.service, l.key)}</span></td>
                }
                .into_any(),
                None => view! { <td class="adi-muted">"—"</td> }.into_any(),
            };
            let process = dash(u.process);
            let pid = u.pid.map_or_else(|| "—".to_string(), |p| p.to_string());
            view! {
                <tr>
                    <td class="adi-mono adi-table__port">{u.port.to_string()}</td>
                    <td>{process}</td>
                    <td class="adi-mono adi-muted">{pid}</td>
                    {owner}
                </tr>
            }
        })
        .collect::<Vec<_>>()
        .into_any()
}

/// The "updated Ns ago" label; empty until the first successful load.
fn updated_text(ports: RwSignal<Option<PortsState>>, secs_since: RwSignal<u32>) -> String {
    if ports.get().is_none() {
        return String::new();
    }
    match secs_since.get() {
        0 => "updated just now".to_string(),
        s => format!("updated {s}s ago"),
    }
}

/// Backend liveness as shown by the status pill.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Status {
    Connecting,
    Online,
    Down,
}

impl Status {
    /// The `data-state` value the CSS keys the LED colour off.
    fn data(self) -> &'static str {
        match self {
            Status::Connecting => "unknown",
            Status::Online => "online",
            Status::Down => "down",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Status::Connecting => "connecting…",
            Status::Online => "online",
            Status::Down => "offline",
        }
    }
}

/// A one-line status message under the form; `kind` drives its colour via `data-kind`.
#[derive(Clone)]
struct Flash {
    kind: &'static str,
    msg: String,
}

impl Flash {
    fn ok(msg: String) -> Self {
        Self { kind: "ok", msg }
    }

    fn err(msg: String) -> Self {
        Self { kind: "err", msg }
    }
}

/// Format an uptime in seconds as `Ns` / `Nm Ss` / `Nh Mm`.
fn fmt_uptime(s: u64) -> String {
    if s < 60 {
        format!("{s}s")
    } else if s < 3_600 {
        format!("{}m {}s", s / 60, s % 60)
    } else {
        format!("{}h {}m", s / 3_600, (s % 3_600) / 60)
    }
}

// ---- theme toggle (persisted; falls back to the OS preference) ----------------------

/// Apply the theme saved in `localStorage`, if any, to `<html data-theme>`.
fn apply_saved_theme() {
    if let Some(theme) = storage().and_then(|s| s.get_item("adi-theme").ok().flatten())
        && let Some(el) = document_element()
    {
        let _ = el.set_attribute("data-theme", &theme);
    }
}

/// Flip the theme and persist the choice, seeding from the OS preference when unset.
fn toggle_theme() {
    let Some(el) = document_element() else {
        return;
    };
    let current = match el.get_attribute("data-theme") {
        Some(t) if !t.is_empty() => t,
        _ if prefers_dark() => "dark".to_string(),
        _ => "light".to_string(),
    };
    let next = if current == "dark" { "light" } else { "dark" };
    let _ = el.set_attribute("data-theme", next);
    if let Some(s) = storage() {
        let _ = s.set_item("adi-theme", next);
    }
}

fn document_element() -> Option<web_sys::Element> {
    web_sys::window()?.document()?.document_element()
}

fn storage() -> Option<web_sys::Storage> {
    web_sys::window()?.local_storage().ok().flatten()
}

fn prefers_dark() -> bool {
    web_sys::window()
        .and_then(|w| w.match_media("(prefers-color-scheme: dark)").ok().flatten())
        .is_some_and(|m| m.matches())
}

/// Thin fetch layer over the `/api/*` endpoints, deserializing into the shared DTOs.
mod fetch {
    use adi_webapp_api::types::{
        ApiError, DirListing, FileContent, FilesRef, Health, HiveState, MeshForwardRef,
        MeshListenRef, MeshPeerRef, MeshPortRef, MeshState, NewProject, PortsState, ProjectDetail,
        AgentRef, AgentsState, NewTask, ProjectRef, ProjectsState, ReleaseResponse,
        ReserveResponse, SaveAgent, StartResult, StartService, StopResult, TasksState, UsedPorts,
        WriteFile,
    };
    use gloo_net::http::{Request, Response};
    use serde::Serialize;
    use serde::de::DeserializeOwned;

    use super::LeaseRef;

    pub async fn health() -> Result<Health, String> {
        get("/api/health").await
    }

    pub async fn ports() -> Result<PortsState, String> {
        get("/api/ports").await
    }

    pub async fn used() -> Result<UsedPorts, String> {
        get("/api/ports/used").await
    }

    pub async fn reserve(body: &LeaseRef) -> Result<ReserveResponse, String> {
        post("/api/ports/reserve", body).await
    }

    pub async fn release(body: &LeaseRef) -> Result<ReleaseResponse, String> {
        post("/api/ports/release", body).await
    }

    // Mesh: every endpoint returns the fresh MeshState so the page updates in one round-trip.

    pub async fn mesh() -> Result<MeshState, String> {
        get("/api/mesh").await
    }

    pub async fn mesh_start() -> Result<MeshState, String> {
        post("/api/mesh/start", &()).await
    }

    pub async fn mesh_stop() -> Result<MeshState, String> {
        post("/api/mesh/stop", &()).await
    }

    pub async fn mesh_allow(port: u16) -> Result<MeshState, String> {
        post("/api/mesh/allow", &MeshPortRef { port }).await
    }

    pub async fn mesh_deny(port: u16) -> Result<MeshState, String> {
        post("/api/mesh/deny", &MeshPortRef { port }).await
    }

    pub async fn mesh_allow_peer(peer: String) -> Result<MeshState, String> {
        post("/api/mesh/peers/allow", &MeshPeerRef { peer }).await
    }

    pub async fn mesh_deny_peer(peer: String) -> Result<MeshState, String> {
        post("/api/mesh/peers/deny", &MeshPeerRef { peer }).await
    }

    pub async fn mesh_add_forward(body: MeshForwardRef) -> Result<MeshState, String> {
        post("/api/mesh/forwards/add", &body).await
    }

    pub async fn mesh_remove_forward(listen: u16) -> Result<MeshState, String> {
        post("/api/mesh/forwards/remove", &MeshListenRef { listen }).await
    }

    // Projects: every endpoint returns the fresh ProjectsState so the page updates in one round-trip.

    pub async fn projects() -> Result<ProjectsState, String> {
        get("/api/projects").await
    }

    pub async fn create_project(body: NewProject) -> Result<ProjectsState, String> {
        post("/api/projects/create", &body).await
    }

    pub async fn archive_project(id: String) -> Result<ProjectsState, String> {
        post("/api/projects/archive", &ProjectRef { id }).await
    }

    pub async fn unarchive_project(id: String) -> Result<ProjectsState, String> {
        post("/api/projects/unarchive", &ProjectRef { id }).await
    }

    pub async fn project_detail(id: &str) -> Result<ProjectDetail, String> {
        get(&format!("/api/projects/{id}")).await
    }

    pub async fn remove_project(id: String) -> Result<ProjectsState, String> {
        post("/api/projects/remove", &ProjectRef { id }).await
    }

    pub async fn tasks() -> Result<TasksState, String> {
        get("/api/tasks").await
    }

    pub async fn create_task(body: NewTask) -> Result<TasksState, String> {
        post("/api/tasks/create", &body).await
    }

    // Agents: every endpoint returns the fresh AgentsState so the page updates in one round-trip.

    pub async fn agents() -> Result<AgentsState, String> {
        get("/api/agents").await
    }

    pub async fn save_agent(body: SaveAgent) -> Result<AgentsState, String> {
        post("/api/agents/save", &body).await
    }

    pub async fn delete_agent(name: String) -> Result<AgentsState, String> {
        post("/api/agents/delete", &AgentRef { name }).await
    }

    pub async fn hive() -> Result<HiveState, String> {
        get("/api/hive").await
    }

    pub async fn start_service(
        project: Option<String>,
        service: String,
    ) -> Result<StartResult, String> {
        post("/api/hive/start", &StartService { project, service }).await
    }

    pub async fn stop_service(
        project: Option<String>,
        service: String,
    ) -> Result<StopResult, String> {
        post("/api/hive/stop", &StartService { project, service }).await
    }

    // Project files: browse/read/edit the files under a project's own directory (jailed to it).

    pub async fn list_files(id: &str, path: &str) -> Result<DirListing, String> {
        post(
            "/api/projects/files",
            &FilesRef {
                id: id.to_string(),
                path: path.to_string(),
            },
        )
        .await
    }

    pub async fn read_file(id: &str, path: &str) -> Result<FileContent, String> {
        post(
            "/api/projects/file/read",
            &FilesRef {
                id: id.to_string(),
                path: path.to_string(),
            },
        )
        .await
    }

    pub async fn write_file(id: &str, path: &str, content: &str) -> Result<FileContent, String> {
        post(
            "/api/projects/file/write",
            &WriteFile {
                id: id.to_string(),
                path: path.to_string(),
                content: content.to_string(),
            },
        )
        .await
    }

    async fn get<T: DeserializeOwned>(url: &str) -> Result<T, String> {
        let resp = Request::get(url).send().await.map_err(stringify)?;
        finish(resp).await
    }

    async fn post<B: Serialize, T: DeserializeOwned>(url: &str, body: &B) -> Result<T, String> {
        let resp = Request::post(url)
            .json(body)
            .map_err(stringify)?
            .send()
            .await
            .map_err(stringify)?;
        finish(resp).await
    }

    /// Turn a response into `T`, or a message: the API's `{ error }` if present, else the
    /// HTTP status line.
    async fn finish<T: DeserializeOwned>(resp: Response) -> Result<T, String> {
        let status = resp.status();
        let text = resp.text().await.map_err(stringify)?;
        if !(200..300).contains(&status) {
            let msg = serde_json::from_str::<ApiError>(&text)
                .map_or_else(|_| format!("{status} {}", resp.status_text()), |e| e.error);
            return Err(msg);
        }
        serde_json::from_str(&text).map_err(stringify)
    }

    fn stringify<E: std::fmt::Display>(e: E) -> String {
        e.to_string()
    }
}
