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

mod attach;
mod fetch;
mod icons;
mod live;
mod pages;
mod pwa;
mod routing;
mod state;
mod store_browser;
mod ui;
mod voice;

// The component library. The titlebar is the first thing on this page built from it; the
// rest of the screen is still the `adi-*` layer, and the two share a page by load order
// (see `styles/tailwind.css`).
use adi_ui::{
    Button, ButtonSize, ButtonVariant, Crumb, Crumbs, Faq, Modal, Qna, TopBar, Tree, TreeNode,
    TreeState,
};
use adi_webapp_api::types::{
    AgentsState, DashboardsState, DbState, FleetState, Health, HiveState,
    MeshState, MetaState,
    PortsState, ProjectDetail, ProjectsState, SecretsState, TasksState, ToolsState,
    TriggersState, UsedPorts, WorkspacesState,
};
use gloo_timers::callback::Interval;
use leptos::prelude::*;
use wasm_bindgen::JsCast;
use wasm_bindgen::closure::Closure;
use wasm_bindgen_futures::spawn_local;

use pages::{
    OnboardingForm, agents_view, analytics_view, chat_home_view, dashboards_view, database_view,
    fleet_view,
    hive_view, knowledge_view, live_view, load_dir, load_store_file,
    mesh_view, meta_view, onboarding_view, poll_hook_log, poll_term, poll_trigger_log, poll_watch,
    ports_manager_view, project_detail_view, projects_view, reset_chat_home, secrets_view,
    seed_onboarding, start_onb_reconfigure, store_file_view,
    tasks_view, tools_view, triggers_view,
};
use routing::{
    ProjectSection, Route, current_path, open_project_section, project_id_from_path,
    project_section_from_path, query_param, replace_state, spa_click,
};
use state::{
    AgentsForm, AgentsWatch, DashboardsForm, DbConsole, FilesState, FleetForm, Simulate,
    Flash, Form, HookLogView, KnowledgeConsole,
    MeshForm, MetaForm, ProjectsForm, ROOT_AGENT, SecretsForm, State, Status, TasksForm, TermWatch,
    ToolEditor, ToolRunView, ToolsForm, TriggersForm, TriggersLogView, load,
    refresh_fleet_dashboards,
};
use ui::{apply_saved_theme, fmt_uptime, toggle_theme};

fn main() {
    console_error_panic_hook::set_once();
    apply_saved_theme();
    // Open the live channel before anything mounts, so the first subscription a page makes goes
    // out on a socket that is already connecting rather than waiting for one to be asked for.
    live::start();
    let path = current_path();
    // Three doors into the one wasm bundle:
    //   * `/embed/dashboard-agent` — a chrome-less page (no workbench shell) hosting the global
    //     agent chat, opened from a dashboard's "edit with adi-agent" launcher.
    //   * `/extended/…` — the full control panel (the App shell + every workbench route).
    //   * anything else (notably the bare `/`) — the minimal launcher that just points at it.
    if path.starts_with("/embed/dashboard-agent") {
        mount_to_body(EmbedDashboardAgent);
    } else if path == "/extended" || path.starts_with("/extended/") {
        mount_to_body(App);
    } else {
        mount_to_body(Home);
    }
    drop_boot_splash();
}

/// Remove the pre-wasm splash `index.html` paints (the wordmark, so the first frame is adi and
/// not a blank page). `mount_to_body` *appends*, so without this the splash would stay above the
/// mounted app. The wasm side keeps showing its own identical [`boot_splash`] while `/api/meta`
/// is in flight, so the handover is invisible.
fn drop_boot_splash() {
    if let Some(el) = web_sys::window()
        .and_then(|w| w.document())
        .and_then(|d| d.get_element_by_id("adi-boot"))
    {
        el.remove();
    }
}

/// The front door's boot screen: the wordmark alone, held while we still don't know what this
/// stack is. Onboarding copy ("Welcome to adi.") would claim a first run before `/api/meta`
/// has said whether it *is* one — for the far more common set-up stack that reads as a wrong
/// turn on every load. Mirrors the pre-wasm splash markup in `index.html`.
fn boot_splash() -> AnyView {
    view! {
        <div class="adi-boot">
            <span class="adi-boot__logo">"adi"<span class="adi-boot__dot">"."</span></span>
        </div>
    }
    .into_any()
}

