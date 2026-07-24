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
mod markdown;
mod pages;
mod routing;
mod state;
mod store_browser;
mod tree;
mod ui;

use adi_webapp_api::types::{
    AgentBackendOption, AgentsState, DashboardsState, Health, HiveState, MeshState, MetaState,
    PortsState, ProjectDetail, ProjectsState, SaveAgent, SecretsState, TasksState, ToolsState,
    TriggersState, UsedPorts, WorkspacesState,
};
use gloo_timers::callback::Interval;
use leptos::prelude::*;
use wasm_bindgen::JsCast;
use wasm_bindgen::closure::Closure;
use wasm_bindgen_futures::spawn_local;

use pages::{
    agents_view, chat_home_view, dashboards_view, hive_view, live_view, load_dir, load_store_file,
    mesh_view, meta_view, poll_hook_log, poll_term, poll_trigger_log, poll_watch,
    ports_manager_view, project_detail_view, projects_view, secrets_view, store_file_view,
    tasks_view, tools_view, triggers_view,
};
use routing::{
    ProjectSection, Route, current_path, open_project_section, project_id_from_path,
    project_section_from_path, query_param, replace_state, spa_click,
};
use state::{
    AgentCodeEditor, AgentsForm, AgentsWatch, DashboardsForm, FilesState, Flash, Form, HookLogView,
    MeshForm, MetaForm, ProjectsForm, SecretsForm, State, Status, TasksForm, TermWatch, ToolEditor,
    ToolRunView, ToolsForm, TriggersForm, TriggersLogView, load,
};
use highlight::Lang;
use ui::{apply_saved_theme, code_editor, fmt_uptime, toggle_theme};

fn main() {
    console_error_panic_hook::set_once();
    apply_saved_theme();
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
}

/// The onboarding steps, in order. Only step 1 is interactive today; the rest scaffold the
/// wizard so "step 1 of N" reads true and there is somewhere to grow into.
const ONBOARDING_STEPS: [&str; 2] = ["Set up your primary agent", "You're ready"];

/// One "do you have…?" branch in the runtime picker: the situation, a note on the shared
/// credential, and the runtimes that fit it. Each option is an `(id, how)` pair — the backend id
/// (matched against the server form spec for its label) and a short note on how that runtime runs.
struct RuntimeGuide {
    question: &'static str,
    note: &'static str,
    options: &'static [(&'static str, &'static str)],
}

/// The "help me to choose?" guidance: a short "do you have…?" checklist that maps what a user
/// already has (a subscription, an API key) onto one or more runtimes. Every id matches a backend
/// in the server's form spec, so picking one drops straight into the runtime select. A Claude
/// login powers both the live-terminal CLI and the headless SDK — same credentials — so it lists
/// both.
const RUNTIME_GUIDE: [RuntimeGuide; 4] = [
    RuntimeGuide {
        question: "I have a Claude subscription (Pro / Max), or the Claude CLI already logged in",
        note: "Uses your existing Claude login — no API key needed. Same credentials either way:",
        options: &[
            ("pty:claude", "Claude Code in a live terminal session"),
            ("harness:claude-sdk", "the Claude SDK, headless, on the same login"),
        ],
    },
    RuntimeGuide {
        question: "I have a ChatGPT / Codex subscription",
        note: "Uses your Codex login.",
        options: &[("pty:codex", "the Codex CLI in a live terminal session")],
    },
    RuntimeGuide {
        question: "I have an Anthropic API key",
        note: "Talks to the Claude API directly with your key — no CLI login required.",
        options: &[("harness:claude-sdk", "the Claude SDK, headless")],
    },
    RuntimeGuide {
        question: "I have another provider's API key (OpenAI, Gemini, Kimi, …)",
        note: "ADI's built-in agent loop speaks to many providers with your key.",
        options: &[("harness:adi", "the ADI agent loop")],
    },
];

