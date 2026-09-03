//! The Meta page (`/meta`): set up and run the default ADI agent — a single well-known global
//! agent named `adi-agent`. It's the "meta-agent" that helps the user configure and operate this
//! ADI environment.
//!
//! The page is deliberately thin: creating the agent is an ordinary `/api/agents/save` under the
//! well-known name, and running/watching it reuses the Agents page's run controls
//! ([`agent_actions`]) and live view ([`live_view`]). All this page adds is a focused setup form
//! (backend + a prefilled, editable system prompt) and a summary of the agent once it exists.

use std::collections::BTreeMap;

use adi_webapp_api::types::{AgentDto, MetaState, SaveAgent};
use leptos::prelude::*;
use wasm_bindgen_futures::spawn_local;

use crate::fetch;
use crate::routing::{Route, spa_click};
use crate::state::{AgentsWatch, Flash, MetaForm, State};
use crate::ui::flash_view;

use super::agents::{agent_actions, live_view};

/// The Meta page: an intro, then either the setup form (no agent yet, or reconfiguring) or the
/// ready summary with run controls, and the shared live-view/run panel underneath.
pub(crate) fn meta_view(
    state: State,
    route: RwSignal<Route>,
    form: MetaForm,
    watch: AgentsWatch,
) -> AnyView {
    view! {
        {intro()}
        {move || match state.meta.get() {
            None => loading_panel(),
            // The setup form doubles as create (no agent yet) and reconfigure (editing an existing
            // one). Otherwise show the ready summary for the agent we have.
            Some(m) => match (m.agent.clone(), form.editing.get()) {
                (Some(agent), false) => ready_panel(state, route, form, watch, agent),
                _ => setup_panel(state, form, m),
            },
        }}
        {move || live_view(state, watch)}
        {flash_view(state.flash)}
    }
    .into_any()
}

/// The lead under the page title: what the meta-agent is and what it's for.
fn intro() -> AnyView {
    view! {
        <p class="adi-meta__lead">
            <b>"adi-agent"</b>
            " is your environment's default agent — a meta-agent that helps you set up and run
             this ADI stack. Pick a backend (Claude, Codex, the ADI loop, …); it comes preloaded
             with a system prompt that teaches it how ADI works — projects, hive services,
             dashboards, ports, and DNS — and with every tool in your store enabled on it.
             Edit the prompt to taste, then run it right here."
        </p>
    }
    .into_any()
}

/// Shown until the first `/api/meta` response lands.
fn loading_panel() -> AnyView {
    view! { <div class="adi-empty">"Loading…"</div> }.into_any()
}

/// The setup form — backend picker + the (prefilled, editable) system prompt. Used both to create
/// the agent for the first time and to reconfigure an existing one (with a Cancel back to the
/// summary). Save is the screen's one orange: the form is what the page is for until it is done.
fn setup_panel(state: State, form: MetaForm, m: MetaState) -> AnyView {
    let creating = m.agent.is_none();
    let backends = m.form.backends.clone();
    let title = if creating {
        "Set up adi-agent"
    } else {
        "Reconfigure adi-agent"
    };
    let action = if creating {
        "Create adi-agent"
    } else {
        "Save changes"
    };
    view! {
        <section class="adi-panel">
            <div class="adi-panel__head">
                <h2 class="adi-panel__title">{title}</h2>
            </div>
            <form class="adi-meta__form" on:submit=move |ev| {
                ev.prevent_default();
                submit_setup(state, form);
            }>
                <div class="adi-field">
                    <label class="adi-field__label" for="meta-backend">"Backend"</label>
                    <select class="adi-input" id="meta-backend"
                        prop:value=move || form.backend.get()
                        on:change=move |ev| form.backend.set(event_target_value(&ev))>
                        <option value="">"Pick a backend"</option>
                        {backends.into_iter().map(|b| {
                            let id = b.id;
                            let label = b.label;
                            view! { <option value=id>{label}</option> }
                        }).collect::<Vec<_>>()}
                    </select>
                </div>
                <div class="adi-field">
                    <label class="adi-field__label" for="meta-prompt">"System prompt"</label>
                    <textarea class="adi-textarea" id="meta-prompt" rows="16"
                        placeholder="How this agent should operate your ADI environment…"
                        prop:value=move || form.prompt.get()
                        on:input=move |ev| form.prompt.set(event_target_value(&ev))></textarea>
                </div>
                <div class="adi-meta__actions">
                    {(!creating).then(|| view! {
                        <button class="adi-btn adi-btn--ghost" type="button"
                            on:click=move |_| form.editing.set(false)>"Cancel"</button>
                    })}
                    <span class="adi-spacer"></span>
                    <button class="adi-btn adi-btn--accent" type="submit"
                        prop:disabled=move || form.busy.get()>{action}</button>
                </div>
            </form>
        </section>
    }
    .into_any()
}