/// The root (`/`). It opens on the wordmark ([`boot_splash`]) and commits to a shape only once
/// `/api/meta` answers: before the root agent exists, a guided onboarding wizard (welcome,
/// stepper, setup form) behind a slim `adi. · extended →` bar; **once the agent exists the app is
/// the chat** — its sessions on the left, the conversation in the centre, dashboards on the right
/// (see [`chat_home_view`]). The bar's "reconfigure" returns to the setup form to change the agent.
#[component]
fn Home() -> impl IntoView {
    let state = State::fresh();
    let watch = AgentsWatch::new();
    // The chat and the wizard share one meta signal — it decides which of the two shows.
    let meta = state.meta;
    // Everything the setup wizard edits: the agent form, the chosen setup preset, its API key.
    let onb = OnboardingForm::new();
    // True once the browser will let us offer "install as an app" (see [`pwa`]).
    let can_install = pwa::installable();
    // The FAQ dialog. It lives here rather than in the bar so both shapes of this page —
    // the chat and the wizard — open the same one.
    let faq_open = RwSignal::new(false);

    // Load the meta state once, seeding the wizard from the server's defaults (or from the agent
    // itself, when there already is one).
    spawn_local(async move {
        if let Ok(m) = fetch::meta().await {
            seed_onboarding(onb, &m);
            meta.set(Some(m));
        }
    });
    // The secret names already in the store — how the wizard knows not to ask again for a key that
    // was pasted once. Names only: `/api/secrets` never carries a value.
    spawn_local(async move {
        if let Ok(s) = fetch::secrets().await {
            state.secrets.set(Some(s));
        }
    });

    // Keep the chat pointed at *the agent it is on* and the rails fresh: detect that agent's
    // executor (pty ⇒ interactive) so the live view picks the right shape, point the watch at the
    // root agent the first time it appears, and refresh the sessions/dashboards lists. Runs on load,
    // on creation, and on a poll. The watched agent is whichever one the rail's picker chose — the
    // root agent only until then — so this follows a switch instead of dragging the view back.
    let refresh = move || {
        spawn_local(async move {
            if let Ok(a) = fetch::agents().await {
                let watched = watch.name.get_untracked();
                let on = watched.clone().unwrap_or_else(|| ROOT_AGENT.to_string());
                if let Some(d) = a.agents.iter().find(|d| d.name == on) {
                    let interactive = d.executor == "pty";
                    if watch.interactive.get_untracked() != interactive {
                        watch.interactive.set(interactive);
                    }
                    if watched.is_none() {
                        watch.name.set(Some(on));
                        poll_watch(watch);
                    }
                }
                // Only on change: the picker and the sessions rail read this list, and a select
                // rebuilt every 4s would drop an open dropdown on the floor.
                if state.agents.get_untracked().as_ref() != Some(&a) {
                    state.agents.set(Some(a));
                }
            }
            // Every agent's sessions in one round-trip — what the rail lists under "Other agents"
            // — cut to the page the rail is currently showing. Read untracked because this runs
            // inside a task rather than an effect: what re-reads it when Load more moves is the
            // subscription below, which is what asks in the ordinary case anyway.
            let limit = Some(state.rail_limit.get_untracked());
            if let Ok(c) = fetch::all_agent_runs(limit).await
                && state.all_chats.get_untracked().as_ref() != Some(&c)
            {
                state.all_chats.set(Some(c));
            }
            if let Ok(dd) = fetch::dashboards().await
                && state.dashboards.get_untracked().as_ref() != Some(&dd)
            {
                state.dashboards.set(Some(dd));
            }
            // The dashboards rail groups by project, so it needs the project names.
            if let Ok(pp) = fetch::projects().await
                && state.projects.get_untracked().as_ref() != Some(&pp)
            {
                state.projects.set(Some(pp));
            }
        });
    };

    // The rail's fleet half, asked once when the chat comes up. Not part of `refresh` and not a
    // subscription: each read is an authenticated call to every paired node over the mesh, so it
    // happens on load and when the rail's Refresh asks again — never on the four-second tick that
    // the local lists ride.
    let asked_fleet = RwSignal::new(false);
    Effect::new(move |_| {
        if meta.get().is_some_and(|m| m.agent.is_some()) && !asked_fleet.get_untracked() {
            asked_fleet.set(true);
            refresh_fleet_dashboards(state);
        }
    });

    // Wire up the chat as soon as the agent exists — on load, or right after the setup form creates it.
    Effect::new(move |_| {
        if meta.get().is_some_and(|m| m.agent.is_some()) {
            refresh();
        }
    });
    // What the chat home watches: the open conversation, plus the rails around it. The same lists
    // `refresh` fetches, arriving only when they change instead of every four seconds.
    Effect::new(move |_| {
        let mut subs = state::chat_subscriptions(watch);
        subs.push(live::Sub::get("/api/agents", move |a: AgentsState| {
            // Keep the live view shaped to *the agent it is on*: a pty backend is interactive.
            let watched = watch.name.get_untracked();
            let on = watched.clone().unwrap_or_else(|| ROOT_AGENT.to_string());
            if let Some(d) = a.agents.iter().find(|d| d.name == on) {
                let interactive = d.executor == "pty";
                if watch.interactive.get_untracked() != interactive {
                    watch.interactive.set(interactive);
                }
                if watched.is_none() {
                    watch.name.set(Some(on));
                }
            }
            // Only on change: the picker and the sessions rail read this list, and a select
            // rebuilt on every message would drop an open dropdown on the floor.
            if state.agents.get_untracked().as_ref() != Some(&a) {
                state.agents.set(Some(a));
            }
        }));
        // Every agent's sessions — what the rail lists under "Other agents" — and the dashboards
        // rail, which groups by project and so needs the project names.
        //
        // Only the rail's current page of sessions, and `rail_limit` is read *tracked*: pressing
        // Load more re-runs this effect, which re-subscribes at the wider path and so asks for the
        // next page immediately rather than at whatever the socket's next tick would have been.
        subs.push(live::Sub::get(
            fetch::all_runs_path(Some(state.rail_limit.get())),
            move |c: adi_webapp_api::types::AllAgentRuns| {
                if state.all_chats.get_untracked().as_ref() != Some(&c) {
                    state.all_chats.set(Some(c));
                }
            },
        ));
        subs.push(live::Sub::get(
            "/api/dashboards",
            move |d: DashboardsState| {
                if state.dashboards.get_untracked().as_ref() != Some(&d) {
                    state.dashboards.set(Some(d));
                }
            },
        ));
        subs.push(live::Sub::get("/api/projects", move |p: ProjectsState| {
            if state.projects.get_untracked().as_ref() != Some(&p) {
                state.projects.set(Some(p));
            }
        }));
        live::watch(subs);
    });

    // The fallback, while the live channel is down — the polling this page used to do.
    Interval::new(1_000, move || {
        if !live::connected() {
            poll_watch(watch);
        }
    })
    .forget();
    Interval::new(4_000, move || {
        if !live::connected() {
            refresh();
        }
    })
    .forget();

    // Bar "reconfigure": seed the setup form from the stored agent, then flip into reconfigure mode
    // so the centred wizard shows in place of the chat.
    let start_reconfigure = move || {
        if let Some(m) = meta.get_untracked() {
            start_onb_reconfigure(onb, &m);
        }
    };

    move || {
        // Three doors, decided by what we actually know. Reads only `meta`/`reconfiguring`, so a
        // poll never rebuilds the tree.
        //   * `/api/meta` hasn't answered yet ⇒ the wordmark splash. We can't tell a first run
        //     from a set-up stack, so we say nothing rather than guess "welcome".
        //   * the agent exists (and we're not mid-reconfigure) ⇒ the chat.
        //   * no agent, or reconfiguring ⇒ the wizard.
        let Some(m) = meta.get() else {
            return boot_splash();
        };
        if m.agent.is_some() && !onb.reconfiguring.get() {
            view! {
                <div class="adi-chome-root">
                    // No `home` on the mark: this page *is* home, and a link to where you
                    // already are is a control that does nothing. It still has somewhere to
                    // put you — this screen as it opened, with no conversation selected — so
                    // the mark does that instead, the way the rail's "+ New" does.
                    <TopBar
                        class="adi-ui-type"
                        logo="adi"
                        on_home=Callback::new(move |()| reset_chat_home(state, watch))
                        actions=move || {
                            view! {
                                {install_pill(can_install)}
                                <Button
                                    size=ButtonSize::Small
                                    variant=ButtonVariant::Ghost
                                    on:click=move |_| start_reconfigure()
                                >
                                    "reconfigure agent"
                                </Button>
                                {analytics_link()}
                                {faq_button(faq_open)}
                                {extended_link()}
                            }
                            .into_any()
                        }
                    />
                    <Modal open=faq_open title="Questions" width="max-w-3xl">
                        <Faq items=faq()/>
                    </Modal>
                    {chat_home_view(state, watch)}
                </div>
            }
            .into_any()
        } else {
            view! {
                <div class="adi-onb">
                    <TopBar
                        class="adi-ui-type"
                        logo="adi"
                        actions=move || {
                            view! {
                                {install_pill(can_install)}
                                {analytics_link()}
                                {faq_button(faq_open)}
                                {extended_link()}
                            }
                            .into_any()
                        }
                    />
                    <Modal open=faq_open title="Questions" width="max-w-3xl">
                        <Faq items=faq()/>
                    </Modal>
                    <main class="adi-onb__body">
                        <div class="adi-onb__panel">{onboarding_view(state, onb, m)}</div>
                    </main>
                </div>
            }
            .into_any()
        }
    }
}

