//! The Agents page: the list of agent definitions (docs/adi-agents.md §5), each row carrying
//! its launch and edit actions in its ⋯ menu. Run starts either an interactive pty session or a
//! headless background process; deeper orchestration is future work.
//!
//! Writing a definition — a backend (`executor:what`), a system prompt, a CLI command scope, and
//! the backend-specific params — happens in [`agent_detail_view`], on the agent's own page: New
//! agent and Edit navigate there rather than filling a form under the list. The form adapts its
//! params to the chosen backend, and for the `harness:adi` backend also to its chosen provider.

use std::collections::BTreeMap;

use adi_ui::{Icon, IconSize, Lucide, Row as TableRow, Table};
use adi_webapp_api::types::{
    AgentDto, SaveAgent, SecretDto, SecretRef, SpawnedByDto, ToolDto, created_by_is_mine,
};
use leptos::prelude::*;
use wasm_bindgen_futures::spawn_local;

use crate::fetch;
use crate::routing::{
    Route, agent_form_path, push_state, reconfigure_href, replace_state, scroll_top, spa_click,
    spa_nav,
};
use crate::state::{AgentsForm, AgentsWatch, Flash, Simulate, State, read_error};
use crate::ui::{
    Key, field_hint, flash_view, menu_item, row_actions, rows_or_status, sort_rows, updated_text,
};

/// The Agents page's columns; the trailing blank one holds the running dot and the ⋯ menu.
pub(crate) const COLS: &[&str] = &["Name", "Backend", "Model", "Project", "Tags", ""];

/// How the Agents page's list is narrowed — modelled on the chat rail's own `SessionFilter`
/// (`pages::agents::actions`), but simpler: there is no third "Starred" state here, because starring
/// is a per-row exemption from Mine rather than a narrowing of its own (see [`agent_is_mine`]).
///
/// Page state, like the rail's: not persisted, so returning to this page always opens on Mine.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub(crate) enum AgentsFilter {
    /// Every registered definition, whoever created it.
    All,
    /// Only definitions this filter counts as a person's own — see [`agent_is_mine`] for exactly
    /// which, and why it is not simply "created by a person".
    #[default]
    Mine,
}

/// Whether `a` counts as "mine" for [`AgentsFilter::Mine`] — a thin, DTO-typed wrapper over
/// [`adi_webapp_api::types::created_by_is_mine`], which is the one place this rule is actually
/// stated (see its doc comment for why an unattributed definition counts as mine here, unlike an
/// unattributed *session* in the rail's own "started by me" filter). Starring is a second, separate
/// exemption — callers that need it check `a.starred` alongside this rather than folding it in here,
/// so a reader can tell "the filter says yes" from "the star overrode it".
#[must_use]
pub(crate) fn agent_is_mine(a: &AgentDto) -> bool {
    created_by_is_mine(&a.created_by)
}

mod actions;
mod chain;
mod form;
mod simulate;

use actions::apply_agents;
pub(crate) use actions::{
    CHAT_COLS, CHAT_RUN_COLS, NEWEST_FIRST, RUN_COLS, adopt_run_settings, agent_actions,
    all_chats_view, auto_title_view, chat_home_view, live_view, open_conversation,
    open_run_answerable_untracked, poll_watch, project_run_limit_view, reset_chat_home,
    run_limit_view,
};
// The onboarding wizard renders the same fields from the same schema, so its half of the form
// lives here rather than as a second copy of these renderers.
// `field_applies` is the whole of "which fields does this runtime take", and the LLM backends
// editor asks the same question of the same schema — so it is one function, not two that agree
// until somebody adds a filter to one of them.
pub(crate) use form::{
    agent_argument_values, agent_environment_fields, agent_param_applies, agent_schema_fields,
    field_applies, load_agent_into_form, parsed_env_vars, parsed_path_dirs, parsed_prelude,
    set_agent_field_value,
};
use form::{agent_form_sections, clear_agent_form};

/// The Agents page: every agent defined on this machine, with the live view above it and the
/// per-row actions in each row's menu. Editing one, or writing a new one, opens
/// [`agent_detail_view`] on that agent's own URL — the definition form is not part of this page.
pub(crate) fn agents_view(
    state: State,
    form: AgentsForm,
    watch: AgentsWatch,
    sim: Simulate,
    route: RwSignal<Route>,
) -> AnyView {
    let agents = state.agents;
    let secs_since = state.secs_since;
    // The run cap's editor is a row that only appears when asked for: the head says the numbers,
    // and "change limit" is the one control that opens the box that sets them.
    let limit_open = RwSignal::new(false);
    // Mine by default, for the same reason the chat rail opens narrowed: a fleet grows a long tail
    // of machine-made definitions, and a page listing every one of them buries the handful a person
    // actually wrote. Page state, not persisted — see [`AgentsFilter`].
    let agents_filter = RwSignal::new(AgentsFilter::default());
    view! {
        // Above everything, when it is open: taking the model's seat is the whole screen, not a
        // panel beside the list of agents you could take it in.
        {move || simulate::simulate_view(state, sim)}

        {move || live_view(state, watch)}

        <header class="adi-bar">
            <h1 class="adi-bar__title">"Agents"</h1>
            <span class="adi-agents__meta">
                {move || agents.get().map(|a| {
                    let n = a.agents.len();
                    let count = if n == 1 { "1 agent".to_string() } else { format!("{n} agents") };
                    let running = if a.max_concurrent_runs == 0 {
                        format!("{} running", a.running_runs)
                    } else {
                        format!("{} of {} running", a.running_runs, a.max_concurrent_runs)
                    };
                    view! {
                        <b>{count}</b>" · "<b>{running}</b>" · "
                        <button class="adi-agents__limit" type="button"
                            aria-expanded=move || limit_open.get().to_string()
                            on:click=move |_| limit_open.update(|o| *o = !*o)>
                            "change limit"
                        </button>
                    }
                })}
            </span>
            {spawn_policy_view(state)}
            <span class="adi-spacer"></span>
            {move || agents_filter_toggle(agents_filter)}
            <span class="adi-updated">{move || updated_text(agents, secs_since)}</span>
            <a class="adi-btn adi-btn--primary" href=agent_form_path("")
                on:click=move |ev| if spa_nav(&ev) {
                    open_agent_editor(state, route, form, None);
                }>
                "New agent"
            </a>
        </header>

        {move || limit_open.get().then(|| view! {
            <div class="adi-agents__limit-row">{run_limit_view(state)}</div>
        })}
        <div class="adi-agents__limit-row">{auto_title_view(state)}</div>

        <div class="adi-agents__refusals-section">
            <h2 class="adi-agents__h2">"Would have been refused"</h2>
            <p class="adi-agents__intro">
                "Agent-to-agent launches on record whose target no longer matches — or never \
                 matched — the caller's own \"Can launch\". Visible in both modes; an empty list \
                 is the signal that it is safe to switch to Enforce."
            </p>
            {move || spawn_refusals_view(state)}
        </div>

        <Table state=state.tables.agents>
            {move || agent_rows(state, form, watch, sim, route, agents_filter.get())}
        </Table>
        {flash_view(state.flash)}
        // What the row menu's launch actions do. It belongs with the rows it explains.
        <p class="adi-hint">
            "Run starts a pty backend in an interactive " <code>"adi-agent-<name>"</code>
            " session you type into. A headless backend starts from a task you give it. "
            <code>"process:claude"</code>" is an independent one-shot run; "
            <code>"process:codex"</code>" and the "<code>"harness"</code>
            " backends start an answerable "<strong>"conversation"</strong>": Chat… sends a first \
             message, the agent answers, and you reply to continue the same thread. Several can run \
             at once, and each keeps its own log and transcript under "
            <code>"~/.adi/mono/sessions/{process,harness}/<agent>/"</code>
            ", browsable as history in View."
        </p>
    }
    .into_any()
}

