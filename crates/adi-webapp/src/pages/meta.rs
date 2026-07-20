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
        {intro_panel()}
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

/// The header blurb: what the meta-agent is and what it's for.
fn intro_panel() -> AnyView {
    view! {
        <section class="adi-panel">
            <p class="adi-hint">
                <strong>"adi-agent"</strong>
                " is your environment's default agent — a meta-agent that helps you set up and run
                 this ADI stack. Pick a backend (Claude, Codex, the ADI loop, …); it comes preloaded
                 with a system prompt that teaches it how ADI works — projects, hive services,
                 dashboards, ports, and DNS. Edit the prompt to taste, then run it right here."
            </p>
        </section>
    }
    .into_any()
}

/// Shown until the first `/api/meta` response lands.
fn loading_panel() -> AnyView {
    view! {
        <section class="adi-panel"><div class="adi-empty">"Loading…"</div></section>
    }
    .into_any()
}

/// The setup form — backend picker + the (prefilled, editable) system prompt. Used both to create
/// the agent for the first time and to reconfigure an existing one (with a Cancel back to the
/// summary).
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
                <span class="adi-spacer"></span>
                {(!creating).then(|| view! {
                    <button class="adi-btn adi-btn--link" type="button"
                        on:click=move |_| form.editing.set(false)>"Cancel"</button>
                })}
            </div>
            <form class="adi-form" on:submit=move |ev| {
                ev.prevent_default();
                submit_setup(state, form);
            }>
                <div class="adi-field">
                    <label class="adi-field__label" for="meta-backend">"Backend"</label>
                    <select class="adi-input" id="meta-backend"
                        prop:value=move || form.backend.get()
                        on:change=move |ev| form.backend.set(event_target_value(&ev))>
                        <option value="">"— pick a backend —"</option>
                        {backends.into_iter().map(|b| {
                            let id = b.id;
                            let label = b.label;
                            view! { <option value=id>{label}</option> }
                        }).collect::<Vec<_>>()}
                    </select>
                </div>
                <div class="adi-field" style="flex:1 1 100%; min-width:0">
                    <label class="adi-field__label" for="meta-prompt">"System prompt"</label>
                    <textarea class="adi-textarea adi-mono" id="meta-prompt" rows="16"
                        placeholder="How this agent should operate your ADI environment…"
                        prop:value=move || form.prompt.get()
                        on:input=move |ev| form.prompt.set(event_target_value(&ev))></textarea>
                </div>
                <button class="adi-btn adi-btn--primary" type="submit"
                    prop:disabled=move || form.busy.get()>{action}</button>
            </form>
        </section>
    }
    .into_any()
}

/// The summary shown once `adi-agent` exists: its backend/model/run state, the run controls (shared
/// with the Agents page), a Reconfigure button, and its system prompt behind a disclosure.
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
    let prompt = arg_text(&a.arguments, "system_prompt");
    let has_prompt = !prompt.trim().is_empty();
    let a_for_actions = a.clone();
    let a_for_edit = a.clone();
    view! {
        <section class="adi-panel">
            <div class="adi-panel__head">
                <h2 class="adi-panel__title">{a.name.clone()}</h2>
                <span class="adi-chip adi-mono" title="backend">{backend}</span>
                {(!model.is_empty()).then(|| view! {
                    <span class="adi-chip adi-mono" title="model">{model}</span>
                })}
                <span class="adi-chip">{if running { "● running" } else { "idle" }}</span>
                <span class="adi-spacer"></span>
                {agent_actions(state, watch, &a_for_actions)}
                <button class="adi-btn adi-btn--link" type="button"
                    on:click=move |_| start_reconfigure(form, &a_for_edit)>"Reconfigure"</button>
            </div>
            {has_prompt.then(|| view! {
                <details>
                    <summary class="adi-muted">"System prompt"</summary>
                    <pre class="adi-term">{prompt}</pre>
                </details>
            })}
            <div class="adi-hint">
                "Run the agent to start a session (interactive backends) or a headless run you give a
                 task to — the live view opens below. For fine-grained settings (model, tools,
                 permissions) edit it on the "
                <a class="adi-btn adi-btn--link" href=Route::Agents.path()
                    on:click=move |ev| spa_click(&ev, route, Route::Agents)>"Agents"</a>
                " page."
            </div>
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
        state
            .flash
            .set(Some(Flash::err("Pick a backend for the agent.".to_string())));
        return;
    }
    let prompt = form.prompt.get_untracked();
    let meta = state.meta.get_untracked();
    let name = meta
        .as_ref()
        .map_or_else(|| "adi-agent".to_string(), |m| m.name.clone());
    // Start from the agent's existing arguments so a reconfigure keeps its model/tools/etc.
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
        tags: Vec::new(),
        starred: false,
        project: None,
        bin_tools: Vec::new(),
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