/// The way to the [`Route::Analytics`] page: what every agent on this machine has actually run,
/// which of them are working, and which have never been launched at all.
///
/// A plain link rather than a modal like the FAQ beside it, because it is a *page* — it lives in
/// the control panel's own explorer too, and a screen that answers a question about forty agents
/// wants the width. Same document as `extended →`, so this is the one dialog-free control here
/// that navigates.
fn analytics_link() -> impl IntoView {
    view! {
        <a
            class="inline-flex h-6 items-center gap-1 rounded-sm px-2 text-mini font-medium \
                   text-meta no-underline hover:bg-card hover:text-ink hover:no-underline"
            href=Route::Analytics.path()
            title="What every agent has run — and which have never been launched"
        >
            <svg
                class="size-3 shrink-0"
                viewBox="0 0 16 16"
                fill="none"
                stroke="currentColor"
                stroke-width="1.5"
                stroke-linecap="round"
                stroke-linejoin="round"
                aria-hidden="true"
                inner_html=icons::Icon::Chart.path()
            ></svg>
            <span>"Global Analytics"</span>
        </a>
    }
}

/// The way into the FAQ, to the left of the way out to the control panel: the two things
/// this bar offers are "explain this" and "show me more of it", in that order.
fn faq_button(open: RwSignal<bool>) -> impl IntoView {
    view! {
        <Button
            size=ButtonSize::Small
            variant=ButtonVariant::Ghost
            icon=icons::Icon::Question.path()
            on:click=move |_| open.set(true)
        >
            "FAQ"
        </Button>
    }
}