/// One agent's editor, on a page of its own (`/agents/new`, `/agents/<name>/edit`) rather than
/// under the list: the whole schema-driven definition form — backend params, tools, knowledge,
/// secrets, run environment — for the agent named by [`State::current_agent`], or a blank one.
///
/// The form's signals are the same [`AgentsForm`] the list page used to hold, so what fills it is
/// unchanged: Edit loads the agent before navigating, and [`main`] covers the arrive-directly case.
pub(crate) fn agent_detail_view(state: State, form: AgentsForm, route: RwSignal<Route>) -> AnyView {
    let agents = state.agents;
    let flash = state.flash;
    let AgentsForm {
        name,
        backend,
        project,
        tags,
        starred,
        arguments,
        argument_values,
        editing,
        busy,
        ..
    } = form;
    // A URL naming an agent the list doesn't have: a stale link, or one deleted from under this
    // tab. Say so rather than opening a create form prefilled with nothing, which would silently
    // turn "edit" into "make a second agent".
    let missing = move || {
        let open = state.current_agent.get();
        !open.is_empty()
            && agents
                .get()
                .is_some_and(|s| !s.agents.iter().any(|a| a.name == open))
    };
    // Whether the system prompt's editor is unfolded. Held here, above the fields, so a refresh
    // of the agents list — which re-renders the fields — does not fold a prompt somebody is in
    // the middle of writing.
    let prompt_open = RwSignal::new(false);
    Effect::new(move |_| {
        if !form.system_prompt.get().is_empty() {
            prompt_open.set(true);
        }
    });
    view! {
        <div class="adi-agents__form">
            <div class="adi-agents__toprow">
                <a class="adi-agents__back" href=Route::Agents.path() title="Back to every agent"
                    on:click=move |ev| spa_click(&ev, route, Route::Agents)>
                    <Icon icon=Lucide::ArrowLeft size=IconSize::Sm/>
                    "Agents"
                </a>
                // The guided wizard, on this same agent — a real navigation into the root
                // document (see `routing::reconfigure_href`), not this page's own route change.
                // Only once the agent exists: a definition still being created has nothing for
                // the wizard to load.
                {move || editing.get().map(|n| view! {
                    <a class="adi-agents__back" href=reconfigure_href(&n)
                        title="Open the guided setup wizard on this agent">
                        "Reconfigure with the wizard"
                        <Icon icon=Lucide::ArrowRight size=IconSize::Sm/>
                    </a>
                })}
            </div>
            <h1 class="adi-agents__title">
                {move || match editing.get() {
                    Some(n) => format!("Reconfigure {n}"),
                    None => "New agent".to_string(),
                }}
            </h1>
            <p class="adi-agents__lead">
                {move || if editing.get().is_some() {
                    "Change the runtime it runs on, what it may reach, or its system prompt."
                } else {
                    "Pick how it should run and give it what that needs. Everything here can \
                     change later."
                }}
            </p>

            {move || missing().then(|| view! {
                <p class="adi-agents__missing">
                    {format!("No agent named “{}” — it may have been deleted or renamed.",
                        state.current_agent.get())}
                </p>
            })}

            // The form stays mounted either way: rebuilding it whenever the polled agents list
            // lands would drop focus mid-edit, and a form still holding a deleted agent is how
            // you put it back.
            <form class="adi-agents__body" on:submit=move |ev| {
                ev.prevent_default();
                let nm = name.get().trim().to_string();
                if nm.is_empty() {
                    flash.set(Some(Flash::err("An agent name is required.".to_string())));
                    return;
                }
                let be = backend.get();
                // An agent that lists LLM backends runs on the runtime of the one it starts on, so
                // there is nothing to pick and nothing for this form to send. Only a chainless
                // agent still has to say.
                let chained = !form.llm_rows.get().is_empty();
                if be.trim().is_empty() && !chained {
                    flash.set(Some(Flash::err(
                        "Pick an LLM backend, or a runtime to run on.".to_string(),
                    )));
                    return;
                }
                let spec = agents.get().map(|st| st.form);
                let prov = argument_values.get().get("provider").cloned().unwrap_or_default();
                // Whether each backend-conditional first-class param applies is driven by the
                // server schema (does a field of that name apply to this backend?), so rescoping a
                // field in the API also stops its value being sent for backends it no longer fits.
                let pm_applies = agent_param_applies(spec.as_ref(), &be, &prov, "permission_mode");
                let temp_applies = agent_param_applies(spec.as_ref(), &be, &prov, "temperature");
                let body = SaveAgent {
                    name: nm.clone(),
                    // Stated only when it is this agent's to state: with a chain, the store derives
                    // it from the head row and would drop whatever was sent anyway.
                    backend: (!chained).then(|| be.clone()),
                    arguments: agent_argument_values(
                        spec.as_ref(),
                        &be,
                        arguments.get(),
                        argument_values.get(),
                        form,
                        pm_applies,
                        temp_applies,
                    ),
                    // This form owns the tags, the star, and the project, so it states all three
                    // even when they are empty — `Some(empty)` clears, where the `None` the other
                    // forms send means "leave as is". A blank project is how "global" is said.
                    tags: Some(tags.get().split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect()),
                    starred: Some(starred.get()),
                    project: Some(project.get()),
                    // The adi tools ticked on for this agent — its own `.bin` at launch. This is
                    // the form that owns the checkboxes, so it states the set even when empty.
                    bin_tools: Some(form.bin_tools.get().into_iter().collect()),
                    // The secrets ticked on for this agent — injected into its runs as env vars.
                    secrets: Some(form.secrets.get().into_iter()
                        .map(|(project, name)| SecretRef { project, name })
                        .collect()),
                    // The knowledge bases ticked on, and whether this agent keeps its own memory.
                    // This is the form that owns both checkboxes, so it states them even when
                    // empty — the other forms send `None`, which leaves them as they are.
                    knowledge: Some(form.knowledge.get().into_iter().collect()),
                    memory: Some(form.memory.get()),
                    // Which agents this one may launch — this is the form that owns it, so it
                    // states the list even when empty. The store still refuses this outright when
                    // the save itself comes from inside a run, whatever is sent here.
                    can_spawn: Some(form.can_spawn.get()),
                    // This is the one form that edits the run environment, so it always states it —
                    // `Some(empty)` clears, where the `None` other forms send means "leave as is".
                    prelude: Some(parsed_prelude(&form.prelude.get())),
                    path: Some(parsed_path_dirs(&form.path.get())),
                    env: Some(parsed_env_vars(&form.env.get())),
                    // …and the same for whether this agent may stop and wait for somebody.
                    unattended: Some(form.unattended.get()),
                    // The ordered backend list — this agent's whole model configuration, and this
                    // is the form that owns it, so it states the list even when it is empty. An
                    // agent that lists none falls back to its runtime's own settings, which is what
                    // every agent did before backends existed.
                    backends: Some(form.llm_rows.get()),
                    // Editing with the name field changed is a rename, not a second agent.
                    rename_from: editing.get(),
                    // Absent means human — this is the control panel, and somebody is looking at it.
                    // Only takes effect on a create; the store ignores it on an edit.
                    created_by: None,
                };
                // Optimistic, like `editing` beside it: a create (or a rename) moves this page to
                // the saved agent's own URL, so a refresh lands back on what is in the form rather
                // than on `new` or on the name it used to have.
                editing.set(Some(nm.clone()));
                state.current_agent.set(nm.clone());
                replace_state(&agent_form_path(&nm));
                apply_agents(state, Some(busy), fetch::save_agent(body));
            }>
                {move || agent_form_sections(state, form)}

                <section class="adi-agents__section">
                    <h2 class="adi-agents__h2">"Tools"</h2>
                    {move || agent_tool_checkboxes(state, form)}
                    {move || agent_cli_field(state, form)}
                </section>

                <section class="adi-agents__section">
                    <h2 class="adi-agents__h2">"Knowledge"</h2>
                    {move || agent_knowledge_checkboxes(form)}
                </section>

                <section class="adi-agents__section">
                    <h2 class="adi-agents__h2">"Launching other agents"</h2>
                    {move || agent_spawn_field(state, form, route)}
                </section>

                <section class="adi-agents__section">
                    <h2 class="adi-agents__h2">"Secrets"</h2>
                    {move || agent_secret_checkboxes(state, form)}
                </section>

                <section class="adi-agents__section">
                    <h2 class="adi-agents__h2">"Run environment"</h2>
                    <div class="adi-agents__grid">{agent_environment_fields(form)}</div>
                </section>

                {move || agent_prompt_disclosure(state, form, prompt_open)}

                <footer class="adi-agents__actions">
                    <a class="adi-btn adi-btn--ghost" href=Route::Agents.path()
                        on:click=move |ev| spa_click(&ev, route, Route::Agents)>"Cancel"</a>
                    // The form page's one orange: saving is what the page is for.
                    <button class="adi-btn adi-btn--accent" type="submit" prop:disabled=move || busy.get()>
                        {move || if editing.get().is_some() { "Save changes" } else { "Create agent" }}
                    </button>
                </footer>
            </form>
            {flash_view(flash)}
        </div>
    }
    .into_any()
}

