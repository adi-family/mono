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
mod launcher;
mod live;
mod menu;
mod origin;
mod pages;
mod pwa;
mod routing;
mod state;
mod store_browser;
mod ui;
mod update;
mod voice;

// The component library: the mark, the icons and the explorer tree come from it; the shell
// around them is the `adi-*` layer, and the two share a page by load order (see
// `styles/tailwind.css`).
use adi_ui::{
    Button, ButtonSize, ButtonVariant, Icon, IconSize, Lucide, Mark, MarkVariant, Tree, TreeNode,
    TreeState,
};
use adi_webapp_api::types::{
    AgentsState, DashboardsState, DbState, FleetState, Health, HiveState, MeshState, MetaState,
    PortsState, ProjectDetail, ProjectsState, SecretsState, TasksState, ToolsState, TriggersState,
    UsedPorts, WorkspacesState,
};
use gloo_timers::callback::Interval;
use launcher::{Action, Launcher};
use leptos::prelude::*;
use wasm_bindgen::JsCast;
use wasm_bindgen::closure::Closure;
use wasm_bindgen_futures::spawn_local;

use pages::{
    FactsConsole, OnboardingForm, adopt_run_settings, agent_detail_view, agents_view,
    analytics_view, chat_home_view, dashboards_view, database_view, facts_view, fleet_view,
    hive_view, knowledge_view, live_view, load_agent_into_form, load_dir, load_store_file,
    marketplace_view, mesh_view, meta_view, onboarding_view, poll_hook_log, poll_term,
    poll_trigger_log, poll_watch, ports_manager_view, project_detail_view, projects_view,
    reset_chat_home, secrets_view, seed_onboarding, start_onb_reconfigure, store_file_view,
    tasks_view, tools_view, triggers_view,
};
use routing::{
    ProjectSection, Route, current_path, open_project_section, project_id_from_path,
    project_section_from_path, query_param, replace_state, spa_click,
};
use state::{
    AgentsForm, AgentsWatch, DashboardsForm, DbConsole, FilesState, Flash, FleetForm, Form,
    HookLogView, KnowledgeConsole, MarketplaceForm, MeshForm, MetaForm, ProjectsForm, ROOT_AGENT,
    SecretsForm, SessionFilter, Simulate, State, Status, TasksForm, TermWatch, ToolEditor,
    ToolRunView, ToolsForm, TriggersForm, TriggersLogView, load, refresh_fleet_dashboards,
};
use ui::fmt_uptime;

