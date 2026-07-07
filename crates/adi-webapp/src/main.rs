//! adi-webapp — the adi control-panel UI, a Leptos client-side-rendered app compiled to
//! wasm by Trunk. It talks to the `/api/*` backend using the DTO types from
//! [`adi_webapp_api`], so the wire format is shared with the server rather than duplicated.
//! Trunk's `dist/` output is embedded into [`adi-app`](../adi-app), which serves it at
//! `app.adi`.

#![allow(non_snake_case)] // Leptos components are PascalCase by convention.

use adi_webapp_api::types::{
    Health, HiveState, LeaseRef, MeshForwardRef, MeshState, NewProject, PortsState, Project,
    ProjectDetail, ProjectsState, UsedPorts,
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
    let hive = RwSignal::new(None::<HiveState>);
    // The id of the project whose detail page is open ("" when not on one). Drives detail
    // loads so navigating from one project to another (route stays ProjectDetail) still refreshes.
    let current_project = RwSignal::new(project_id_from_path(&current_path()).unwrap_or_default());
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
        hive,
    };

    // The Projects page's local form: the create inputs, a busy flag, and the active/archived filter.
    let projects_form = ProjectsForm {
        id: RwSignal::new(String::new()),
        name: RwSignal::new(String::new()),
        description: RwSignal::new(String::new()),
        busy: RwSignal::new(false),
        show_archived: RwSignal::new(false),
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
                | Route::Hive
                | Route::PortsManager
                | Route::Mesh
        ) {
            spawn_local(load(state));
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
                    <a class="adi-nav__item" href=Route::Overview.path()
                        aria-current=move || aria_current(route, Route::Overview)
                        on:click=move |ev| spa_click(&ev, route, Route::Overview)>
                        <span>"Overview"</span>
                    </a>
                    <a class="adi-nav__item" href=Route::Projects.path()
                        aria-current=move || if matches!(route.get(), Route::Projects | Route::ProjectDetail) { "page" } else { "false" }
                        on:click=move |ev| spa_click(&ev, route, Route::Projects)>
                        <span>"Projects"</span>
                    </a>
                    <div class="adi-nav__group">
                        <div class="adi-nav__heading">"Settings"</div>
                        <a class="adi-nav__item" href=Route::Hive.path()
                            aria-current=move || aria_current(route, Route::Hive)
                            on:click=move |ev| spa_click(&ev, route, Route::Hive)>
                            <span>"Hive"</span>
                        </a>
                        <a class="adi-nav__item" href=Route::PortsManager.path()
                            aria-current=move || aria_current(route, Route::PortsManager)
                            on:click=move |ev| spa_click(&ev, route, Route::PortsManager)>
                            <span>"Ports Manager"</span>
                        </a>
                        <a class="adi-nav__item" href=Route::Mesh.path()
                            aria-current=move || aria_current(route, Route::Mesh)
                            on:click=move |ev| spa_click(&ev, route, Route::Mesh)>
                            <span>"Mesh"</span>
                        </a>
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

/// The Overview page: system liveness at a glance.
fn overview_view(state: State) -> AnyView {
    let State { health, .. } = state;
    view! {
        <section class="adi-tiles">
            <div class="adi-tile">
                <div class="adi-tile__label">"Uptime"</div>
                <div class="adi-tile__value">
                    {move || health.get().map_or_else(|| "—".to_string(), |h| fmt_uptime(h.uptime_secs))}
                </div>
                <div class="adi-tile__note">
                    {move || health.get().map_or_else(|| "adi-app".to_string(),
                        |h| format!("{} v{}", h.service, h.version))}
                </div>
            </div>
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
            <div class="adi-tile">
                <div class="adi-tile__label">"Projects"</div>
                <div class="adi-tile__value">
                    {move || projects.get().map_or_else(|| "—".to_string(), |p| p.projects.len().to_string())}
                </div>
                <div class="adi-tile__note">"registered manifests"</div>
            </div>
            <div class="adi-tile">
                <div class="adi-tile__label">"Active"</div>
                <div class="adi-tile__value">
                    {move || projects.get().map_or_else(|| "—".to_string(),
                        |p| p.projects.iter().filter(|x| !x.is_archived()).count().to_string())}
                </div>
                <div class="adi-tile__note">
                    {move || projects.get().map_or_else(|| "not archived".to_string(),
                        |p| format!("{} archived", p.projects.iter().filter(|x| x.is_archived()).count()))}
                </div>
            </div>
        </section>

        <section class="adi-panel">
            <div class="adi-panel__head">
                <h2 class="adi-panel__title">"Registered projects"</h2>
                <span class="adi-updated">{move || updated_text(state.ports, secs_since)}</span>
                <span class="adi-spacer"></span>
                <div class="adi-segmented" role="group" aria-label="Filter projects">
                    <button class="adi-segmented__option" type="button"
                        aria-pressed=move || (!show_archived.get()).to_string()
                        on:click=move |_| show_archived.set(false)>"Active"</button>
                    <button class="adi-segmented__option" type="button"
                        aria-pressed=move || show_archived.get().to_string()
                        on:click=move |_| show_archived.set(true)>"All"</button>
                </div>
            </div>

            <div class="adi-tablewrap">
                <table class="adi-table">
                    <thead>
                        <tr><th>"Name"</th><th>"ID"</th><th>"Created"</th><th>"Status"</th><th></th></tr>
                    </thead>
                    <tbody>
                        {move || project_rows(state, show_archived, route)}
                    </tbody>
                </table>
            </div>

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
                <div class="adi-field">
                    <label class="adi-field__label" for="proj-id">"Project id"</label>
                    <input class="adi-input adi-mono" id="proj-id" placeholder="my-app" autocomplete="off"
                        prop:value=move || id.get()
                        on:input=move |ev| id.set(event_target_value(&ev)) />
                </div>
                <div class="adi-field">
                    <label class="adi-field__label" for="proj-name">"Name"</label>
                    <input class="adi-input" id="proj-name" placeholder="My App (defaults to the id)" autocomplete="off"
                        prop:value=move || name.get()
                        on:input=move |ev| name.set(event_target_value(&ev)) />
                </div>
                <div class="adi-field" style="flex:1 1 240px; min-width:0">
                    <label class="adi-field__label" for="proj-desc">"Description"</label>
                    <input class="adi-input adi-input--wide" id="proj-desc" placeholder="optional one-liner" autocomplete="off"
                        prop:value=move || description.get()
                        on:input=move |ev| description.set(event_target_value(&ev)) />
                </div>
                <button class="adi-btn adi-btn--primary" type="submit" prop:disabled=move || busy.get()>
                    "Add project"
                </button>
            </form>
            <div class="adi-flash" data-kind=move || flash.get().map_or("none", |f| f.kind)>
                {move || flash.get().map(|f| f.msg).unwrap_or_default()}
            </div>
        </section>
    }
    .into_any()
}

/// Render the projects table body: a loading/empty placeholder, or one row per project
/// (filtered to active-only unless `show_archived`). The name opens the project's detail
/// page; the trailing action archives/restores it.
fn project_rows(state: State, show_archived: RwSignal<bool>, route: RwSignal<Route>) -> AnyView {
    let Some(state_projects) = state.projects.get() else {
        return view! { <tr><td class="adi-empty" colspan="5">"Loading…"</td></tr> }.into_any();
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
        return view! { <tr><td class="adi-empty" colspan="5">{msg}</td></tr> }.into_any();
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

/// Run a projects mutation: set the returned state and a success flash, or an error flash;
/// toggles `busy` around the request when a form is driving it.
fn apply_projects<F>(state: State, busy: Option<RwSignal<bool>>, ok_msg: String, fut: F)
where
    F: std::future::Future<Output = Result<ProjectsState, String>> + 'static,
{
    if let Some(b) = busy {
        b.set(true);
    }
    spawn_local(async move {
        match fut.await {
            Ok(p) => {
                state.projects.set(Some(p));
                state.flash.set(Some(Flash::ok(ok_msg)));
            }
            Err(e) => state.flash.set(Some(Flash::err(e))),
        }
        if let Some(b) = busy {
            b.set(false);
        }
    });
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

        <div class="adi-flash" data-kind=move || flash.get().map_or("none", |f| f.kind)>
            {move || flash.get().map(|f| f.msg).unwrap_or_default()}
        </div>
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
            <div class="adi-tile">
                <div class="adi-tile__label">"Created"</div>
                <div class="adi-tile__value">{created}</div>
                <div class="adi-tile__note">{archived_note}</div>
            </div>
            <div class="adi-tile">
                <div class="adi-tile__label">"Services"</div>
                <div class="adi-tile__value">{service_count.to_string()}</div>
                <div class="adi-tile__note">"from .adi/hive.yaml"</div>
            </div>
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
                <span class="adi-updated">"the project's .adi/hive.yaml"</span>
            </div>
            <div class="adi-tablewrap">
                <table class="adi-table">
                    <thead>
                        <tr><th>"Service"</th><th>"Host"</th><th>"Ports"</th><th>"Command"</th><th>"Restart"</th></tr>
                    </thead>
                    <tbody>{service_rows(services, has_hive)}</tbody>
                </table>
            </div>
        </section>
    }
    .into_any()
}

/// Rows for the services table: a message when there's no hive / no services, else one row per
/// service (host, ports as `key:port`, run command, restart policy).
fn service_rows(services: Vec<adi_webapp_api::types::ProjectService>, has_hive: bool) -> AnyView {
    if services.is_empty() {
        let msg = if has_hive {
            "This project's .adi/hive.yaml declares no services."
        } else {
            "No .adi/hive.yaml — this project has no runtime services yet."
        };
        return view! { <tr><td class="adi-empty" colspan="5">{msg}</td></tr> }.into_any();
    }
    services
        .into_iter()
        .map(|s| {
            let host = s.host.unwrap_or_else(|| "—".to_string());
            let ports = if s.ports.is_empty() {
                "—".to_string()
            } else {
                s.ports
                    .iter()
                    .map(|p| format!("{}:{}", p.key, p.port))
                    .collect::<Vec<_>>()
                    .join(", ")
            };
            let run = s.run.unwrap_or_else(|| "—".to_string());
            let restart = s.restart.unwrap_or_else(|| "—".to_string());
            view! {
                <tr>
                    <td class="adi-mono">{s.name}</td>
                    <td class="adi-mono">{host}</td>
                    <td class="adi-mono adi-table__port">{ports}</td>
                    <td class="adi-mono adi-muted">{run}</td>
                    <td class="adi-muted">{restart}</td>
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

/// The Hive settings page: every service declared across all projects' `.adi/hive.yaml` plus
/// the global front-door hive, each with a live running/stopped indicator.
fn hive_view(state: State, route: RwSignal<Route>) -> AnyView {
    let State { hive, .. } = state;
    view! {
        <section class="adi-tiles">
            <div class="adi-tile">
                <div class="adi-tile__label">"Services"</div>
                <div class="adi-tile__value">
                    {move || hive.get().map_or_else(|| "—".to_string(), |h| h.services.len().to_string())}
                </div>
                <div class="adi-tile__note">"across all projects + front-door"</div>
            </div>
            <div class="adi-tile">
                <div class="adi-tile__label">"Running"</div>
                <div class="adi-tile__value">
                    {move || hive.get().map_or_else(|| "—".to_string(),
                        |h| h.services.iter().filter(|s| s.running).count().to_string())}
                </div>
                <div class="adi-tile__note">
                    {move || hive.get().map_or_else(|| "primary port listening".to_string(),
                        |h| format!("{} stopped", h.services.iter().filter(|s| !s.running).count()))}
                </div>
            </div>
            <div class="adi-tile">
                <div class="adi-tile__label">"Projects"</div>
                <div class="adi-tile__value">
                    {move || hive.get().map_or_else(|| "—".to_string(), |h| {
                        let mut ids: Vec<&String> = h.services.iter().filter_map(|s| s.project.as_ref()).collect();
                        ids.sort_unstable();
                        ids.dedup();
                        ids.len().to_string()
                    })}
                </div>
                <div class="adi-tile__note">"contributing services (+ front-door)"</div>
            </div>
        </section>

        <section class="adi-panel">
            <div class="adi-panel__head">
                <h2 class="adi-panel__title">"Hive services"</h2>
                <span class="adi-spacer"></span>
                <span class="adi-updated">
                    {move || hive.get().map_or(String::new(), |h| format!("{} services", h.services.len()))}
                </span>
            </div>
            <div class="adi-tablewrap">
                <table class="adi-table">
                    <thead>
                        <tr>
                            <th>"Source"</th><th>"Service"</th><th>"Host"</th><th>"Ports"</th>
                            <th>"Command"</th><th>"Restart"</th><th>"Status"</th>
                        </tr>
                    </thead>
                    <tbody>{move || hive_rows(state, route)}</tbody>
                </table>
            </div>
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
        return view! { <tr><td class="adi-empty" colspan="7">"Loading…"</td></tr> }.into_any();
    };
    if h.services.is_empty() {
        return view! {
            <tr><td class="adi-empty" colspan="7">"No hive services declared in any project or the global hive."</td></tr>
        }
        .into_any();
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
            let host = s.host.unwrap_or_else(|| "—".to_string());
            let ports = if s.ports.is_empty() {
                "—".to_string()
            } else {
                s.ports.iter().map(|p| format!("{}:{}", p.key, p.port)).collect::<Vec<_>>().join(", ")
            };
            let run = s.run.unwrap_or_else(|| "—".to_string());
            let restart = s.restart.unwrap_or_else(|| "—".to_string());
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
            <div class="adi-tile">
                <div class="adi-tile__label">"Active leases"</div>
                <div class="adi-tile__value">
                    {move || ports.get().map_or_else(|| "—".to_string(), |p| p.leases.len().to_string())}
                </div>
                <div class="adi-tile__note">"reserved static ports"</div>
            </div>
            <div class="adi-tile">
                <div class="adi-tile__label">"Allocatable range"</div>
                <div class="adi-tile__value">
                    {move || ports.get().map_or_else(|| "—".to_string(),
                        |p| format!("{}–{}", p.range.start, p.range.end))}
                </div>
                <div class="adi-tile__note">
                    {move || ports.get().map_or_else(|| "ports handed out from here".to_string(), |p| {
                        let span = u32::from(p.range.end) - u32::from(p.range.start) + 1;
                        format!("{span} ports · {} reserved bands", p.reserved.len())
                    })}
                </div>
            </div>
        </section>

        <section class="adi-panel">
            <div class="adi-panel__head">
                <h2 class="adi-panel__title">"Port registry"</h2>
                <span class="adi-spacer"></span>
                <span class="adi-updated">{move || updated_text(ports, secs_since)}</span>
            </div>

            <div class="adi-tablewrap">
                <table class="adi-table">
                    <thead>
                        <tr><th>"Service"</th><th>"Key"</th><th>"Port"</th><th></th></tr>
                    </thead>
                    <tbody>
                        {move || rows_view(state)}
                    </tbody>
                </table>
            </div>

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
                <div class="adi-field">
                    <label class="adi-field__label" for="svc">"Service"</label>
                    <input class="adi-input" id="svc" placeholder="frontend" autocomplete="off"
                        prop:value=move || svc.get()
                        on:input=move |ev| svc.set(event_target_value(&ev)) />
                </div>
                <div class="adi-field">
                    <label class="adi-field__label" for="key">"Port key"</label>
                    <input class="adi-input" id="key" placeholder="http" autocomplete="off"
                        prop:value=move || key.get()
                        on:input=move |ev| key.set(event_target_value(&ev)) />
                </div>
                <button class="adi-btn adi-btn--primary" type="submit"
                    prop:disabled=move || reserving.get()>
                    "Reserve port"
                </button>
                <span class="adi-spacer" style="flex:1"></span>
                <span class="adi-chip adi-mono">{move || reserved.get()}</span>
            </form>
            <div class="adi-flash" data-kind=move || flash.get().map_or("none", |f| f.kind)>
                {move || flash.get().map(|f| f.msg).unwrap_or_default()}
            </div>
        </section>

        <section class="adi-panel">
            <div class="adi-panel__head">
                <h2 class="adi-panel__title">"Ports in use"</h2>
                <span class="adi-updated">
                    {move || used.get().map_or(String::new(), |u| format!("{} listening", u.ports.len()))}
                </span>
                <span class="adi-spacer"></span>
                <div class="adi-segmented" role="group" aria-label="Filter ports">
                    <button class="adi-segmented__option" type="button"
                        aria-pressed=move || (!managed_only.get()).to_string()
                        on:click=move |_| managed_only.set(false)>"All"</button>
                    <button class="adi-segmented__option" type="button"
                        aria-pressed=move || managed_only.get().to_string()
                        on:click=move |_| managed_only.set(true)>"ADI managed"</button>
                </div>
            </div>
            <div class="adi-tablewrap">
                <table class="adi-table">
                    <thead>
                        <tr><th>"Port"</th><th>"Process"</th><th>"PID"</th><th>"Owner"</th></tr>
                    </thead>
                    <tbody>
                        {move || used_rows_view(state, managed_only)}
                    </tbody>
                </table>
            </div>
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
            <div class="adi-tile">
                <div class="adi-tile__label">"Daemon"</div>
                <div class="adi-tile__value">
                    {move || mesh.get().map_or_else(|| "—".to_string(),
                        |m| if m.running { "running".to_string() } else { "stopped".to_string() })}
                </div>
                <div class="adi-tile__note">"runs adi-mesh; publishes a ticket while up"</div>
            </div>
            <div class="adi-tile">
                <div class="adi-tile__label">"Ports exposed"</div>
                <div class="adi-tile__value">
                    {move || mesh.get().map_or_else(|| "—".to_string(), |m| m.allow.len().to_string())}
                </div>
                <div class="adi-tile__note">"reachable by peers"</div>
            </div>
            <div class="adi-tile">
                <div class="adi-tile__label">"Forwards"</div>
                <div class="adi-tile__value">
                    {move || mesh.get().map_or_else(|| "—".to_string(), |m| m.forwards.len().to_string())}
                </div>
                <div class="adi-tile__note">"local → peer tunnels"</div>
            </div>
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
                    <div class="adi-copyrow">
                        <input class="adi-input adi-input--wide adi-mono" readonly=true node_ref=form.id_ref
                            prop:value=move || mesh.get().map(|m| m.id).unwrap_or_default()
                            on:focus=move |ev| select_target(&ev) />
                        <button class="adi-btn adi-btn--ghost" type="button"
                            on:click=move |_| copy_field(form.id_ref)>"Copy"</button>
                    </div>
                    <div class="adi-field__hint">"The minimal token a peer can dial (resolved via discovery)."</div>
                </div>
                <div class="adi-field">
                    <label class="adi-field__label">"Ticket"</label>
                    {move || match mesh.get().and_then(|m| m.ticket) {
                        Some(ticket) => view! {
                            <div class="adi-copyrow">
                                <input class="adi-input adi-input--wide adi-mono" readonly=true node_ref=form.ticket_ref
                                    prop:value=ticket on:focus=move |ev| select_target(&ev) />
                                <button class="adi-btn adi-btn--ghost" type="button"
                                    on:click=move |_| copy_field(form.ticket_ref)>"Copy"</button>
                            </div>
                        }.into_any(),
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
            <div class="adi-tablewrap">
                <table class="adi-table">
                    <thead><tr><th>"Port"</th><th></th></tr></thead>
                    <tbody>{move || mesh_allow_rows(state)}</tbody>
                </table>
            </div>
            <form class="adi-form" on:submit=move |ev| {
                ev.prevent_default();
                if let Some(port) = parse_port(&form.allow_port.get()) {
                    form.allow_port.set(String::new());
                    apply_mesh(state, Some(form.busy), format!("Exposed port {port} to peers."),
                        fetch::mesh_allow(port));
                }
            }>
                <div class="adi-field">
                    <label class="adi-field__label" for="mesh-allow-port">"Local port"</label>
                    <input class="adi-input" id="mesh-allow-port" inputmode="numeric" placeholder="3000"
                        autocomplete="off" prop:value=move || form.allow_port.get()
                        on:input=move |ev| form.allow_port.set(event_target_value(&ev)) />
                </div>
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
            <div class="adi-tablewrap">
                <table class="adi-table">
                    <thead><tr><th>"Endpoint ID"</th><th></th></tr></thead>
                    <tbody>{move || mesh_peer_rows(state)}</tbody>
                </table>
            </div>
            <form class="adi-form" on:submit=move |ev| {
                ev.prevent_default();
                let peer = form.peer.get().trim().to_string();
                if !peer.is_empty() {
                    form.peer.set(String::new());
                    apply_mesh(state, Some(form.busy), "Authorized the peer.".to_string(),
                        fetch::mesh_allow_peer(peer));
                }
            }>
                <div class="adi-field" style="flex:1 1 240px; min-width:0">
                    <label class="adi-field__label" for="mesh-peer">"Peer id or ticket"</label>
                    <input class="adi-input adi-input--wide adi-mono" id="mesh-peer" placeholder="an EndpointId or adimesh: ticket"
                        autocomplete="off" prop:value=move || form.peer.get()
                        on:input=move |ev| form.peer.set(event_target_value(&ev)) />
                </div>
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
            <div class="adi-tablewrap">
                <table class="adi-table">
                    <thead><tr><th>"Name"</th><th>"Local"</th><th>"Peer"</th><th>"Remote"</th><th></th></tr></thead>
                    <tbody>{move || mesh_forward_rows(state)}</tbody>
                </table>
            </div>
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
                <div class="adi-field">
                    <label class="adi-field__label" for="fwd-listen">"Local port"</label>
                    <input class="adi-input" id="fwd-listen" inputmode="numeric" placeholder="5000" autocomplete="off"
                        prop:value=move || form.fwd_listen.get()
                        on:input=move |ev| form.fwd_listen.set(event_target_value(&ev)) />
                </div>
                <div class="adi-field" style="flex:1 1 220px; min-width:0">
                    <label class="adi-field__label" for="fwd-peer">"Peer id or ticket"</label>
                    <input class="adi-input adi-input--wide adi-mono" id="fwd-peer" placeholder="peer to reach"
                        autocomplete="off" prop:value=move || form.fwd_peer.get()
                        on:input=move |ev| form.fwd_peer.set(event_target_value(&ev)) />
                </div>
                <div class="adi-field">
                    <label class="adi-field__label" for="fwd-port">"Remote port"</label>
                    <input class="adi-input" id="fwd-port" inputmode="numeric" placeholder="3000" autocomplete="off"
                        prop:value=move || form.fwd_port.get()
                        on:input=move |ev| form.fwd_port.set(event_target_value(&ev)) />
                </div>
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
        return view! { <tr><td class="adi-empty" colspan="2">"Loading…"</td></tr> }.into_any();
    };
    if mesh.allow.is_empty() {
        return view! {
            <tr><td class="adi-empty" colspan="2">"No ports exposed — add one below to let peers reach it."</td></tr>
        }
        .into_any();
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
        return view! { <tr><td class="adi-empty" colspan="2">"Loading…"</td></tr> }.into_any();
    };
    if mesh.authorized_peers.is_empty() {
        return view! {
            <tr><td class="adi-empty" colspan="2">"Any peer may use the exposed ports. Add one to restrict access."</td></tr>
        }
        .into_any();
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
        return view! { <tr><td class="adi-empty" colspan="5">"Loading…"</td></tr> }.into_any();
    };
    if mesh.forwards.is_empty() {
        return view! {
            <tr><td class="adi-empty" colspan="5">"No forwards — add one below to reach a peer's port locally."</td></tr>
        }
        .into_any();
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
    if let Some(b) = busy {
        b.set(true);
    }
    spawn_local(async move {
        match fut.await {
            Ok(m) => {
                state.mesh.set(Some(m));
                state.flash.set(Some(Flash::ok(ok_msg)));
            }
            Err(e) => state.flash.set(Some(Flash::err(e))),
        }
        if let Some(b) = busy {
            b.set(false);
        }
    });
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
    state.current_project.set(id.clone());
    push_state(&format!("/projects/{id}"));
    route.set(Route::ProjectDetail);
    scroll_top();
}

/// Navigate back to the projects list.
fn go_projects(state: State, route: RwSignal<Route>) {
    state.current_project.set(String::new());
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
    hive: RwSignal<Option<HiveState>>,
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
        None => view! { <tr><td class="adi-empty" colspan="4">"Loading…"</td></tr> }.into_any(),
        Some(p) if p.leases.is_empty() => view! {
            <tr><td class="adi-empty" colspan="4">"No ports reserved yet — reserve one below."</td></tr>
        }
        .into_any(),
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
        return view! { <tr><td class="adi-empty" colspan="4">"Scanning…"</td></tr> }.into_any();
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
        return view! { <tr><td class="adi-empty" colspan="4">{msg}</td></tr> }.into_any();
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
            let process = u.process.unwrap_or_else(|| "—".to_string());
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
        ApiError, Health, HiveState, MeshForwardRef, MeshListenRef, MeshPeerRef, MeshPortRef,
        MeshState, NewProject, PortsState, ProjectDetail, ProjectRef, ProjectsState,
        ReleaseResponse, ReserveResponse, UsedPorts,
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

    pub async fn hive() -> Result<HiveState, String> {
        get("/api/hive").await
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