/// The `tools` schema field — which `adi-mono` command groups the agent may use — rendered under
/// the tool checkboxes it belongs with rather than among the backend params.
fn agent_cli_field(state: State, form: AgentsForm) -> AnyView {
    let Some(st) = state.agents.get() else {
        return ().into_any();
    };
    let only = ["tools".to_string()];
    view! {
        <div class="adi-agents__grid">
            {agent_schema_fields(&st.form, Some(only.as_slice()), &[], state, form)}
        </div>
    }
    .into_any()
}

/// The system prompt, folded under a disclosure row at the foot of the form: optional, advanced,
/// and long — open, it is the one field that takes the page.
fn agent_prompt_disclosure(state: State, form: AgentsForm, open: RwSignal<bool>) -> AnyView {
    let Some(st) = state.agents.get() else {
        return ().into_any();
    };
    let Some(field) = st
        .form
        .fields
        .iter()
        .find(|f| f.name == "system_prompt")
        .cloned()
    else {
        return ().into_any();
    };
    let placeholder = field.placeholder;
    view! {
        <div class="adi-agents__disclosure">
            <button class="adi-agents__disclosure-btn" type="button"
                aria-expanded=move || open.get().to_string()
                on:click=move |_| open.update(|o| *o = !*o)>
                <span class=move || if open.get() {
                    "adi-agents__caret adi-agents__caret--open"
                } else {
                    "adi-agents__caret"
                }>
                    <Icon icon=Lucide::ChevronRight size=IconSize::Sm/>
                </span>
                <span>{field.label}</span>
                <span class="adi-agents__disclosure-hint">"optional, advanced"</span>
            </button>
            {move || open.get().then(|| {
                let placeholder = placeholder.clone();
                view! {
                    <div class="adi-agents__prompt">
                        <textarea class="adi-textarea" id="agent-system-prompt" placeholder=placeholder
                            prop:value=move || form.system_prompt.get()
                            on:input=move |ev| form.system_prompt.set(event_target_value(&ev))></textarea>
                    </div>
                }
            })}
        </div>
    }
    .into_any()
}

/// Open an agent's editor on its own page: load `agent` into the form — or clear it, for a
/// definition that doesn't exist yet — then navigate to `/agents/<name>/edit` (or `/agents/new`).
///
/// Loading before navigating is what makes the editor render filled on the first paint. A project's
/// Agents panel lands here too, so Edit means the same thing wherever it is clicked.
pub(crate) fn open_agent_editor(
    state: State,
    route: RwSignal<Route>,
    form: AgentsForm,
    agent: Option<&AgentDto>,
) {
    match agent {
        Some(a) => load_agent_into_form(form, a),
        None => clear_agent_form(form),
    }
    let name = agent.map(|a| a.name.clone()).unwrap_or_default();
    state.current_agent.set(name.clone());
    push_state(&agent_form_path(&name));
    route.set(Route::AgentDetail);
    scroll_top();
}

/// One checkbox in a wrapping list of them. `muted` dims a choice that is listed but cannot be
/// taken; the reason travels in `title`.
fn check_box(
    label: impl IntoView + 'static,
    title: String,
    checked: impl Fn() -> bool + Send + Sync + 'static,
    disabled: bool,
    on_change: impl Fn(bool) + 'static,
    trailing: Option<AnyView>,
) -> AnyView {
    let class = if disabled {
        "adi-agents__check adi-agents__check--off"
    } else {
        "adi-agents__check"
    };
    view! {
        <label class=class title=title>
            <input type="checkbox" prop:checked=checked prop:disabled=disabled
                on:change=move |ev| on_change(event_target_checked(&ev)) />
            <span>{label}</span>
            {trailing}
        </label>
    }
    .into_any()
}