fn main() {
    console_error_panic_hook::set_once();
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

/// The front door's boot screen: the mark and the wordmark, held while we still don't know what
/// this stack is. Onboarding copy ("Welcome to adi.") would claim a first run before `/api/meta`
/// has said whether it *is* one — for the far more common set-up stack that reads as a wrong
/// turn on every load. Mirrors the pre-wasm splash markup in `index.html`, which is what makes
/// the handover between the two invisible.
fn boot_splash() -> AnyView {
    view! {
        <div class="adi-boot">
            // Solid rather than cut: the splash never draws the mark below 72px, and above ~64
            // the lobes are told apart by their tones without hairline gaps.
            <Mark variant=MarkVariant::Solid class="adi-boot__mark"/>
            <span class="adi-boot__logo">"adi"</span>
        </div>
    }
    .into_any()
}

/// The root (`/`). It opens on the wordmark ([`boot_splash`]) and commits to a shape only once
/// `/api/meta` answers: before the root agent exists, a guided onboarding wizard (welcome,
/// stepper, setup form); **once the agent exists the app is the chat** — its sessions on the
/// left, the conversation in the centre, dashboards on the right (see [`chat_home_view`]).
///
/// Neither of those wears a bar. What one would have carried is in the `⌘K` menu instead
/// ([`launcher`]) — the way back to the setup form among them ([`root_actions`]). The chat
/// opens it from the line above its sessions rail, the wizard from a mark in the corner.
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
    // What version this machine is on, and whether a newer one is published for it. Started
    // here rather than inside the bar below because the bar is rebuilt whenever `meta` moves,
    // and this owns a timer (see [`update::watch`]).
    let updates = update::watch();

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
                        // Whatever this browser last set this agent up to run as — see
                        // `adopt_run_settings`. Here as well as in `point_watch`, because the first
                        // agent the chat lands on is never picked: it is the one it opens with.
                        adopt_run_settings(watch, &on);
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
            // Which paired nodes this machine can drive — the sessions rail's node menu.
            if let Ok(n) = fetch::fleet_nodes().await
                && state.fleet_nodes.get_untracked().as_ref() != Some(&n)
            {
                state.fleet_nodes.set(Some(n));
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
        // Which machine these are about, read *tracked* (`docs/fleet.md` §13). Every agent path
        // below is rewritten by `fetch::routed` as the `Sub` is built, so picking a node in the
        // rail has to rebuild them — otherwise the socket would go on reporting the machine that
        // was chosen when this effect last ran, under the new node's name.
        let _node = state.session_node.get();
        let mut subs = state::chat_subscriptions(watch);
        // The node menu's own list: which paired nodes this machine holds a password for. Local
        // and cheap — it asks no node anything — so it rides the socket with everything else.
        subs.push(live::Sub::get(
            "/api/fleet/nodes",
            move |n: adi_webapp_api::types::FleetNodes| {
                if state.fleet_nodes.get_untracked().as_ref() != Some(&n) {
                    state.fleet_nodes.set(Some(n));
                }
            },
        ));
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
                    adopt_run_settings(watch, &on);
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

    // The mark and the menu behind it, in place of the bar these two screens used to wear.
    // Built out here so it is one object across the switch between them: dragging it on the
    // wizard and finding it moved on the chat is the whole point of a position that persists.
    let launcher = Launcher::new();

    view! {
        {move || {
            // Three doors, decided by what we actually know. Reads only `meta`/`reconfiguring`,
            // so a poll never rebuilds the tree.
            //   * `/api/meta` hasn't answered yet ⇒ the wordmark splash. We can't tell a first
            //     run from a set-up stack, so we say nothing rather than guess "welcome".
            //   * the agent exists (and we're not mid-reconfigure) ⇒ the chat.
            //   * no agent, or reconfiguring ⇒ the wizard.
            let Some(m) = meta.get() else {
                return boot_splash();
            };
            if m.agent.is_some() && !onb.reconfiguring.get() {
                view! {
                    <div class="adi-chome-root">{chat_home_view(state, watch, launcher)}</div>
                }
                .into_any()
            } else {
                view! {
                    <div class="adi-onb">
                        <main class="adi-onb__body">
                            <div class="adi-onb__panel">{onboarding_view(state, onb, m)}</div>
                        </main>
                        // The wizard has no rail to dock the mark at the head of, so here it
                        // is the corner one. The chat draws its own (`launcher::brand`).
                        {launcher::floating(launcher)}
                    </div>
                }
                .into_any()
            }
        }}

        // Over whichever of those is up, rather than inside any one of them: the screen under
        // the menu swaps between the chat and the wizard, and a menu that closed itself every
        // time that happened would be one nobody could use during a reconfigure. Held back
        // until `/api/meta` answers, so the boot splash stays the single wordmark it was drawn
        // as and the menu never opens onto rows about an agent we do not know exists.
        {move || {
            meta.get().is_some().then(|| {
                view! {
                    {launcher::overlay(launcher, move || {
                        root_actions(state, watch, onb, updates, can_install)
                    })}
                    // *What's new*: the version pill used to carry this dialog, and the menu
                    // row that replaced the pill only opens it. So it is mounted here, where
                    // nothing rebuilds it mid-read.
                    {update::offer_dialog(updates)}
                }
            })
        }}
    }
}

/// What the two root screens add to the menu: the rows that act on the conversation in front of
/// you, which the control panel has no equivalent of. Everything else they offer is
/// [`menu::rows`] — the same list the panel's own menu is built from, so the two can never come
/// to disagree about what this app can do.
///
/// Rebuilt on every draw (see [`launcher::overlay`]), which is what lets it be honest about
/// the moment. Reading signals here is therefore deliberate — it is what makes the menu track
/// them.
fn root_actions(
    state: State,
    watch: AgentsWatch,
    onb: OnboardingForm,
    updates: update::UpdateWatch,
    can_install: RwSignal<bool>,
) -> Vec<Action> {
    // The wizard is a screen with no agent behind it (or one being replaced), so the rows that
    // act on a conversation would act on nothing. Everything below that line is true either way.
    let chatting = state.meta.get().is_some_and(|m| m.agent.is_some()) && !onb.reconfiguring.get();
    let mut rows = Vec::new();

    if chatting {
        // What clicking the wordmark used to do. It was the bar's one navigation and it had
        // nowhere else to go, so it leads the menu.
        rows.push(Action::new(
            "New chat",
            "Back to the start, nothing selected",
            icons::Icon::Spark,
            move || reset_chat_home(state, watch),
        ));
        rows.push(Action::new(
            "Reconfigure agent",
            "Change its model, prompt or backend",
            icons::Icon::Agent,
            move || {
                if let Some(m) = state.meta.get_untracked() {
                    start_onb_reconfigure(onb, &m);
                }
            },
        ));
    }

    rows.extend(menu::rows(menu::Shell::Root, state, updates, can_install));
    rows
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
                // Set outright, and this embed deliberately never calls `adopt_run_settings`: the
                // directory here is not a preference somebody left on, it is which dashboard this
                // chat is *for*, and restoring a saved one over it would edit the wrong files.
                watch.run_dir.set(board.dir.clone());
                watch.context_prefix.set(format!(
                    "[Context: you are editing dashboard {id}, and this chat already runs in its \
                     directory — UI panels in frontend/modules/*.ts, endpoints in \
                     backend/routes/*.ts. Edit those .ts files by relative path; the dashboard \
                     hot-reloads.]"
                ));
            }
        });
        receive_picks(watch, dashboard.clone());
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
                    <span class="adi-embed__ctx" title=ctx_label.clone()>
                        <Icon icon=Lucide::LayoutDashboard size=IconSize::Sm/>
                        <span class="adi-mono">
                            {ctx_label.chars().take(8).collect::<String>()}
                        </span>
                    </span>
                })}
            </header>
            <div class="adi-embed__body">
                {move || live_view(state, watch)}
            </div>
        </div>
    }
}