/// The summary shown once `adi-agent` exists: its name with its state beside it, a key-value
/// list of what it runs on, the run controls (shared with the Agents page), a Reconfigure link,
/// and its system prompt behind a disclosure.
fn ready_panel(
    state: State,
    route: RwSignal<Route>,
    form: MetaForm,
    watch: AgentsWatch,
    a: AgentDto,
) -> AnyView {
    let backend = a.backend.clone();
    let model = arg_text(&a.arguments, "model");
    let running = a.running;
    let tool_count = a.bin_tools.len();
    let prompt = arg_text(&a.arguments, "system_prompt");
    let has_prompt = !prompt.trim().is_empty();
    let a_for_actions = a.clone();
    let a_for_edit = a.clone();
    let tools = match tool_count {
        0 => "none".to_string(),
        1 => "1 tool".to_string(),
        n => format!("{n} tools"),
    };
    view! {
        <section class="adi-panel">
            <div class="adi-panel__head">
                <h2 class="adi-panel__title">{a.name.clone()}</h2>
                <span class="adi-status" data-state={if running { "running" } else { "idle" }}>
                    <span class="adi-status__led"></span>
                    <span>{if running { "running" } else { "idle" }}</span>
                </span>
                <span class="adi-spacer"></span>
                {agent_actions(state, watch, &a_for_actions)}
                <button class="adi-btn adi-btn--link" type="button"
                    on:click=move |_| start_reconfigure(form, &a_for_edit)>"Reconfigure"</button>
            </div>
            <dl class="adi-meta__kv">
                <dt>"Backend"</dt><dd class="adi-mono">{backend}</dd>
                <dt>"Model"</dt>
                {if model.is_empty() {
                    view! { <dd class="adi-muted">"\u{2014}"</dd> }.into_any()
                } else {
                    view! { <dd class="adi-mono">{model}</dd> }.into_any()
                }}
                <dt>"Tools"</dt><dd>{tools}</dd>
            </dl>
            {has_prompt.then(|| view! {
                <details class="adi-meta__prompt">
                    <summary>"System prompt"</summary>
                    <pre class="adi-term">{prompt}</pre>
                </details>
            })}
            <p class="adi-hint">
                "Run the agent to start a session (interactive backends) or a headless run you give a
                 task to — the live view opens below. For fine-grained settings (model, tools,
                 permissions) edit it on the "
                <a class="adi-link" href=Route::Agents.path()
                    on:click=move |ev| spa_click(&ev, route, Route::Agents)>"Agents"</a>
                " page."
            </p>
        </section>
    }
    .into_any()
}