/// The per-agent tool checkboxes: one toggle per registered (active) tool — system or user — that
/// adds/removes its id from the agent's enabled set (`bin_tools`). The ticked tools become shims in
/// the agent's own `.bin`, on its PATH at launch. This is the "functionality enabled for this agent"
/// control: nothing is auto-added, including the system tools — an agent gets a tool only when it's
/// ticked on here.
fn agent_tool_checkboxes(state: State, form: AgentsForm) -> AnyView {
    let Some(st) = state.tools.get() else {
        return view! { <p class="adi-agents__loading">"Loading tools…"</p> }.into_any();
    };
    let mut tools: Vec<ToolDto> = st.tools.into_iter().filter(|t| !t.is_archived()).collect();
    // System tools first, then by name — the platform CLIs group together.
    tools.sort_by(|a, b| b.system.cmp(&a.system).then_with(|| a.name.cmp(&b.name)));
    if tools.is_empty() {
        return view! {
            <p class="adi-hint">
                "No tools yet — create one on the Tools page. Whatever you enable here lands "
                "in this agent's " <code>".bin"</code> "."
            </p>
        }
        .into_any();
    }
    let boxes = tools
        .into_iter()
        .map(|t| {
            let id = t.id.clone();
            let id_checked = id.clone();
            let id_toggle = id.clone();
            let checked = move || form.bin_tools.get().contains(&id_checked);
            let label = if t.system {
                format!("{} (system)", t.name)
            } else if let Some(p) = t.project.as_deref().filter(|p| !p.trim().is_empty()) {
                format!("{} · {p}", t.name)
            } else {
                t.name.clone()
            };
            let title = t.description.clone().unwrap_or_default();
            check_box(
                label,
                title,
                checked,
                false,
                move |on| {
                    form.bin_tools.update(|set| {
                        if on {
                            set.insert(id_toggle.clone());
                        } else {
                            set.remove(&id_toggle);
                        }
                    });
                },
                None,
            )
        })
        .collect::<Vec<_>>();
    view! {
        <div class="adi-field">
            <span class="adi-field__label">"This agent's .bin"</span>
            <div class="adi-agents__checks">{boxes}</div>
            <p class="adi-field__note">
                "Each one is asked " <code>"llm help"</code> " (then " <code>"help"</code> ", then "
                <code>"--help"</code> ") at launch, and what it answers is appended to this agent's "
                "system prompt — so it knows what the commands are without being told twice."
            </p>
        </div>
    }
    .into_any()
}

/// The agent's knowledge: whether it keeps a memory of its own, and which existing bases it
/// works with.
///
/// The base list is **honest about reach**. `knowledge` on an agent is a wish list, not a grant —
/// the three isolation levels decide what it actually reads, so a base outside this agent's scope
/// (another project's) is shown disabled with the reason, rather than offered as a checkbox that
/// would be silently dropped at read time. Another agent's memory *is* offered: every agent may
/// read every other's, which is the point of that level.
///
/// The list is fetched once, here, rather than polled into the shell state: a base's counts cost
/// a status pass over its storage.
fn agent_knowledge_checkboxes(form: AgentsForm) -> AnyView {
    // One fetch when the fieldset first renders. `Effect` rather than a fetch per render, and
    // guarded on `None` so re-opening the form doesn't re-ask.
    Effect::new(move |_| {
        if form.knowledge_bases.get_untracked().is_none() {
            spawn_local(async move {
                if let Ok(state) = fetch::knowledge().await {
                    form.knowledge_bases.set(Some(state.bases));
                }
            });
        }
    });

    let memory_toggle = view! {
        <div class="adi-field">
            <div class="adi-agents__checks">
                {check_box(
                    "Give this agent a memory of its own",
                    String::new(),
                    move || form.memory.get(),
                    false,
                    move |on| form.memory.set(on),
                    None,
                )}
            </div>
            <p class="adi-field__note">
                {move || {
                    let name = form.name.get();
                    let base = if name.trim().is_empty() {
                        "agent:<name>/memory".to_string()
                    } else {
                        format!("agent:{}/memory", name.trim())
                    };
                    view! {
                        <code>{base}</code>
                        " — it alone writes there, and every other agent may read it. Off by \
                         default: an agent that records what it learns is a different thing \
                         from one that does not."
                    }
                }}
            </p>
        </div>
    };

    view! {
        {memory_toggle}
        <div class="adi-field">
            <span class="adi-field__label">"Bases it searches"</span>
            {move || base_checkboxes(form)}
        </div>
    }
    .into_any()
}

/// One checkbox per existing base, minus this agent's own memory (which is the toggle above).
fn base_checkboxes(form: AgentsForm) -> AnyView {
    let Some(bases) = form.knowledge_bases.get() else {
        return view! { <p class="adi-agents__loading">"Loading knowledge bases…"</p> }.into_any();
    };
    let agent = form.name.get().trim().to_string();
    let project = form.project.get().trim().to_string();
    let offered: Vec<_> = bases
        .into_iter()
        // The agent's own memory is the toggle, not a checkbox — offering both would be two
        // controls for one fact.
        .filter(|b| !(b.memory && b.owner.as_deref() == Some(agent.as_str())))
        .collect();
    if offered.is_empty() {
        return view! {
            <p class="adi-hint">
                "No knowledge bases yet — make one on the Knowledge page. Whatever you tick here "
                "is what this agent searches."
            </p>
        }
        .into_any();
    }
    let boxes = offered
        .into_iter()
        .map(|b| {
            let id = b.id.clone();
            let (id_checked, id_toggle) = (id.clone(), id.clone());
            let checked = move || form.knowledge.get().contains(&id_checked);
            // Why this agent could not read it, if it could not. A project base belongs to one
            // project; an agent filed elsewhere (or nowhere) never sees it.
            let out_of_reach = match (b.level.as_str(), b.owner.as_deref()) {
                ("project", Some(owner)) if owner != project => Some(if project.is_empty() {
                    format!("only agents filed under project {owner} can read this")
                } else {
                    format!("this agent is filed under {project}, not {owner}")
                }),
                _ => None,
            };
            let disabled = out_of_reach.is_some();
            let title = out_of_reach
                .clone()
                .unwrap_or_else(|| match b.level.as_str() {
                    "agent" => "another agent's memory — readable, never writable".to_string(),
                    "project" => "this project's knowledge".to_string(),
                    _ => "shared by everything on this machine".to_string(),
                });
            // A base id is a machine string; the reason it is out of reach is a tag on the row.
            let label = view! { <span class="adi-mono">{b.id.clone()}</span> };
            let trailing = out_of_reach
                .map(|_| view! { <span class="adi-chip">"out of scope"</span> }.into_any());
            check_box(
                label,
                title,
                checked,
                disabled,
                move |on| {
                    form.knowledge.update(|set| {
                        if on {
                            set.insert(id_toggle.clone());
                        } else {
                            set.remove(&id_toggle);
                        }
                    });
                },
                trailing,
            )
        })
        .collect::<Vec<_>>();
    view! { <div class="adi-agents__checks">{boxes}</div> }.into_any()
}