/// The way through to the control panel. A plain link, not a route: `/extended` is a
/// different document, and the wasm bundle decides which of the two it is at boot.
fn extended_link() -> impl IntoView {
    view! {
        <a
            class="inline-flex h-6 items-center gap-1 rounded-sm px-2 text-mini font-medium \
                   text-meta no-underline hover:bg-card hover:text-ink hover:no-underline"
            href="/extended"
        >
            <span>"extended"</span>
            <span aria-hidden="true">"\u{2192}"</span>
        </a>
    }
}

/// What people ask before they have used this for an hour. Answers are Markdown, so a path
/// or a command can be one — see [`adi_ui::Qna`].
///
/// A few for now; this list is meant to grow, and it costs one entry per question.
fn faq() -> Vec<Qna> {
    vec![
        Qna::new(
            "What is adi?",
            "A place to run agents on your own machine. It keeps their sessions, their tools \
             and their credentials in one store, and gives every one of them a front door on \
             your network — so an agent is something you can come back to rather than a tab \
             you left open.",
        ),
        Qna::new(
            "What is the difference between this page and *extended*?",
            "This page is the chat: one agent, one conversation, the things it produced. \
             **Extended** is the control panel behind it — projects, tasks, tools, secrets, \
             the database, the mesh. Same stack, more surface.",
        ),
        Qna::new(
            "Where does my data live?",
            "On this machine, under `~/.adi`. Sessions, transcripts and caches all sit in \
             that one directory, and nothing leaves it except the calls an agent's own \
             provider needs.",
        ),
        Qna::new(
            "Can I install it as an app?",
            "Yes — the **Install** button appears in this bar when your browser has an \
             install to offer. It runs in its own window, keeps its own icon, and works \
             offline for everything that does not need the backend.",
        ),
        Qna::new(
            "How do I add another agent?",
            "In **extended → agents**. Every agent gets a runtime (a CLI in a live terminal, \
             or a headless SDK loop), a prompt, and whatever tools you give it.",
        ),
        Qna::new(
            "Something is stuck. What do I look at first?",
            "The run's own log — every agent run keeps one, and it is the first place a \
             failure says what it was. After that, **extended → hive** shows what is \
             actually running on this machine and on what port.",
        ),
    ]
}