/// The root (`/`). Before the root agent exists it's a guided onboarding wizard (welcome, stepper,
/// setup form) behind a slim `adi. · extended →` bar. **Once the agent exists the app becomes the
/// chat**: its sessions on the left, the conversation in the centre, dashboards on the right (see
/// [`chat_home_view`]). The bar's "reconfigure" returns to the setup form to change the agent.
#[component]
fn Home() -> impl IntoView {
    let state = State::fresh();
    let watch = AgentsWatch::new();
    // The chat and the wizard share one meta signal — it decides which of the two shows.
    let meta = state.meta;
    let backend = RwSignal::new(String::new());
    let prompt = RwSignal::new(String::new());
    let busy = RwSignal::new(false);
    let error = RwSignal::new(None::<String>);
    // True while editing an existing agent, so the wizard's setup form shows instead of the chat.
    let reconfiguring = RwSignal::new(false);
    // Whether the "help me to choose?" runtime-picker modal is open.
    let show_help = RwSignal::new(false);
    // The system prompt is advanced and seeded from a sensible default, so it's collapsed by
    // default; this reveals the in-app editor for it.
    let show_prompt = RwSignal::new(false);

    // Load the meta state once, seeding the form from the server's default prompt and first
    // backend when the agent hasn't been created yet.
    spawn_local(async move {
        if let Ok(m) = fetch::meta().await {
            if m.agent.is_none() {
                if prompt.get_untracked().is_empty() {
                    prompt.set(m.default_prompt.clone());
                }
                if backend.get_untracked().is_empty()
                    && let Some(first) = m.form.backends.first()
                {
                    backend.set(first.id.clone());
                }
            }
            meta.set(Some(m));
        }
    });

    // Keep the chat pointed at the root agent and the dashboards rail fresh: detect the agent's
    // executor (pty ⇒ interactive) so the live view picks the right shape, point the watch at it the
    // first time it appears, and refresh the dashboards list. Runs on load, on creation, and on a poll.
    let refresh = move || {
        spawn_local(async move {
            if let Ok(a) = fetch::agents().await {
                if let Some(d) = a.agents.iter().find(|d| d.name == "adi-agent") {
                    let interactive = d.executor == "pty";
                    if watch.interactive.get_untracked() != interactive {
                        watch.interactive.set(interactive);
                    }
                    if watch.name.get_untracked().is_none() {
                        watch.name.set(Some("adi-agent".to_string()));
                        poll_watch(watch);
                    }
                }
                state.agents.set(Some(a));
            }
            if let Ok(dd) = fetch::dashboards().await
                && state.dashboards.get_untracked().as_ref() != Some(&dd)
            {
                state.dashboards.set(Some(dd));
            }
        });
    };

    // Wire up the chat as soon as the agent exists — on load, or right after the setup form creates it.
    Effect::new(move |_| {
        if meta.get().is_some_and(|m| m.agent.is_some()) {
            refresh();
        }
    });
    Interval::new(1_000, move || poll_watch(watch)).forget();
    Interval::new(4_000, move || refresh()).forget();

    // Bar "reconfigure": seed the setup form from the stored agent, then flip into reconfigure mode
    // so the centred wizard shows in place of the chat.
    let start_reconfigure = move || {
        if let Some(agent) = meta.get_untracked().and_then(|m| m.agent) {
            backend.set(agent.backend.clone());
            prompt.set(arg_text(&agent.arguments, "system_prompt"));
            reconfiguring.set(true);
        }
    };

    move || {
        // The chat takes over once the agent exists and we're not mid-reconfigure; otherwise the
        // onboarding wizard. Reads only `meta`/`reconfiguring`, so a poll never rebuilds the tree.
        if meta.get().is_some_and(|m| m.agent.is_some()) && !reconfiguring.get() {
            view! {
                <div class="adi-chome-root">
                    <header class="adi-onb__bar">
                        <span class="adi-onb__brand">"adi"<span class="adi-onb__dot">"."</span></span>
                        <span class="adi-spacer"></span>
                        <button class="adi-onb__ext" type="button"
                            on:click=move |_| start_reconfigure()>"reconfigure agent"</button>
                        <a class="adi-onb__ext" href="/extended">
                            <span>"extended"</span>
                            <span class="adi-onb__ext-arrow">"\u{2192}"</span>
                        </a>
                    </header>
                    {chat_home_view(state, watch)}
                </div>
            }
            .into_any()
        } else {
            view! {
                <div class="adi-onb">
                    <header class="adi-onb__bar">
                        <span class="adi-onb__brand">"adi"<span class="adi-onb__dot">"."</span></span>
                        <span class="adi-spacer"></span>
                        <a class="adi-onb__ext" href="/extended">
                            <span>"extended"</span>
                            <span class="adi-onb__ext-arrow">"\u{2192}"</span>
                        </a>
                    </header>
                    <main class="adi-onb__body">
                        <div class="adi-onb__panel">
                            <div class="adi-onb__intro">
                                <h1 class="adi-onb__welcome">
                                    "Welcome to adi"<span class="adi-onb__dot">"."</span>
                                </h1>
                                <p class="adi-onb__sub">"Let\u{2019}s set up your primary agent."</p>
                            </div>
                            <ol class="adi-onb__steps">{onb_steps(meta, reconfiguring)}</ol>
                            {move || match meta.get() {
                                None => view! {
                                    <div class="adi-onb__card">
                                        <div class="adi-onb__loading">"Loading…"</div>
                                    </div>
                                }
                                .into_any(),
                                Some(m) => onb_setup_form(
                                    meta, backend, prompt, busy, error, reconfiguring, show_help,
                                    show_prompt, m,
                                ),
                            }}
                        </div>
                    </main>
                </div>
            }
            .into_any()
        }
    }
}