/// A field that takes the whole row — the same rule `form::WIDE` states for the schema-driven
/// fields, repeated here because this section is hand-built rather than schema-rendered.
const SPAWN_FIELD_WIDE: &str = "flex:1 1 100%; min-width:0; grid-column:1 / -1";

/// Which agents this one's runs may launch, as removable chips, plus a picker/free-text box to add
/// one — a rule names a *pattern* (a glob, `project:<id>`, `*`) or an existing agent's exact name,
/// never one of a fixed set of things like the knowledge checkboxes above (ADI-MONO-114).
///
/// Paired with the editable reverse view: who can launch *this* agent. It is never stored on this
/// one — it is computed from everyone else's `can_spawn` (`AgentDto::spawned_by`) — so adding or
/// removing an entry there writes into the *caller's* definition through
/// `POST /api/agents/spawn-rule`, immediately, rather than waiting on this form's own Save.
///
/// Server-guarded on top of whatever this renders: a save made from inside a run drops `can_spawn`
/// outright, so this field only ever really moves when a person is looking at it (see
/// `Agents::save`).
fn agent_spawn_field(state: State, form: AgentsForm, route: RwSignal<Route>) -> AnyView {
    view! {
        <div class="adi-field" style=SPAWN_FIELD_WIDE>
            <span class="adi-field__label">"Can launch"</span>
            {move || agent_can_spawn_chips(state, form)}
            {move || agent_can_spawn_add(state, form)}
            {field_hint(
                "An exact agent name, a glob (dr-*), project:<id> (every agent filed directly \
                 under that project), or * (everything). Each shows how many currently-registered \
                 agents it matches right now, so a pattern is never a guess. Empty means this \
                 agent's runs may launch no agents. Checked only against an agent-to-agent \
                 launch — a person or an automated trigger is never refused by it.",
            )}
        </div>
        {move || form.editing.get().map(|name| {
            let name_for_add = name.clone();
            view! {
                <div class="adi-field" style=SPAWN_FIELD_WIDE>
                    <span class="adi-field__label">"Can be launched by"</span>
                    {move || agent_spawned_by_list(state, form, route)}
                    {move || agent_spawn_grant_add(state, form, name_for_add.clone())}
                    {field_hint(
                        "Every other agent whose own \"Can launch\" already reaches this one. An \
                         exact-name rule may be removed here; one reached only through a pattern, \
                         project:<id>, or * points at the caller's own page instead — narrow or \
                         remove it there.",
                    )}
                </div>
            }
        })}
    }
    .into_any()
}

/// The currently-editing agent's own fresh [`AgentDto`], read live off the polled agents list —
/// `None` for a not-yet-created agent (nothing to look up) or before the first load lands.
/// Deliberately not cached on the form: the match counts and the reverse view both change from
/// outside this form (another Allow click, another agent's own save), and a snapshot taken once at
/// load time would go stale the moment either did.
fn current_agent_dto(state: State, form: AgentsForm) -> Option<AgentDto> {
    let name = form.editing.get()?;
    state
        .agents
        .get()?
        .agents
        .into_iter()
        .find(|a| a.name == name)
}

/// The `can_spawn` chips: the draft rule list this form owns, each carrying the match count from
/// this agent's last-saved state when it has one — a rule just typed and not yet saved shows none.
fn agent_can_spawn_chips(state: State, form: AgentsForm) -> AnyView {
    let rules = form.can_spawn.get();
    if rules.is_empty() {
        return view! {
            <p class="adi-hint">"No rules yet — this agent's runs may launch nothing."</p>
        }
        .into_any();
    }
    let counts: BTreeMap<String, u32> = current_agent_dto(state, form)
        .map(|a| a.can_spawn.into_iter().map(|r| (r.rule, r.matches)).collect())
        .unwrap_or_default();
    let chips = rules
        .into_iter()
        .map(|rule| {
            let label = match counts.get(&rule) {
                Some(n) => format!("{rule} \u{2014} {n} agent{}", if *n == 1 { "" } else { "s" }),
                None => rule.clone(),
            };
            let remove = rule.clone();
            let title = format!("Remove {rule}");
            view! {
                <span class="adi-agents__rule-chip">
                    <span class="adi-mono">{label}</span>
                    <button type="button" title=title.clone() aria-label=title
                        on:click=move |_| {
                            form.can_spawn.update(|rules| rules.retain(|r| r != &remove));
                        }>
                        <Icon icon=Lucide::X size=IconSize::Sm/>
                    </button>
                </span>
            }
        })
        .collect::<Vec<_>>();
    view! { <div class="adi-agents__rule-chips">{chips}</div> }.into_any()
}

/// The box that adds a `can_spawn` rule: free text (so a pattern is always typeable), suggesting
/// every other registered agent's exact name and every project as `project:<id>` — the two shapes
/// that name something real, which is what a search/autocomplete offers per ADI-MONO-114. Purely
/// local: the rule joins the draft list and is only actually saved on the form's own Save.
fn agent_can_spawn_add(state: State, form: AgentsForm) -> AnyView {
    let self_name = form.name.get();
    let taken: std::collections::BTreeSet<String> = form.can_spawn.get().into_iter().collect();
    let agent_names: Vec<String> = state
        .agents
        .get()
        .map(|s| {
            s.agents
                .into_iter()
                .map(|a| a.name)
                .filter(|n| *n != self_name && !taken.contains(n))
                .collect()
        })
        .unwrap_or_default();
    let project_ids: Vec<String> = state
        .projects
        .get()
        .map(|p| {
            p.projects
                .into_iter()
                .filter(|proj| !proj.is_archived())
                .map(|proj| format!("project:{}", proj.id))
                .filter(|rule| !taken.contains(rule))
                .collect()
        })
        .unwrap_or_default();
    view! {
        <div class="adi-agents__chain-add">
            <input class="adi-input adi-mono" list="agent-can-spawn-options"
                aria-label="Add a can_spawn rule" placeholder="dr-*, project:acme, reviewer, *"
                prop:value=move || form.can_spawn_draft.get()
                on:input=move |ev| form.can_spawn_draft.set(event_target_value(&ev))
                on:keydown=move |ev| if ev.key() == "Enter" {
                    ev.prevent_default();
                    add_can_spawn_rule(form);
                } />
            <datalist id="agent-can-spawn-options">
                {agent_names.into_iter().map(|n| view! { <option value=n></option> }).collect::<Vec<_>>()}
                {project_ids.into_iter().map(|p| view! { <option value=p></option> }).collect::<Vec<_>>()}
                <option value="*"></option>
            </datalist>
            <button class="adi-btn" type="button" on:click=move |_| add_can_spawn_rule(form)>
                "Add"
            </button>
        </div>
    }
    .into_any()
}

