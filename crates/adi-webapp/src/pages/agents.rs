//! The Agents page: create, edit, and delete agent definitions (docs/adi-agents.md §5) — pick a
//! backend, a system prompt, a tool scope, and backend-specific params. No run/orchestration here;
//! this only edits the stored spec. The form adapts its params to the chosen backend kind.

use adi_webapp_api::types::{AgentDto, AgentsState, SaveAgent};
use leptos::prelude::*;

use crate::fetch;
use crate::routing::scroll_top;
use crate::state::{AgentsForm, Flash, State};
use crate::ui::{
    TextField, apply_mutation, data_table, flash_view, placeholder_row, tile, updated_text,
};

/// The Agents page: create, edit, and delete agent definitions (docs/adi-agents.md §5) — pick a
/// backend, a system prompt, a tool scope, and backend-specific params. No run/orchestration here;
/// this only edits the stored spec. The form adapts its params to the chosen backend kind.
pub(crate) fn agents_view(state: State, form: AgentsForm) -> AnyView {
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
                        <option value="api:monshoot">"Monshoot (API)"</option>
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
            let name_disp = if a.starred {
                format!("★ {}", a.name)
            } else {
                a.name.clone()
            };
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
    form.permission_mode
        .set(a.permission_mode.clone().unwrap_or_default());
    form.temperature
        .set(a.temperature.map(|t| t.to_string()).unwrap_or_default());
    form.max_turns
        .set(a.max_turns.map(|n| n.to_string()).unwrap_or_default());
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
        "api:monshoot" => "kimi-k2.6 / kimi-k2",
        "api:ollama" => "llama3.1 / qwen2.5-coder",
        _ => "model alias",
    }
}

/// Trim a form string into an optional, dropping it when blank.
fn opt_str(value: String) -> Option<String> {
    let trimmed = value.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}