/// The stepper row: one node per onboarding step. Step 1 is `done` once `adi-agent` exists
/// (and we aren't mid-reconfigure), otherwise `active`; later steps are `upcoming`.
fn onb_steps(meta: RwSignal<Option<MetaState>>, reconfiguring: RwSignal<bool>) -> impl IntoView {
    let done_first =
        move || meta.get().is_some_and(|m| m.agent.is_some()) && !reconfiguring.get();
    ONBOARDING_STEPS
        .iter()
        .enumerate()
        .map(|(i, label)| {
            let label = (*label).to_string();
            let num = (i + 1).to_string();
            let is_first = i == 0;
            let state = move || {
                if is_first {
                    if done_first() { "done" } else { "active" }
                } else {
                    "upcoming"
                }
            };
            let num_view = move || {
                if is_first && done_first() {
                    "\u{2713}".to_string()
                } else {
                    num.clone()
                }
            };
            view! {
                <li class="adi-onb__step" data-state=state>
                    <span class="adi-onb__step-num">{num_view}</span>
                    <span class="adi-onb__step-label">{label}</span>
                </li>
            }
        })
        .collect::<Vec<_>>()
}

/// Step 1's setup form — root-runtime picker + prefilled system prompt. Doubles as create (no
/// agent yet) and reconfigure (an agent exists and Cancel returns to the summary). The runtime
/// select carries a "help me to choose?" link that opens the [`onb_help_modal`] picker.
fn onb_setup_form(
    meta: RwSignal<Option<MetaState>>,
    backend: RwSignal<String>,
    prompt: RwSignal<String>,
    busy: RwSignal<bool>,
    error: RwSignal<Option<String>>,
    reconfiguring: RwSignal<bool>,
    show_help: RwSignal<bool>,
    show_prompt: RwSignal<bool>,
    m: MetaState,
) -> AnyView {
    let creating = m.agent.is_none();
    let backends = m.form.backends.clone();
    let backends_modal = m.form.backends.clone();
    view! {
        <div class="adi-onb__card">
            <span class="adi-onb__eyebrow">"Step 1"</span>
            <h2 class="adi-onb__title">"Set up your primary agent"</h2>
            <p class="adi-onb__desc">
                <strong>"adi-agent"</strong>
                " is your environment's root agent — a meta-agent that helps you set up and
                 operate this ADI stack. Pick the runtime it runs on and give it a system prompt;
                 you can change all of it later."
            </p>
            <form class="adi-onb__form" on:submit=move |ev| {
                ev.prevent_default();
                submit_onb_agent(meta, backend, prompt, busy, error, reconfiguring);
            }>
                <div class="adi-field">
                    <div class="adi-onb__field-head">
                        <label class="adi-field__label" for="onb-backend">
                            "Select your root agent runtime"
                        </label>
                        <button class="adi-onb__help-link" type="button"
                            on:click=move |_| show_help.set(true)>"help me to choose?"</button>
                    </div>
                    <select class="adi-input" id="onb-backend"
                        prop:value=move || backend.get()
                        on:change=move |ev| backend.set(event_target_value(&ev))>
                        <option value="">"— pick a runtime —"</option>
                        {backends.into_iter().map(|b| view! {
                            <option value=b.id>{b.label}</option>
                        }).collect::<Vec<_>>()}
                    </select>
                </div>
                <div class="adi-field">
                    <button class="adi-onb__disclosure" type="button"
                        aria-expanded=move || show_prompt.get().to_string()
                        on:click=move |_| show_prompt.update(|v| *v = !*v)>
                        <span class="adi-onb__disclosure-caret"
                            class:is-open=move || show_prompt.get()>"\u{25b8}"</span>
                        <span class="adi-onb__disclosure-label">"System prompt"</span>
                        <span class="adi-onb__disclosure-hint">"optional \u{00b7} advanced"</span>
                    </button>
                    {move || show_prompt.get().then(|| view! {
                        <div class="adi-onb__prompt">
                            <p class="adi-onb__hint">
                                "Seeded with a default that orients the agent in your ADI stack and
                                 points it at the guides in "
                                <code class="adi-onb__code">"~/.adi/mono/guides"</code>
                                " (dashboards, tasks, tools, …). Edit freely — you can change it
                                 later."
                            </p>
                            {code_editor(|| Lang::Md, prompt, "adi-code--form", "onb-prompt")}
                        </div>
                    })}
                </div>

                {move || error.get().map(|e| view! { <p class="adi-onb__error">{e}</p> })}

                <div class="adi-onb__actions">
                    {(!creating).then(|| view! {
                        <button class="adi-btn adi-btn--link" type="button"
                            on:click=move |_| reconfiguring.set(false)>"Cancel"</button>
                    })}
                    <span class="adi-spacer"></span>
                    <button class="adi-btn adi-btn--primary adi-onb__submit" type="submit"
                        prop:disabled=move || busy.get()>
                        {move || match (busy.get(), creating) {
                            (true, _) => "Saving…",
                            (false, true) => "Create adi-agent",
                            (false, false) => "Save changes",
                        }}
                    </button>
                </div>
            </form>
        </div>

        {move || show_help.get()
            .then(|| onb_help_modal(show_help, backend, backends_modal.clone()))}
    }
    .into_any()
}