/// Add the draft box's text to `form.can_spawn` (trimmed, deduped) and clear it.
fn add_can_spawn_rule(form: AgentsForm) {
    let rule = form.can_spawn_draft.get_untracked().trim().to_string();
    if rule.is_empty() {
        return;
    }
    form.can_spawn.update(|rules| {
        if !rules.iter().any(|r| r == &rule) {
            rules.push(rule);
        }
    });
    form.can_spawn_draft.set(String::new());
}

/// The "Can be launched by" rows: one per caller currently reaching this agent, each either
/// removable in place (an exact-name rule) or a link to the caller's own page (a pattern).
fn agent_spawned_by_list(state: State, form: AgentsForm, route: RwSignal<Route>) -> AnyView {
    let entries = current_agent_dto(state, form)
        .map(|a| a.spawned_by)
        .unwrap_or_default();
    if entries.is_empty() {
        return view! {
            <p class="adi-hint">"No other agent currently lists this one."</p>
        }
        .into_any();
    }
    let rows = entries
        .into_iter()
        .map(|e| spawned_by_row(state, form, route, e))
        .collect::<Vec<_>>();
    view! { <div class="adi-agents__rule-chips">{rows}</div> }.into_any()
}

/// One caller in the reverse view: an × that removes the exact rule responsible, or — reached only
/// through a pattern this page cannot safely narrow — a link to the caller's own page instead.
fn spawned_by_row(
    state: State,
    form: AgentsForm,
    route: RwSignal<Route>,
    entry: SpawnedByDto,
) -> AnyView {
    if entry.exact {
        let (caller, rule) = (entry.caller.clone(), entry.via.clone());
        let title = format!("Remove {caller}");
        view! {
            <span class="adi-agents__rule-chip">
                <span class="adi-mono">{entry.caller}</span>
                <button type="button" title=title.clone() aria-label=title
                    on:click=move |_| apply_agents(
                        state,
                        None,
                        fetch::set_spawn_rule(caller.clone(), rule.clone(), false),
                    )>
                    <Icon icon=Lucide::X size=IconSize::Sm/>
                </button>
            </span>
        }
        .into_any()
    } else {
        let caller = entry.caller.clone();
        let label = format!("{} \u{2014} via {}", entry.caller, entry.via);
        let title = format!("Only {} — narrow or remove it there", entry.caller);
        view! {
            <a class="adi-chip" href=agent_form_path(&caller) title=title
                on:click=move |ev| if spa_nav(&ev) {
                    open_other_agent(state, route, form, &caller);
                }>
                {label}
            </a>
        }
        .into_any()
    }
}

/// Navigate this same editor onto a different, already-registered agent — what a "via <pattern>"
/// link in the reverse view does, mirroring what the row menu's own Edit does with the row's DTO
/// already in hand.
fn open_other_agent(state: State, route: RwSignal<Route>, form: AgentsForm, name: &str) {
    let target = state
        .agents
        .get()
        .and_then(|s| s.agents.into_iter().find(|a| a.name == name));
    open_agent_editor(state, route, form, target.as_ref());
}

/// The box that grants another agent the right to launch `target` — writes "target" into the
/// picked agent's own `can_spawn`, immediately, through `POST /api/agents/spawn-rule` (this is a
/// write to *another* agent's definition, so it cannot wait on this form's own Save).
fn agent_spawn_grant_add(state: State, form: AgentsForm, target: String) -> AnyView {
    let already: std::collections::BTreeSet<String> = current_agent_dto(state, form)
        .map(|a| a.spawned_by.into_iter().map(|e| e.caller).collect())
        .unwrap_or_default();
    let candidates: Vec<String> = state
        .agents
        .get()
        .map(|s| {
            s.agents
                .into_iter()
                .map(|a| a.name)
                .filter(|n| *n != target && !already.contains(n))
                .collect()
        })
        .unwrap_or_default();
    view! {
        <div class="adi-agents__chain-add">
            <input class="adi-input adi-mono" list="agent-spawn-grant-options"
                aria-label="Agent to allow" placeholder="agent to allow"
                prop:value=move || form.spawn_grant_draft.get()
                on:input=move |ev| form.spawn_grant_draft.set(event_target_value(&ev))
                on:keydown={
                    let target = target.clone();
                    move |ev| if ev.key() == "Enter" {
                        ev.prevent_default();
                        grant_spawn_rule(state, form, target.clone());
                    }
                } />
            <datalist id="agent-spawn-grant-options">
                {candidates.into_iter().map(|n| view! { <option value=n></option> }).collect::<Vec<_>>()}
            </datalist>
            <button class="adi-btn" type="button"
                on:click=move |_| grant_spawn_rule(state, form, target.clone())>
                "Allow"
            </button>
        </div>
    }
    .into_any()
}

/// Send the grant box's text as an exact-name rule onto the picked agent's `can_spawn`, and clear
/// the box.
fn grant_spawn_rule(state: State, form: AgentsForm, target: String) {
    let caller = form.spawn_grant_draft.get_untracked().trim().to_string();
    if caller.is_empty() {
        return;
    }
    form.spawn_grant_draft.set(String::new());
    apply_agents(state, None, fetch::set_spawn_rule(caller, target, true));
}

/// The Observe/Enforce switch, beside the run counters: whether an `agent:<name>` launch outside
/// the caller's own `can_spawn` is actually refused, or only logged (ADI-MONO-113/114). The title
/// says what each mode means, since the two words alone do not.
fn spawn_policy_view(state: State) -> AnyView {
    let agents = state.agents;
    let policy = move || agents.get().map(|a| a.spawn_policy).unwrap_or_default();
    view! {
        <div class="adi-segmented" role="group" aria-label="Spawn policy"
            title="Observe: nothing is refused — every would-be refusal is only logged and \
                   listed below. Enforce: a launch outside the caller's Can launch is actually \
                   refused.">
            <button class="adi-segmented__option" type="button"
                aria-pressed=move || (policy() == "observe").to_string()
                on:click=move |_| apply_agents(
                    state, None, fetch::set_spawn_policy("observe".to_string()),
                )>
                "Observe"
            </button>
            <button class="adi-segmented__option" type="button"
                aria-pressed=move || (policy() == "enforce").to_string()
                on:click=move |_| apply_agents(
                    state, None, fetch::set_spawn_policy("enforce".to_string()),
                )>
                "Enforce"
            </button>
        </div>
    }
    .into_any()
}