/// Save the setup form as the `adi-agent` definition (create or update). Any other arguments the
/// agent already carries (model, tools, …) are preserved — only the backend and system prompt are
/// edited here. Refreshes `/api/meta` afterwards so the summary reflects the save.
fn submit_setup(state: State, form: MetaForm) {
    let backend = form.backend.get_untracked().trim().to_string();
    if backend.is_empty() {
        state.flash.set(Some(Flash::err(
            "Pick a backend for the agent.".to_string(),
        )));
        return;
    }
    let prompt = form.prompt.get_untracked();
    let meta = state.meta.get_untracked();
    let name = meta
        .as_ref()
        .map_or_else(|| "adi-agent".to_string(), |m| m.name.clone());
    // The meta-agent runs the whole environment, so it gets every tool the store has: the
    // server's default set unioned with whatever is already enabled. Stated rather than omitted
    // because a reconfigure is also how the agent picks up tools registered since it was made.
    let bin_tools = Some(meta_bin_tools(meta.as_ref()));
    // Start from the agent's existing arguments so a reconfigure keeps its model/etc.
    let mut arguments = meta
        .and_then(|m| m.agent)
        .map(|a| a.arguments)
        .unwrap_or_default();
    if prompt.trim().is_empty() {
        arguments.remove("system_prompt");
    } else {
        arguments.insert(
            "system_prompt".to_string(),
            serde_json::Value::String(prompt),
        );
    }
    let body = SaveAgent {
        name,
        backend,
        arguments,
        tags: None,
        starred: None,
        project: None,
        bin_tools,
        // As `path` and `env` below: this form doesn't edit the pre-run commands, so `None`
        // leaves whatever the agent already has.
        prelude: None,
        secrets: None,
        // No form offers the knowledge bases or the memory toggle yet, so none of them
        // states one: `None` leaves whatever the agent already has. Set them with
        // `adi-mono agents save --knowledge … --memory` until the editor grows the
        // checkboxes.
        knowledge: None,
        memory: None,
        // This form doesn't edit the run environment — `None` leaves whatever the
        // agent already has, instead of clearing it on every save.
        path: None,
        env: None,
        unattended: None,
        rename_from: None,
    };
    form.busy.set(true);
    spawn_local(async move {
        match fetch::save_agent(body).await {
            Ok(agents_state) => {
                state.agents.set(Some(agents_state));
                state
                    .flash
                    .set(Some(Flash::ok("Saved your ADI agent.".to_string())));
                form.editing.set(false);
                if let Ok(m) = fetch::meta().await {
                    state.meta.set(Some(m));
                }
            }
            Err(e) => state.flash.set(Some(Flash::err(e))),
        }
        form.busy.set(false);
    });
}

/// The tools to save the meta-agent with: the server's default set (every active tool) unioned
/// with the ones it already has enabled. The union is what keeps a save additive — a tool the user
/// ticked on the Agents page survives, and a tool registered since the agent was created gets
/// picked up. Sorted and deduped, so the saved list is stable across saves.
pub(crate) fn meta_bin_tools(meta: Option<&MetaState>) -> Vec<String> {
    let Some(m) = meta else {
        return Vec::new();
    };
    let mut ids: std::collections::BTreeSet<String> = m.default_bin_tools.iter().cloned().collect();
    if let Some(agent) = m.agent.as_ref() {
        ids.extend(agent.bin_tools.iter().cloned());
    }
    ids.into_iter().collect()
}

/// Load the current agent's backend + system prompt into the form and switch to the setup view.
fn start_reconfigure(form: MetaForm, a: &AgentDto) {
    form.backend.set(a.backend.clone());
    form.prompt.set(arg_text(&a.arguments, "system_prompt"));
    form.editing.set(true);
}

/// A scalar backend argument as display text (string/bool/number), or empty when absent/structured.
fn arg_text(arguments: &BTreeMap<String, serde_json::Value>, name: &str) -> String {
    match arguments.get(name) {
        Some(serde_json::Value::String(s)) => s.clone(),
        Some(serde_json::Value::Bool(b)) => b.to_string(),
        Some(serde_json::Value::Number(n)) => n.to_string(),
        _ => String::new(),
    }
}