/// The root bar's "install app" pill, styled like its `extended →` neighbour. Rendered only
/// while the browser actually has an install to offer, so it's absent once the app is
/// installed and on origins that can't install at all — see [`pwa`].
fn install_pill(can_install: RwSignal<bool>) -> impl IntoView {
    move || {
        can_install.get().then(|| {
            view! {
                <button class="adi-onb__ext" type="button"
                    title="Install adi as an app in its own window"
                    on:click=move |_| pwa::install()>
                    <span aria-hidden="true">"\u{2913}"</span>
                    <span>"install app"</span>
                </button>
            }
        })
    }
}


/// The chrome-less dashboard-agent embed (`/embed/dashboard-agent?dashboard=<id>`): the one global
/// `adi-agent` chat, opened from a dashboard's launcher. It reuses the agent live view, points it at
/// `adi-agent`, and starts the conversation **in that dashboard's directory**, tagged with which
/// dashboard it was opened from — the agent then edits that dashboard's `.ts` files. Served by
/// app.adi, so its API calls are same-origin (no CORS).
#[component]
fn EmbedDashboardAgent() -> impl IntoView {
    let state = State::fresh();
    let watch = AgentsWatch::new();
    let dashboard = query_param("dashboard").unwrap_or_default();

    if !dashboard.is_empty() {
        // Until the directory is known, the agent is told the path — the conventional one — and to
        // move there once. It is the honest fallback: a chat sent before the listing lands (or at
        // all, if it fails) still knows where the files are.
        watch.context_prefix.set(format!(
            "[Context: you are editing dashboard {dashboard}. Its files are at \
             ~/.adi/mono/dashboards/{dashboard} — UI panels in frontend/modules/*.ts, endpoints in \
             backend/routes/*.ts. `cd` there once, then edit those .ts files by relative path; the \
             dashboard hot-reloads.]"
        ));
        // Then *start the conversation there*, rather than handing over a path and leaving every
        // command to re-state it. The launch directory is the one statement about location that
        // can't be forgotten halfway through a run — it is the process's own cwd, so relative
        // paths land in the dashboard by construction. The server states the path
        // (`GET /api/dashboards` → `dir`), so the embed never rebuilds the store's layout out of
        // an id; once it has it, the prefix stops mentioning directories at all.
        let id = dashboard.clone();
        spawn_local(async move {
            if let Ok(boards) = fetch::dashboards().await
                && let Some(board) = boards.dashboards.iter().find(|d| d.id == id)
                && !board.dir.is_empty()
            {
                watch.run_dir.set(board.dir.clone());
                watch.context_prefix.set(format!(
                    "[Context: you are editing dashboard {id}, and this chat already runs in its \
                     directory — UI panels in frontend/modules/*.ts, endpoints in \
                     backend/routes/*.ts. Edit those .ts files by relative path; the dashboard \
                     hot-reloads.]"
                ));
            }
        });
    }

    // Learn whether adi-agent is interactive (pty) vs headless, point the live view at it, and poll.
    spawn_local(async move {
        if let Ok(a) = fetch::agents().await {
            let interactive = a
                .agents
                .iter()
                .find(|d| d.name == ROOT_AGENT)
                .is_some_and(|d| d.executor == "pty");
            watch.interactive.set(interactive);
            state.agents.set(Some(a));
        }
        watch.name.set(Some(ROOT_AGENT.to_string()));
        poll_watch(watch);
    });
    // The embed shows one chat and nothing else, so that chat is all it watches.
    Effect::new(move |_| live::watch(state::chat_subscriptions(watch)));
    Interval::new(1_000, move || {
        if !live::connected() {
            poll_watch(watch);
        }
    })
    .forget();

    let ctx_label = dashboard.clone();
    view! {
        <div class="adi-embed">
            <header class="adi-embed__head">
                <span class="adi-embed__brand">"adi\u{00b7}agent"</span>
                {(!ctx_label.is_empty()).then(|| view! {
                    <span class="adi-embed__ctx adi-mono" title=ctx_label.clone()>
                        {format!("\u{270e} {}", ctx_label.chars().take(8).collect::<String>())}
                    </span>
                })}
            </header>
            <div class="adi-embed__body">
                {move || live_view(state, watch)}
            </div>
        </div>
    }
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
    let fleet = RwSignal::new(None::<FleetState>);
    let projects = RwSignal::new(None::<ProjectsState>);
    let project_detail = RwSignal::new(None::<ProjectDetail>);
    let tasks = RwSignal::new(None::<TasksState>);
    let agents = RwSignal::new(None::<AgentsState>);
    let all_chats = RwSignal::new(None::<adi_webapp_api::types::AllAgentRuns>);
    let tools = RwSignal::new(None::<ToolsState>);
    let secrets = RwSignal::new(None::<SecretsState>);
    let db = RwSignal::new(None::<DbState>);
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
    // True once the browser will let us offer "install as an app" (see [`pwa`]).
    let can_install = pwa::installable();
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
        fleet,
        projects,
        project_detail,
        current_project,
        current_section,
        tasks,
        agents,
        all_chats,
        tools,
        secrets,
        db,
        meta,
        triggers,
        hive,
        dashboards,
        // The workbench shell has no dashboards rail, so nothing here ever asks the fleet what it
        // runs — the signals exist because `State` is one shape, not because this page fills them.
        fleet_dashboards: RwSignal::new(None),
        fleet_dashboards_busy: RwSignal::new(false),
        fleet_unlock: state::FleetUnlock::new(),
        workspaces,
        files,
        store,
        row_menu: RwSignal::new(None),
        session_menu: RwSignal::new(None),
        show_hidden: RwSignal::new(false),
        starred_only: RwSignal::new(false),
        // No sessions rail on the workbench shell either — its "All chats" table reads the whole
        // index, so nothing here pages. The field exists because `State` is one shape.
        rail_limit: RwSignal::new(state::SESSION_PAGE),
        chat_drawer: RwSignal::new(None),
        // Each table restores the arrangement its user last left, else its declared columns.
        tables: state::Tables::new(),
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
    let explorer = TreeState::new();

    let dashboards_form = DashboardsForm {
        name: RwSignal::new(String::new()),
        description: RwSignal::new(String::new()),
        busy: RwSignal::new(false),
        show_archived: RwSignal::new(false),
        transfer_id: RwSignal::new(String::new()),
        transfer_node: RwSignal::new(String::new()),
        transfer_move: RwSignal::new(false),
        transfer_delete: RwSignal::new(false),
        transfer_password: RwSignal::new(String::new()),
        transfer_busy: RwSignal::new(false),
    };

    let tasks_form = TasksForm {
        title: RwSignal::new(String::new()),
        project: RwSignal::new(String::new()),
        parent: RwSignal::new(String::new()),
        tag: RwSignal::new(String::new()),
        cwd: RwSignal::new(String::new()),
        details: RwSignal::new(String::new()),
        busy: RwSignal::new(false),
        show_done: RwSignal::new(false),
    };

    let agents_form = AgentsForm::new();

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
        events: RwSignal::new(String::new()),
        trigger_on: RwSignal::new(Vec::new()),
        editing: RwSignal::new(None::<String>),
        busy: RwSignal::new(false),
    };

    let triggers_log = TriggersLogView::new();
    let hook_log = HookLogView::new();
    let term_watch = TermWatch::new();
    let agents_watch = AgentsWatch::new();
    // The simulator's own state: which run has a person in its seat. Apart from `agents_watch`,
    // which watches a run somebody else is doing — see `state::Simulate`.
    let agents_sim = Simulate::new();

    // The Tools page's create/link form, and the run + script-editor panels it shares with a
    // project's Tools panel (page-scoped so they survive re-renders and thread into both).
    let tools_form = ToolsForm::new();
    let tool_editor = ToolEditor::new();
    let tool_run = ToolRunView::new();
    let db_console = DbConsole::new();

    // The Knowledge page's search box, base list, and note reader. Page-local like the SQL
    // console: a base's counts cost a status pass over every base's storage, which is not
    // something the shell's 4s poll should be doing on every page.
    let knowledge = KnowledgeConsole::new();

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

    let fleet_form = FleetForm::new();

    let managed_only = RwSignal::new(true);

    // What the Global Analytics chart's bars measure: false counts runs, true adds up what they
    // cost. Here rather than in the page for the reason every other view signal is — a page
    // function re-runs on each route render, and a signal made inside one forgets on every redraw.
    let analytics_spend = RwSignal::new(false);

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

    // "updated Ns ago" counts from the last time the backend said anything — which on the live
    // channel is any pushed answer, not a poll landing.
    live::on_message(move || secs_since.set(0));

    // Tell the backend what this page is looking at, and re-tell it whenever the page moves — a
    // route change, a different project, another chat or log or terminal opened. Everything the
    // shell used to fetch on a timer now arrives on the socket, and only when it has changed.
    Effect::new(move |_| {
        live::watch(state::subscriptions(
            state,
            route.get(),
            agents_watch,
            triggers_log,
            hook_log,
            term_watch,
        ));
    });

    // The fallback, for a backend that can't hold a socket open: exactly the polling this page
    // used to do, and only while the live channel is down. Load once at startup either way, so a
    // first paint never waits on the handshake.
    spawn_local(load(state));
    Interval::new(4_000, move || {
        if !live::connected() {
            spawn_local(load(state));
        }
    })
    .forget();
    // The "updated Ns ago" label ticks regardless — it counts local time, not requests.
    Interval::new(1_000, move || {
        secs_since.update(|s| *s = s.saturating_add(1));
        if !live::connected() {
            poll_watch(agents_watch);
            poll_trigger_log(triggers_log);
            poll_hook_log(hook_log);
            poll_term(term_watch);
        }
    })
    .forget();

    // Refresh immediately when a page that has page-specific data opens (the port scan on
    // Ports Manager, the mesh state on Mesh), so it isn't stale. On the live channel the
    // subscription effect above has already done this — a new page means a new watch list, which
    // the server answers with a snapshot straight away — so only the fallback path fetches here.
    //
    // `opened` is `None` on the very first run, which is the mount rather than a navigation: the
    // `load` above has already covered it, and fetching again would double every request a page
    // load makes.
    Effect::new(move |opened: Option<()>| {
        // Re-run when the open project changes too, so navigating detail A → B reloads.
        let _ = current_project.get();
        // Any page change closes an open row menu (its row is about to unmount anyway).
        state.row_menu.set(None);
        if matches!(
            route.get(),
            Route::Meta
                | Route::Analytics
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
                | Route::Fleet
        ) && opened.is_some()
            && !live::connected()
        {
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
        // Closing the terminal view never kills the pty session — it just stops the poll.
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
        // The frame's lid — the first thing on this page built from `adi-ui` rather than
        // from the `adi-*` layer around it. Identity on the left, where you are next to it,
        // and the ways out on the right.
        <TopBar
            logo="adi"
            // The mark is the way home: `/` is the chat, and the control panel is the
            // room you stepped into from it.
            home="/"
            actions=move || {
                view! {
                    // The way back out of the control panel. The root bar offers
                    // "extended →" in the other direction, so the trip is round rather than
                    // one-way; a plain link, since `/` is a different document than the
                    // workbench and not an SPA route.
                    <a
                        class="inline-flex h-6 items-center gap-1 rounded-sm px-2 text-mini \
                               font-medium text-meta no-underline hover:bg-card hover:text-ink \
                               hover:no-underline"
                        href="/"
                        title="Back to the simple chat view"
                    >
                        <span aria-hidden="true">"\u{2190}"</span>
                        <span>"simple"</span>
                    </a>
                    {move || can_install.get().then(|| view! {
                        <Button
                            size=ButtonSize::Small
                            variant=ButtonVariant::Ghost
                            icon=icons::Icon::Download.path()
                            attr:title="Install adi as an app in its own window"
                            on:click=move |_| pwa::install()
                        >
                            "Install"
                        </Button>
                    })}
                    // Icon only, so it says what it is to a screen reader rather than
                    // nothing at all.
                    <Button
                        size=ButtonSize::Small
                        variant=ButtonVariant::Ghost
                        icon=icons::Icon::Contrast.path()
                        attr:title="Toggle theme"
                        attr:aria-label="Toggle theme"
                        on:click=move |_| toggle_theme()
                    />
                }
                .into_any()
            }
        >
            // Where you are, read left to right from the mark — the natural reading order,
            // and it keeps the bar from being two clumps with a void between them.
            <Crumbs items=Signal::derive(move || {
                let mut items = vec![Crumb::new(route.get().title())];
                let id = state.current_project.get();
                if matches!(route.get(), Route::ProjectDetail) && !id.is_empty() {
                    items.push(Crumb::new(id));
                }
                items
            })/>
        </TopBar>

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
                        Route::Analytics => analytics_view(state, analytics_spend),
                        Route::Projects => projects_view(state, projects_form, route),
                        Route::ProjectDetail => project_detail_view(state, route, triggers_log, agents_watch, agents_form, hook_log, term_watch, tool_editor, tool_run, knowledge),
                        Route::StoreFile => store_file_view(state),
                        Route::Tasks => tasks_view(state, tasks_form),
                        Route::Agents => agents_view(state, agents_form, agents_watch, agents_sim),
                        Route::Tools => tools_view(state, tools_form, tool_editor, tool_run),
                        Route::Secrets => secrets_view(state, secrets_form),
                        Route::Knowledge => knowledge_view(state, knowledge),
                        Route::Database => database_view(state, db_console),
                        Route::Triggers => triggers_view(state, triggers_form, triggers_log),
                        Route::Dashboards => dashboards_view(state, dashboards_form),
                        Route::Hive => hive_view(state, route),
                        Route::PortsManager => ports_manager_view(state, form, managed_only),
                        Route::Mesh => mesh_view(state, mesh_form),
                        Route::Fleet => fleet_view(state, fleet_form),
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
            Route::Analytics,
            Route::Projects,
            Route::Tasks,
            Route::Agents,
            Route::Tools,
            Route::Secrets,
            Route::Knowledge,
            Route::Database,
            Route::Triggers,
            Route::Dashboards,
        ],
    ),
    (
        "Settings",
        &[Route::Hive, Route::PortsManager, Route::Mesh, Route::Fleet],
    ),
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
fn explorer_tree(state: State, explorer: TreeState, route: RwSignal<Route>) -> AnyView {
    let mut nodes: Vec<TreeNode> = Vec::new();

    for (label, routes) in GLOBAL_SCOPES {
        nodes.push(
            TreeNode::new(format!("scope:{label}"), 0, label)
                .children(true)
                .container(true)
                .icon(scope_icon(label).path()),
        );
        for r in routes {
            nodes.push(
                TreeNode::new(format!("route:{}", r.path()), 1, r.title())
                    .icon(icons::route_icon(*r).path()),
            );
        }
    }

    let Some(projects) = state.projects.get() else {
        return view! {
            <Tree nodes=nodes state=explorer empty="Loading…" class="adi-ui-type"/>
        }
        .into_any();
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
            TreeNode::new(format!("proj:{}", p.id), *depth, p.name.clone())
                // Always a branch: even a project with no sub-projects holds its sections.
                .children(true)
                .icon(icons::Icon::Folder.path())
                .emphasis(true)
                .separated(first_child)
                .maybe_badge(open.filter(|n| *n > 0).map(|n| n.to_string()))
                .maybe_title(p.description.clone()),
        );
        for section in ProjectSection::ALL {
            nodes.push(
                TreeNode::new(
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
    view! {
        <Tree
            nodes=nodes
            state=explorer
            selected=selected
            empty="Nothing here yet."
            class="adi-ui-type"
        />
    }
    .into_any()
}