/// The "would have been refused" list under the switch: every caller \u{2192} target pair on
/// record whose target does not match the caller's *current* rules (ADI-MONO-114) — derived by the
/// server, never stored, so a row disappears as soon as it is allowed. An empty list is the signal
/// that it is safe to switch to Enforce.
fn spawn_refusals_view(state: State) -> AnyView {
    let Some(refusals) = state.agents.get().map(|a| a.spawn_refusals) else {
        return ().into_any();
    };
    if refusals.is_empty() {
        return view! {
            <p class="adi-hint">
                "Nothing recorded would be refused right now \u{2014} it is safe to switch to \
                 Enforce."
            </p>
        }
        .into_any();
    }
    let rows = refusals
        .into_iter()
        .map(|r| {
            let (caller, target) = (r.caller.clone(), r.target.clone());
            let (caller_allow, target_allow) = (caller.clone(), target.clone());
            let times = if r.count == 1 { "once".to_string() } else { format!("{} times", r.count) };
            let when = actions::run_age(r.last_at);
            view! {
                <li class="adi-agents__refusal">
                    <span class="adi-mono">{caller}</span>
                    <Icon icon=Lucide::ArrowRight size=IconSize::Sm/>
                    <span class="adi-mono">{target}</span>
                    <span class="adi-muted">{format!("{times}, last {when}")}</span>
                    <button class="adi-btn adi-btn--link" type="button"
                        on:click=move |_| apply_agents(
                            state,
                            None,
                            fetch::set_spawn_rule(caller_allow.clone(), target_allow.clone(), true),
                        )>
                        "Allow"
                    </button>
                </li>
            }
        })
        .collect::<Vec<_>>();
    view! { <ul class="adi-agents__refusals">{rows}</ul> }.into_any()
}

/// The per-agent secret checkboxes: one toggle per registered secret (across every scope) that
/// adds/removes its `(scope, name)` reference from the agent's attachment set. Only the ticked
/// secrets are decrypted and injected into the agent's runs as environment variables — an explicit
/// allowlist, so nothing is inherited from a scope for merely existing. Populated from the shared
/// secrets list (`state.secrets`), which carries metadata only (never a value).
fn agent_secret_checkboxes(state: State, form: AgentsForm) -> AnyView {
    let Some(st) = state.secrets.get() else {
        return view! { <p class="adi-agents__loading">"Loading secrets…"</p> }.into_any();
    };
    let mut secrets: Vec<SecretDto> = st.secrets;
    // Global secrets first, then by project, then by name — the shared baseline groups together.
    secrets.sort_by(|a, b| {
        a.project
            .is_some()
            .cmp(&b.project.is_some())
            .then_with(|| a.project.cmp(&b.project))
            .then_with(|| a.name.cmp(&b.name))
    });
    if secrets.is_empty() {
        return view! {
            <p class="adi-hint">
                "No secrets yet — add one on the Secrets page. Whatever you tick here is "
                "injected into this agent's runs as an environment variable."
            </p>
        }
        .into_any();
    }
    let boxes = secrets
        .into_iter()
        .map(|s| {
            let key = (s.project.clone(), s.name.clone());
            let key_checked = key.clone();
            let key_toggle = key.clone();
            let checked = move || form.secrets.get().contains(&key_checked);
            // The name is the env var the run sees, so it is mono; the project it is filed under
            // is a name and is not.
            let scope = s
                .project
                .as_deref()
                .filter(|p| !p.trim().is_empty())
                .map(|p| format!(" · {p}"));
            let label = view! { <span class="adi-mono">{s.name.clone()}</span>{scope} };
            // The value is never sent here; the tooltip carries the description / OAuth provider.
            let title = match (&s.description, &s.oauth) {
                (Some(d), _) if !d.trim().is_empty() => d.clone(),
                (_, Some(o)) => format!("OAuth · {}", o.provider),
                _ => String::new(),
            };
            check_box(
                label,
                title,
                checked,
                false,
                move |on| {
                    form.secrets.update(|set| {
                        if on {
                            set.insert(key_toggle.clone());
                        } else {
                            set.remove(&key_toggle);
                        }
                    });
                },
                None,
            )
        })
        .collect::<Vec<_>>();
    view! {
        <div class="adi-field">
            <span class="adi-field__label">"Injected as environment variables"</span>
            <div class="adi-agents__checks">{boxes}</div>
        </div>
    }
    .into_any()
}

/// The Agents page's Mine/All toggle, beside the run count in the header.
///
/// A plain two-way toggle rather than the rail's dropdown-with-a-menu: there is no grouping to carry
/// alongside it here, so a button that says what it is narrowed to (and flips it on a click) is the
/// whole control. Lit while narrowed, the same "a list showing less than everything has to say so"
/// rule the rail's own filter button follows.
fn agents_filter_toggle(filter: RwSignal<AgentsFilter>) -> AnyView {
    let narrowed = filter.get() == AgentsFilter::Mine;
    let title = if narrowed {
        "Showing agents you created — a person, or unattributed (every agent from before this was \
         recorded). Starred agents always show. Click to show every agent, including the ones a \
         script or another agent made."
    } else {
        "Showing every agent. Click to narrow to the ones you created."
    };
    view! {
        <button class="adi-btn adi-btn--ghost" type="button" title=title class:is-on=narrowed
            on:click=move |_| filter.update(|f| *f = match *f {
                AgentsFilter::Mine => AgentsFilter::All,
                AgentsFilter::All => AgentsFilter::Mine,
            })>
            {if narrowed { "Mine" } else { "All" }}
        </button>
    }
    .into_any()
}

/// Render the agents table body: a loading/empty placeholder, or one row per agent. Every action —
/// the launch controls, Edit, Simulate, Delete — sits in the row's ⋯ menu; the only thing drawn
/// beside it is a dot on the rows that are running.
fn agent_rows(
    state: State,
    form: AgentsForm,
    watch: AgentsWatch,
    sim: Simulate,
    route: RwSignal<Route>,
    filter: AgentsFilter,
) -> AnyView {
    let table = state.tables.agents;
    let mut agents = match rows_or_status(
        table,
        state.agents.get().map(|v| v.agents),
        "No agents yet — “New agent” starts one.",
        read_error(state, "/api/agents"),
    ) {
        Ok(rows) => rows,
        Err(placeholder) => return placeholder,
    };
    if filter == AgentsFilter::Mine {
        // Starred is a second, separate exemption — a machine-made agent somebody starred must not
        // vanish from the default view just because a script or another agent made it.
        agents.retain(|a| agent_is_mine(a) || a.starred);
        if agents.is_empty() {
            return view! {
                <adi_ui::EmptyRow state=table>
                    "No agents you created — everything registered here was made by a script or \
                     another agent. Switch to All to see them."
                </adi_ui::EmptyRow>
            }
            .into_any();
        }
    }
    sort_rows(&mut agents, table.sort.get(), agent_key, |a| {
        Key::text(&a.name)
    });
    agents
        .into_iter()
        .map(|a| {
            let del_name = a.name.clone();
            let a_edit = a.clone();
            let a_name = a.name.clone();
            let sim_name = a.name.clone();
            let mut items = Vec::new();
            // The launch controls lead the menu, on the rows that have any.
            if a.runnable || a.running {
                items.push(
                    view! { <div class="adi-agents__live">{agent_actions(state, watch, &a)}</div> }
                        .into_any(),
                );
            }
            items.push(menu_item(state, "Edit", false, move || {
                open_agent_editor(state, route, form, Some(&a_edit));
            }));
            // The setup wizard, on this agent — a real navigation into the root document (see
            // `routing::reconfigure_href`), not a route change.
            items.push(menu_item(state, "Reconfigure", false, move || {
                if let Some(w) = web_sys::window() {
                    let _ = w.location().set_href(&reconfigure_href(&a_name));
                }
            }));
            // Take the model's seat in a run of this agent: a way of reading what the agent is
            // told, not of running it — it opens a screen rather than starting work.
            items.push(menu_item(state, "Simulate", false, move || {
                simulate::start_simulation(
                    state,
                    sim,
                    sim_name.clone(),
                    "Simulated run — read the prompt and take the model's seat.".to_string(),
                );
            }));
            items.push(menu_item(state, "Delete", true, move || {
                apply_agents(
                    state,
                    None,
                    fetch::delete_agent(del_name.clone()),
                );
            }));
            let live = a
                .running
                .then(|| view! { <span class="adi-agents__live-dot" title="running"></span> });
            let actions = row_actions(state, format!("agent:{}", a.name), live, items);
            view! {
                <TableRow state=table cell=move |col| agent_cell(col, &a) actions=actions/>
            }
            .into_any()
        })
        .collect::<Vec<_>>()
        .into_any()
}