/// The "help me to choose?" modal: a "do you have…?" checklist (from [`RUNTIME_GUIDE`]) that
/// recommends a runtime for what the user already has and, on "Use this", writes it into the
/// select and closes. Clicking the scrim or the ✕ dismisses it. Only rendered while open.
fn onb_help_modal(
    show_help: RwSignal<bool>,
    backend: RwSignal<String>,
    backends: Vec<AgentBackendOption>,
) -> AnyView {
    let rows = RUNTIME_GUIDE
        .iter()
        .map(|guide| {
            let opts = guide
                .options
                .iter()
                .map(|(id, how)| {
                    let id = (*id).to_string();
                    // Show the server's own label for the runtime when it offers one, so the modal
                    // never drifts from the select; fall back to the raw id if the list lacks it.
                    let label = backends
                        .iter()
                        .find(|b| b.id == id)
                        .map_or_else(|| id.clone(), |b| b.label.clone());
                    let pick = id.clone();
                    view! {
                        <div class="adi-help__opt">
                            <div class="adi-help__opt-main">
                                <code class="adi-onb__code">{label}</code>
                                <span class="adi-help__how">{(*how).to_string()}</span>
                            </div>
                            <span class="adi-spacer"></span>
                            <button class="adi-btn adi-btn--ghost adi-help__use" type="button"
                                on:click=move |_| {
                                    backend.set(pick.clone());
                                    show_help.set(false);
                                }>"Use this"</button>
                        </div>
                    }
                })
                .collect::<Vec<_>>();
            view! {
                <li class="adi-help__row">
                    <p class="adi-help__q">{guide.question}</p>
                    <p class="adi-help__note">{guide.note}</p>
                    <div class="adi-help__opts">{opts}</div>
                </li>
            }
        })
        .collect::<Vec<_>>();

    view! {
        <div class="adi-help" role="dialog" aria-modal="true" aria-label="Choose a runtime"
            on:click=move |_| show_help.set(false)>
            <div class="adi-help__panel" on:click=|ev| ev.stop_propagation()>
                <header class="adi-help__head">
                    <h3 class="adi-help__title">"Which runtime should I pick?"</h3>
                    <button class="adi-btn adi-btn--icon-sm" type="button" aria-label="Close"
                        on:click=move |_| show_help.set(false)>"\u{00d7}"</button>
                </header>
                <p class="adi-help__intro">
                    "Tell us what you already have — we\u{2019}ll point you at a matching runtime.
                     You can change it any time."
                </p>
                <ul class="adi-help__list">{rows}</ul>
                <p class="adi-help__foot">
                    "Still unsure? Start with " <strong>"Claude CLI"</strong>
                    " — every runtime is swappable from Extended \u{2192} Meta later."
                </p>
            </div>
        </div>
    }
    .into_any()
}