/// The message a dashboard's element picker posts into this frame.
const PICK_MESSAGE: &str = "adi.dashboard.pick";

/// What this frame posts back once it is listening, so the dashboard knows when a pick will
/// actually arrive. See [`receive_picks`].
const READY_MESSAGE: &str = "adi.dashboard.ready";

/// Take elements picked in the dashboard that frames this embed and drop them into whichever
/// composer is on screen.
///
/// Prefilled, never sent. A pick is a *reference* — this element, in this file — and what to do
/// about it is still typed underneath by hand. That is also what keeps the listener modest about
/// where messages come from: the worst an unexpected one achieves is text in a box a person is
/// already reading. It must still carry the right message type and name the dashboard this embed
/// was opened for. The sender's origin is deliberately not pinned, because there isn't one to name:
/// the same panel is framed from `nosh.adi` on this machine and from `nosh.laptop-b.n.adi` when the
/// dashboard is viewed over the mesh.
fn receive_picks(watch: AgentsWatch, dashboard: String) {
    let Some(window) = web_sys::window() else {
        return;
    };
    let announce = dashboard.clone();
    let on_message =
        Closure::<dyn Fn(web_sys::MessageEvent)>::new(move |ev: web_sys::MessageEvent| {
            let data = ev.data();
            let field = |key: &str| {
                js_sys::Reflect::get(&data, &wasm_bindgen::JsValue::from_str(key))
                    .ok()
                    .and_then(|v| v.as_string())
            };
            if field("type").as_deref() != Some(PICK_MESSAGE)
                || field("dashboard").as_deref() != Some(dashboard.as_str())
            {
                return;
            }
            let Some(text) = field("text") else {
                return;
            };
            // The reply box exists only under an open answerable conversation; anywhere else the
            // composer on screen is the one that starts a new one.
            let composer =
                if watch.run_id.get_untracked().is_some() && watch.answerable.get_untracked() {
                    watch.reply
                } else {
                    watch.input
                };
            // Appended, so picking a second element adds to the request being written rather than
            // throwing away the first — pointing at two things and asking to align them is the
            // ordinary case, not an edge one.
            composer.update(|held| {
                if !held.is_empty() && !held.ends_with('\n') {
                    held.push('\n');
                }
                held.push_str(&text);
            });
        });
    let _ = window.add_event_listener_with_callback("message", on_message.as_ref().unchecked_ref());
    on_message.forget();

    // Now tell the page that framed this embed that picks will be received, because it cannot
    // work that out for itself. The frame's `load` fires when its *document* is done, which is
    // well before the wasm behind it has booted and run the line above — and a pick posted into
    // that gap is delivered to nobody and never redelivered. The dashboard holds its picks until
    // this lands.
    //
    // Sent to any origin: it carries nothing that isn't already known to the page it is going to
    // (which dashboard it framed), and this embed cannot name that page's origin — the same panel
    // is framed from `nosh.adi` here and from `nosh.laptop-b.n.adi` over the mesh.
    if let Ok(Some(parent)) = window.parent() {
        let ready = js_sys::Object::new();
        let set = |key: &str, value: &str| {
            let _ = js_sys::Reflect::set(
                &ready,
                &wasm_bindgen::JsValue::from_str(key),
                &wasm_bindgen::JsValue::from_str(value),
            );
        };
        set("type", READY_MESSAGE);
        set("dashboard", &announce);
        let _ = parent.post_message(&ready, "*");
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
    let marketplace = RwSignal::new(None::<adi_webapp_api::types::MarketplaceState>);
    let workspaces = RwSignal::new(None::<WorkspacesState>);
    // The id of the project whose detail page is open ("" when not on one). Drives detail
    // loads so navigating from one project to another (route stays ProjectDetail) still refreshes.
    let current_project = RwSignal::new(project_id_from_path(&current_path()).unwrap_or_default());
    // Which section of that project is showing; the bare project path is its overview.
    let current_section = RwSignal::new(project_section_from_path(&current_path()));
    // The agent whose editor is open ("" on /agents/new, and when no editor is open).
    let current_agent =
        RwSignal::new(routing::agent_form_from_path(&current_path()).unwrap_or_default());
    // True once the browser will let us offer "install as an app" (see [`pwa`]).
    let can_install = pwa::installable();
    // The version pill's watcher — one per mounted app, since it owns a timer.
    let updates = update::watch();
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
        current_agent,
        all_chats,
        tools,
        secrets,
        db,
        meta,
        triggers,
        hive,
        dashboards,
        marketplace,
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
        session_filter: RwSignal::new(SessionFilter::default()),
        session_filter_menu: RwSignal::new(None),
        // The workbench shell has no sessions rail and so no node menu: it is always about this
        // machine, and `None` here is what keeps `fetch::routed` pointed at it.
        session_node: RwSignal::new(None),
        session_node_menu: RwSignal::new(None),
        fleet_nodes: RwSignal::new(None),
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

    let marketplace_form = MarketplaceForm::new();
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

    // The Facts page's transaction queue, base filter and acting-as identity. Page-local for
    // the same reason as the two above: nothing here belongs on the shell's 4s poll.
    let facts = FactsConsole::new();

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

    // Whether the standing "ask adi-agent" line has been dismissed on this browser. Seeded from
    // storage rather than defaulting to shown, so a reload doesn't undo the dismissal.
    let advice_hidden = RwSignal::new(ui::advice_hidden());

    // The active page, derived from the URL path. Unknown paths (including `/`) resolve to
    // Projects; canonicalize the address bar so a refresh lands on the same page.
    let route = RwSignal::new(Route::from_path(&current_path()));
    // Canonicalize the address bar, except where the path carries data `path()` cannot
    // reproduce — a project's id, or a store file's path. Canonicalizing those would rewrite
    // `/files/<path>` to `/files` and lose the file before it is ever read.
    if !matches!(
        route.get_untracked(),
        Route::ProjectDetail | Route::StoreFile | Route::AgentDetail
    ) && current_path() != route.get_untracked().path()
    {
        replace_state(route.get_untracked().path());
    }
    // Arrived here by pressing "Pair new device" on the root screen? Then raise the QR, rather
    // than landing beside the button and leaving it to be found (see [`menu::consume_pair_intent`]).
    menu::consume_pair_intent(state, fleet_form, route);

    // The same corner mark the setup wizard wears, and the same rows behind it. Without it, ⌘K
    // would stop answering on the very page it was used to reach.
    let launcher = Launcher::workbench();

    // The explorer navigates: a node's id encodes its destination. Guarded on `Some`, so the
    // initial (empty) selection never navigates on load.
    Effect::new(move |_| {
        let Some(id) = explorer.selected.get() else {
            return;
        };
        match node_target(&id) {
            Some(Nav::Global(target)) => routing::go_global(state, route, target),
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
        current_agent.set(routing::agent_form_from_path(&path).unwrap_or_default());
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
                | Route::AgentDetail
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
        // Same rule for pairing: the invite minted here, the one pasted in to be spent, and the
        // password that spending it bought are all bearer secrets, and the screen they were drawn
        // on is the only place any of them was meant to exist.
        if !matches!(route.get(), Route::Fleet) {
            fleet_form.clear_pairing();
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

    // Fill the agent editor when the page is *arrived at* rather than clicked into — a deep link,
    // a refresh, or Back onto `/agents/<name>/edit`. Clicking Edit has already loaded the form, so
    // the guard on `editing` makes this a no-op there, and it also stops the 4s refresh of
    // `state.agents` from writing over an edit in progress.
    Effect::new(move |_| {
        if !matches!(route.get(), Route::AgentDetail) {
            return;
        }
        let name = current_agent.get();
        if name.is_empty() || agents_form.editing.get_untracked().as_deref() == Some(name.as_str())
        {
            return;
        }
        if let Some(a) = state
            .agents
            .get()
            .and_then(|s| s.agents.into_iter().find(|a| a.name == name))
        {
            load_agent_into_form(agents_form, &a);
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
        // The frame's lid: identity on the left, where you are next to it, and the ways out
        // on the right (design/examples/setup-agents-fleet.html, `.bar`).
        <header class="adi-titlebar">
            // The mark is the way home: `/` is the chat, and the control panel is the room
            // you stepped into from it.
            <a class="adi-logo" href="/" title="Home">
                <Mark/>
                "adi"
            </a>
            {move || crumbs(route.get(), state.current_project.get())}
            <span class="adi-spacer"></span>
            // The way back out of the control panel. A plain link, since `/` is a different
            // document than the workbench and not an SPA route.
            <a class="adi-btn adi-btn--link" href="/" title="Back to the simple chat view">
                <Icon icon=Lucide::ArrowLeft size=IconSize::Sm/>
                "simple"
            </a>
            // What this machine is on, and the way to the next version — the same control
            // the root screen's menu carries, so the answer is in the same place whichever
            // of the two documents you are in. The update button, when there is one, is this
            // screen's one orange.
            {update::version_pill(updates)}
            {move || can_install.get().then(|| view! {
                <Button
                    size=ButtonSize::Small
                    variant=ButtonVariant::Ghost
                    icon=icons::Icon::Download.lucide()
                    attr:title="Install adi as an app in its own window"
                    on:click=move |_| pwa::install()
                >
                    "Install"
                </Button>
            })}
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
                        on:click=move |ev| spa_click(&ev, route, Route::Projects)>
                        <Icon icon=Lucide::Plus/>
                    </a>
                </div>
                <div class="adi-explorer__body">
                    {move || explorer_tree(state, explorer, route)}
                </div>
            </aside>

            <main class="adi-main"
                class:adi-main--flush=move || matches!(route.get(), Route::StoreFile)>
                <div class="adi-container">
                    {move || agent_advice(advice_hidden, route.get())}

                    {move || match route.get() {
                        // These pages render their own headings — no generic page title.
                        // StoreFile is a full-bleed editor: its head carries the file path.
                        // The agent editor's head names the agent and links back to the list;
                        // the agents list's carries its counts and its one action.
                        Route::PortsManager
                        | Route::ProjectDetail
                        | Route::StoreFile
                        | Route::Agents
                        | Route::AgentDetail => None,
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
                        Route::Agents => agents_view(state, agents_form, agents_watch, agents_sim, route),
                        Route::AgentDetail => agent_detail_view(state, agents_form, route),
                        Route::Tools => tools_view(state, tools_form, tool_editor, tool_run),
                        Route::Secrets => secrets_view(state, secrets_form),
                        Route::Knowledge => knowledge_view(state, knowledge),
                        Route::Facts => facts_view(facts),
                        Route::Database => database_view(state, db_console),
                        Route::Triggers => triggers_view(state, triggers_form, triggers_log),
                        Route::Dashboards => dashboards_view(state, dashboards_form),
                        Route::Marketplace => marketplace_view(state, marketplace_form),
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

        // Outside the workbench rather than inside it: the mark is fixed to the viewport and
        // belongs to no column of the frame. *What's new* is not mounted beside it here — the
        // version pill in the bar above already carries that dialog, and a second copy would
        // open two.
        {launcher::floating(launcher)}
        {launcher::overlay(launcher, move || {
            menu::rows(
                menu::Shell::Panel { route, fleet: fleet_form },
                state,
                updates,
                can_install,
            )
        })}
    }
}

/// Where you are, read left to right from the mark: `/ Settings / Fleet`, `/ Projects / api`.
/// Sans, in the bar's own grey, with the current segment in ink — a location is not a machine
/// string. Every segment but the last is somewhere you can go back to.
fn crumbs(route: Route, project: String) -> AnyView {
    let mut path: Vec<(String, Option<String>)> = Vec::new();
    match route {
        Route::Hive | Route::PortsManager | Route::Mesh | Route::Fleet => {
            path.push(("Settings".to_string(), None));
        }
        Route::ProjectDetail if !project.is_empty() => {
            path.push((
                "Projects".to_string(),
                Some(Route::Projects.path().to_string()),
            ));
            path.push((project, None));
            return crumb_nav(path);
        }
        _ => {}
    }
    path.push((route.title().to_string(), None));
    crumb_nav(path)
}

fn crumb_nav(path: Vec<(String, Option<String>)>) -> AnyView {
    let last = path.len().saturating_sub(1);
    view! {
        <nav class="adi-crumbs" aria-label="Breadcrumb">
            {path
                .into_iter()
                .enumerate()
                .map(|(i, (label, href))| view! {
                    <span class="adi-crumbs__sep" aria-hidden="true">"/"</span>
                    {match href {
                        Some(href) if i != last => view! { <a href=href>{label}</a> }.into_any(),
                        _ if i == last => {
                            view! { <span class="adi-crumbs__here" aria-current="page">{label}</span> }
                                .into_any()
                        }
                        _ => view! { <span>{label}</span> }.into_any(),
                    }}
                })
                .collect::<Vec<_>>()}
        </nav>
    }
    .into_any()
}

/// The workbench's standing advice, one line above every page: the ordinary way to change this
/// machine is to ask the agent, and these panels are the careful case.
///
/// It is worth saying on the panels themselves because that is where somebody is when they are
/// about to do it the long way. Dismissible, and the dismissal sticks (see [`ui::hide_advice`]) —
/// a recommendation that cannot be turned off is a nag.
///
/// Absent on the full-bleed editor, whose container gives its whole height to one child and has
/// no room for a line above it.
fn agent_advice(hidden: RwSignal<bool>, route: Route) -> Option<AnyView> {
    if hidden.get() || matches!(route, Route::StoreFile) {
        return None;
    }
    Some(
        view! {
            <div class="adi-advice">
                <span class="adi-advice__text">
                    <b>"Recommended:"</b>
                    " let adi-agent manage this — say what you want in chat and it sets up
                     projects, services, tools and secrets the way this store expects. These
                     panels are for the careful case: seeing exactly what is there, or changing
                     one thing precisely."
                </span>
                <a class="adi-advice__link" href="/" title="Ask the agent instead">"Open chat"</a>
                <button class="adi-advice__hide" type="button" aria-label="Dismiss"
                    title="Hide this — it stays hidden on this browser"
                    on:click=move |_| ui::hide_advice(hidden)>
                    <Icon icon=Lucide::X size=IconSize::Sm/>
                </button>
            </div>
        }
        .into_any(),
    )
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
            Route::Facts,
            Route::Database,
            Route::Triggers,
            Route::Dashboards,
            Route::Marketplace,
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
                .icon(scope_icon(label).lucide()),
        );
        for r in routes {
            nodes.push(
                TreeNode::new(format!("route:{}", r.path()), 1, r.title())
                    .icon(icons::route_icon(*r).lucide()),
            );
        }
    }

    let Some(projects) = state.projects.get() else {
        return view! { <Tree nodes=nodes state=explorer empty="Loading…"/> }.into_any();
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
                .icon(icons::Icon::Folder.lucide())
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
                .icon(icons::section_icon(section).lucide()),
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
    view! { <Tree nodes=nodes state=explorer selected=selected empty="Nothing here yet."/> }
        .into_any()
}