/// An agent's sort key under `col`. Shared with a project's Agents panel, which shows a subset of
/// the same columns over the same rows.
pub(crate) fn agent_key(a: &AgentDto, col: &str) -> Key {
    match col {
        "Backend" => Key::text(&a.backend),
        "Model" => Key::text(argument_text(&a.arguments, "model")),
        "Project" => Key::maybe(a.project.as_deref()),
        "Tags" => Key::text(a.tags.join(", ")),
        // "Name", and any header without a key of its own. Starred agents sort with their name,
        // not ahead of it — the star is a marker on the row, not a separate rank.
        _ => Key::text(&a.name),
    }
}

/// One agent's cell under `col`. Matching the header text — the same key the sort uses — is what
/// lets the user hide and reorder columns without the row builder knowing about it. The Status
/// column belongs to a project's panel, which builds it there from the live watch.
pub(crate) fn agent_cell(col: &str, a: &AgentDto) -> AnyView {
    match col {
        // Backend and model are machine ids, and the backend repeats down the column.
        "Backend" => view! { <span class="adi-mono">{a.backend.clone()}</span> }.into_any(),
        "Model" => match argument_text(&a.arguments, "model") {
            m if m.is_empty() => view! { <span class="adi-muted">"—"</span> }.into_any(),
            m => view! { <span class="adi-mono">{m}</span> }.into_any(),
        },
        "Project" => match &a.project {
            Some(p) if !p.trim().is_empty() => {
                view! { <span><span class="adi-chip">{p.clone()}</span></span> }.into_any()
            }
            _ => view! { <span><span class="adi-muted">"—"</span></span> }.into_any(),
        },
        "Tags" => match a.tags.join(", ") {
            t if t.is_empty() => view! { <span class="adi-muted">"—"</span> }.into_any(),
            t => view! { <span class="adi-muted">{t}</span> }.into_any(),
        },
        // "Name", and anything the layout offers that this match doesn't name.
        _ => agent_name_cell(a),
    }
}

/// The name, led by its star: lit on a starred agent, faint on the rest so every name starts on
/// the same edge. A machine-made definition — one [`agent_is_mine`] would exclude from Mine — also
/// carries who made it, whether it is on screen because Mine is off or because starring exempted it:
/// either way, a reader looking at a name Mine wouldn't have shown on its own is owed the reason.
pub(crate) fn agent_name_cell(a: &AgentDto) -> AnyView {
    let star = if a.starred {
        "adi-agents__star adi-agents__star--on"
    } else {
        "adi-agents__star"
    };
    let title = if a.starred { "starred" } else { "not starred" };
    view! {
        <span class="adi-agents__name">
            <span class=star title=title><Icon icon=Lucide::Star size=IconSize::Sm/></span>
            {a.name.clone()}
            {creator_marker(a)}
            {spawn_marker(a)}
        </span>
    }
    .into_any()
}

/// "by <name>" for a machine-made definition, or nothing for one [`agent_is_mine`] counts as mine.
fn creator_marker(a: &AgentDto) -> Option<AnyView> {
    if agent_is_mine(a) {
        return None;
    }
    let who = a
        .created_by
        .strip_prefix("agent:")
        .unwrap_or(&a.created_by);
    Some(
        view! {
            <span class="adi-chip" title="who created this definition">{format!("by {who}")}</span>
        }
        .into_any(),
    )
}

/// "launches N" (or "launches *" when one rule is the wildcard) for an agent with a non-empty
/// `can_spawn` (ADI-MONO-114), so it is visible which agents launch others without opening each
/// one's edit page. Nothing for the overwhelming majority, whose `can_spawn` is empty.
fn spawn_marker(a: &AgentDto) -> Option<AnyView> {
    if a.can_spawn.is_empty() {
        return None;
    }
    let label = if a.can_spawn.iter().any(|r| r.rule == "*") {
        "launches *".to_string()
    } else {
        format!("launches {}", a.can_spawn.len())
    };
    Some(
        view! {
            <span class="adi-chip" title="this agent's runs may launch others — see Launching \
                other agents on its edit page">{label}</span>
        }
        .into_any(),
    )
}

/// The live view's input row, wired to the watched agent's pty session.
fn send_bar(state: State, watch: AgentsWatch) -> impl IntoView {
    crate::ui::send_bar(watch.input, "type to the agent…", move |text, key| {
        send_to_agent(state, watch, text, key);
    })
}

/// Type into the watched agent's session: send `text` literally, then press `key`. The reply is
/// a fresh pane snapshot, applied immediately (unless the view moved on meanwhile) so the
/// keystrokes show without waiting for the next poll; errors go to the flash line.
fn send_to_agent(state: State, watch: AgentsWatch, text: String, key: &'static str) {
    let Some(name) = watch.name.get_untracked() else {
        return;
    };
    if text.is_empty() && key.is_empty() {
        return;
    }
    let key = key.to_string();
    let node = watch.node.get_untracked();
    spawn_local(async move {
        match fetch::send_agent_keys(node.as_deref(), name, text, key).await {
            Ok(peek) => {
                if watch.name.get_untracked().as_deref() == Some(peek.name.as_str()) {
                    watch.peek.set(Some(peek));
                }
            }
            Err(e) => state.flash.set(Some(Flash::err(e))),
        }
    });
}

fn argument_text(arguments: &BTreeMap<String, serde_json::Value>, name: &str) -> String {
    arguments
        .get(name)
        .and_then(scalar_argument_text)
        .unwrap_or_default()
}

fn scalar_argument_text(value: &serde_json::Value) -> Option<String> {
    match value {
        serde_json::Value::String(value) => Some(value.clone()),
        serde_json::Value::Bool(value) => Some(value.to_string()),
        serde_json::Value::Number(value) => Some(value.to_string()),
        _ => None,
    }
}