/// Save the setup form as the `adi-agent` definition (create or update), preserving any other
/// arguments (model, tools, …) the agent already carries. Refreshes `/api/meta` on success.
fn submit_onb_agent(
    meta: RwSignal<Option<MetaState>>,
    backend: RwSignal<String>,
    prompt: RwSignal<String>,
    busy: RwSignal<bool>,
    error: RwSignal<Option<String>>,
    reconfiguring: RwSignal<bool>,
) {
    let chosen = backend.get_untracked().trim().to_string();
    if chosen.is_empty() {
        error.set(Some("Pick a backend for the agent.".to_string()));
        return;
    }
    let text = prompt.get_untracked();
    let current = meta.get_untracked();
    let name = current
        .as_ref()
        .map_or_else(|| "adi-agent".to_string(), |m| m.name.clone());
    let mut arguments = current
        .and_then(|m| m.agent)
        .map(|a| a.arguments)
        .unwrap_or_default();
    if text.trim().is_empty() {
        arguments.remove("system_prompt");
    } else {
        arguments.insert("system_prompt".to_string(), serde_json::Value::String(text));
    }
    let body = SaveAgent {
        name,
        backend: chosen,
        arguments,
        tags: Vec::new(),
        starred: false,
        project: None,
        bin_tools: Vec::new(),
        secrets: Vec::new(),
        rename_from: None,
    };
    busy.set(true);
    error.set(None);
    spawn_local(async move {
        match fetch::save_agent(body).await {
            Ok(_) => {
                reconfiguring.set(false);
                if let Ok(m) = fetch::meta().await {
                    meta.set(Some(m));
                }
            }
            Err(e) => error.set(Some(e)),
        }
        busy.set(false);
    });
}

/// A scalar string argument as text, or empty when absent/structured.
fn arg_text(arguments: &BTreeMap<String, serde_json::Value>, name: &str) -> String {
    match arguments.get(name) {
        Some(serde_json::Value::String(s)) => s.clone(),
        _ => String::new(),
    }
}

/// The chrome-less dashboard-agent embed (`/embed/dashboard-agent?dashboard=<id>`): the one global
/// `adi-agent` chat, opened from a dashboard's launcher. It reuses the agent live view, points it at
/// `adi-agent`, and sets a context prefix so every message it sends is tagged with which dashboard
/// it was opened from — the agent then edits that dashboard's `.ts` files. Served by app.adi, so its
/// API calls are same-origin (no CORS).
#[component]
fn EmbedDashboardAgent() -> impl IntoView {
    let state = State::fresh();
    let watch = AgentsWatch::new();
    let dashboard = query_param("dashboard").unwrap_or_default();

    if !dashboard.is_empty() {
        watch.context_prefix.set(format!(
            "[Context: you are editing dashboard {dashboard}. Its files are at \
             ~/.adi/mono/dashboards/{dashboard} — UI panels in frontend/modules/*.ts, endpoints in \
             backend/routes/*.ts. Edit those .ts files; the dashboard hot-reloads.]"
        ));
    }

    // Learn whether adi-agent is interactive (pty) vs headless, point the live view at it, and poll.
    spawn_local(async move {
        if let Ok(a) = fetch::agents().await {
            let interactive = a
                .agents
                .iter()
                .find(|d| d.name == "adi-agent")
                .is_some_and(|d| d.executor == "pty");
            watch.interactive.set(interactive);
            state.agents.set(Some(a));
        }
        watch.name.set(Some("adi-agent".to_string()));
        poll_watch(watch);
    });
    Interval::new(1_000, move || poll_watch(watch)).forget();

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
    let projects = RwSignal::new(None::<ProjectsState>);
    let project_detail = RwSignal::new(None::<ProjectDetail>);
    let tasks = RwSignal::new(None::<TasksState>);
    let agents = RwSignal::new(None::<AgentsState>);
    let all_chats = RwSignal::new(None::<adi_webapp_api::types::AllAgentRuns>);
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
        all_chats,
        tools,
        secrets,
        meta,
        triggers,
        hive,
        dashboards,
        workspaces,
        files,
        store,
        row_menu: RwSignal::new(None),
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
        events: RwSignal::new(String::new()),
        trigger_on: RwSignal::new(Vec::new()),
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
        // Any page change closes an open row menu (its row is about to unmount anyway).
        state.row_menu.set(None);
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
