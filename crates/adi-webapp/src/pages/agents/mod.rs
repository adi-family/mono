//! The Agents page: create, edit, delete, and launch agent definitions (docs/adi-agents.md §5) —
//! pick a backend (`executor:what`), a system prompt, a CLI command scope, and backend-specific
//! params. ▶ Run starts either an interactive tmux session or a headless background process;
//! deeper orchestration is future work. The form adapts its params to the chosen backend, and
//! for the `harness:adi` backend also to its chosen provider.

use std::collections::BTreeMap;

use adi_webapp_api::types::{SaveAgent, SecretDto, SecretRef, ToolDto};
use leptos::prelude::*;
use wasm_bindgen_futures::spawn_local;

use crate::fetch;
use crate::state::{AgentCodeEditor, AgentsForm, AgentsWatch, Flash, State};
use crate::ui::{data_table, flash_view, placeholder_row, updated_text};

mod actions;
mod code;
mod form;

use actions::apply_agents;
pub(crate) use actions::{agent_actions, live_view, poll_watch};
use code::{code_editor_view, open_code_editor};
pub(crate) use form::load_agent_into_form;
use form::{agent_argument_values, agent_form_fields, agent_param_applies, clear_agent_form};

/// The Agents page: create, edit, delete, and launch agent definitions (docs/adi-agents.md §5) —
/// pick a backend (`executor:what`), a system prompt, a CLI command scope, and backend-specific
/// params. ▶ Run starts either an interactive tmux session or a headless background process;
/// deeper orchestration is future work. The form adapts its params to the chosen backend, and
/// for the `harness:adi` backend also to its chosen provider.
pub(crate) fn agents_view(
    state: State,
    form: AgentsForm,
    watch: AgentsWatch,
    code: AgentCodeEditor,
) -> AnyView {
    let agents = state.agents;
    let secs_since = state.secs_since;
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
    view! {
        {move || live_view(state, watch)}

        {move || code_editor_view(state, code)}

        <section class="adi-panel">
            <div class="adi-panel__head">
                <span class="adi-chip adi-mono" title="Agents defined">
                    {move || agents.get().map_or_else(|| "\u{2014}".to_string(),
                        |a| a.agents.len().to_string())}
                </span>
                <span class="adi-updated">{move || updated_text(agents, secs_since)}</span>
            </div>

            {data_table(&["Name", "Backend", "Model", "Project", "Tags", ""], move || agent_rows(state, form, watch, code))}
        </section>

        <section class="adi-panel">
            <div class="adi-panel__head">
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
                let st = agents.get();
                let prov = argument_values.get().get("provider").cloned().unwrap_or_default();
                // Whether each backend-conditional first-class param applies is driven by the
                // server schema (does a field of that name apply to this backend?), so rescoping a
                // field in the API also stops its value being sent for backends it no longer fits.
                let pm_applies = agent_param_applies(st.as_ref(), &be, &prov, "permission_mode");
                let temp_applies = agent_param_applies(st.as_ref(), &be, &prov, "temperature");
                let body = SaveAgent {
                    name: nm.clone(),
                    backend: be.clone(),
                    arguments: agent_argument_values(
                        st.as_ref(),
                        &be,
                        arguments.get(),
                        argument_values.get(),
                        form,
                        pm_applies,
                        temp_applies,
                    ),
                    tags: tags.get().split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect(),
                    starred: starred.get(),
                    project: opt_str(project.get()),
                    // The adi tools ticked on for this agent — its own `.bin` at launch.
                    bin_tools: form.bin_tools.get().into_iter().collect(),
                    // The secrets ticked on for this agent — injected into its runs as env vars.
                    secrets: form.secrets.get().into_iter()
                        .map(|(project, name)| SecretRef { project, name })
                        .collect(),
                    // Editing with the name field changed is a rename, not a second agent.
                    rename_from: editing.get(),
                };
                editing.set(Some(nm.clone()));
                apply_agents(state, Some(busy), format!("Saved agent “{nm}”."), fetch::save_agent(body));
            }>
                {move || agent_form_fields(state, form)}
                {move || agent_tool_checkboxes(state, form)}
                {move || agent_secret_checkboxes(state, form)}
                <button class="adi-btn adi-btn--primary" type="submit" prop:disabled=move || busy.get()>
                    {move || if editing.get().is_some() { "Update agent" } else { "Create agent" }}
                </button>
            </form>
            {flash_view(flash)}
            <div class="adi-hint">
                "▶ Run launches tmux backends in an interactive " <code>"adi-agent-<name>"</code>
                " session you type into. For headless backends (process / harness) the agent is a
                 template: ▶ Run… starts an independent run from a task you give it — one "
                <code>"--print"</code> " turn, never a continuation — and several can run at once.
                 Each run keeps its own log under "
                <code>"~/.adi/mono/sessions/{process,harness}/<agent>/"</code>
                ", browsable as history in ● View."
            </div>
        </section>
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
        return view! {
            <div class="adi-field adi-field--grow">
                <label class="adi-field__label">"Tools"</label>
                <div class="adi-muted">"Loading tools…"</div>
            </div>
        }
        .into_any();
    };
    let mut tools: Vec<ToolDto> = st.tools.into_iter().filter(|t| !t.is_archived()).collect();
    // System tools first, then by name — the platform CLIs group together.
    tools.sort_by(|a, b| b.system.cmp(&a.system).then_with(|| a.name.cmp(&b.name)));
    if tools.is_empty() {
        return view! {
            <div class="adi-field adi-field--grow">
                <label class="adi-field__label">"Tools"</label>
                <div class="adi-hint">
                    "No tools yet — create one on the Tools page. Whatever you enable here lands "
                    "in this agent's " <code>".bin"</code> "."
                </div>
            </div>
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
            view! {
                <label class="adi-check" title=title
                    style="display:inline-flex; align-items:center; gap:var(--space-1); margin:0 var(--space-3) var(--space-1) 0">
                    <input type="checkbox" prop:checked=checked
                        on:change=move |ev| {
                            let on = event_target_checked(&ev);
                            form.bin_tools.update(|set| {
                                if on { set.insert(id_toggle.clone()); } else { set.remove(&id_toggle); }
                            });
                        } />
                    <span class="adi-mono">{label}</span>
                </label>
            }
        })
        .collect::<Vec<_>>();
    view! {
        <div class="adi-field adi-field--grow">
            <label class="adi-field__label">"Tools (this agent's .bin)"</label>
            <div style="display:flex; flex-wrap:wrap; align-items:center; padding:var(--space-1) 0">
                {boxes}
            </div>
        </div>
    }
    .into_any()
}

/// The per-agent secret checkboxes: one toggle per registered secret (across every scope) that
/// adds/removes its `(scope, name)` reference from the agent's attachment set. Only the ticked
/// secrets are decrypted and injected into the agent's runs as environment variables — an explicit
/// allowlist, so nothing is inherited from a scope for merely existing. Populated from the shared
/// secrets list (`state.secrets`), which carries metadata only (never a value).
fn agent_secret_checkboxes(state: State, form: AgentsForm) -> AnyView {
    let Some(st) = state.secrets.get() else {
        return view! {
            <div class="adi-field adi-field--grow">
                <label class="adi-field__label">"Secrets"</label>
                <div class="adi-muted">"Loading secrets…"</div>
            </div>
        }
        .into_any();
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
            <div class="adi-field adi-field--grow">
                <label class="adi-field__label">"Secrets"</label>
                <div class="adi-hint">
                    "No secrets yet — add one on the Secrets page. Whatever you tick here is "
                    "injected into this agent's runs as an environment variable."
                </div>
            </div>
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
            let label = match s.project.as_deref().filter(|p| !p.trim().is_empty()) {
                Some(p) => format!("{} · {p}", s.name),
                None => s.name.clone(),
            };
            // The value is never sent here; the tooltip carries the description / OAuth provider.
            let title = match (&s.description, &s.oauth) {
                (Some(d), _) if !d.trim().is_empty() => d.clone(),
                (_, Some(o)) => format!("OAuth · {}", o.provider),
                _ => String::new(),
            };
            view! {
                <label class="adi-check" title=title
                    style="display:inline-flex; align-items:center; gap:var(--space-1); margin:0 var(--space-3) var(--space-1) 0">
                    <input type="checkbox" prop:checked=checked
                        on:change=move |ev| {
                            let on = event_target_checked(&ev);
                            form.secrets.update(|set| {
                                if on { set.insert(key_toggle.clone()); } else { set.remove(&key_toggle); }
                            });
                        } />
                    <span class="adi-mono">{label}</span>
                </label>
            }
        })
        .collect::<Vec<_>>();
    view! {
        <div class="adi-field adi-field--grow">
            <label class="adi-field__label">"Secrets (injected as env vars)"</label>
            <div style="display:flex; flex-wrap:wrap; align-items:center; padding:var(--space-1) 0">
                {boxes}
            </div>
        </div>
    }
    .into_any()
}

/// Render the agents table body: a loading/empty placeholder, or one row per agent with Run or
/// View (live session), Code (wasm employees), Edit (loads it into the form), and Delete actions.
fn agent_rows(
    state: State,
    form: AgentsForm,
    watch: AgentsWatch,
    code: AgentCodeEditor,
) -> AnyView {
    let Some(st) = state.agents.get() else {
        return placeholder_row("6", "Loading…");
    };
    if st.agents.is_empty() {
        return placeholder_row("6", "No agents yet — define one below.");
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
            let model = argument_text(&a.arguments, "model");
            let project_cell = match &a.project {
                Some(p) if !p.trim().is_empty() => {
                    let p = p.clone();
                    view! { <span class="adi-chip adi-mono">{p}</span> }.into_any()
                }
                _ => view! { <span class="adi-muted">"—"</span> }.into_any(),
            };
            let tags = a.tags.join(", ");
            let is_wasm = a.executor == "wasm";
            let code_name = a.name.clone();
            let del_name = a.name.clone();
            let a_edit = a.clone();
            view! {
                <tr>
                    <td>{name_disp}</td>
                    <td class="adi-mono">{backend}</td>
                    <td class="adi-mono adi-muted">{model}</td>
                    <td>{project_cell}</td>
                    <td class="adi-muted">{tags}</td>
                    <td class="adi-table__actions">
                        {agent_actions(state, watch, &a)}
                        {is_wasm.then(|| view! {
                            <button class="adi-btn adi-btn--link" title="edit the employee's TypeScript source"
                                on:click=move |_| open_code_editor(state, code, code_name.clone())>"{ } Code"</button>
                            " "
                        })}
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

/// The live view's input row: a text field (submit types it into the session, without a trailing
/// Enter — the ⏎ quick key sends that) plus the special keys interactive TUIs need (Enter, arrows
/// for menus, Esc, Ctrl-C).
fn send_bar(state: State, watch: AgentsWatch) -> impl IntoView {
    view! {
        <form class="adi-form"
            on:submit=move |ev| {
                ev.prevent_default();
                let text = watch.input.get();
                watch.input.set(String::new());
                send_to_agent(state, watch, text, "");
            }>
            <input class="adi-input adi-input--wide adi-mono" autocomplete="off"
                placeholder="type to the agent…"
                prop:value=move || watch.input.get()
                on:input=move |ev| watch.input.set(event_target_value(&ev)) />
            <button class="adi-btn adi-btn--primary" type="submit">"Send"</button>
            {quick_key(state, watch, "⏎", "Enter")}
            {quick_key(state, watch, "↑", "Up")}
            {quick_key(state, watch, "↓", "Down")}
            {quick_key(state, watch, "Tab", "Tab")}
            {quick_key(state, watch, "Esc", "Escape")}
            {quick_key(state, watch, "^C", "C-c")}
        </form>
    }
}

/// One special-key button in the send bar, pressing a single tmux key in the session.
fn quick_key(
    state: State,
    watch: AgentsWatch,
    label: &'static str,
    key: &'static str,
) -> impl IntoView {
    view! {
        <button class="adi-btn adi-btn--ghost adi-mono" type="button"
            title=format!("send {key}")
            on:click=move |_| send_to_agent(state, watch, String::new(), key)>{label}</button>
    }
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
    spawn_local(async move {
        match fetch::send_agent_keys(name, text, key).await {
            Ok(peek) => {
                if watch.name.get_untracked().as_deref() == Some(peek.name.as_str()) {
                    watch.peek.set(Some(peek));
                }
            }
            Err(e) => state.flash.set(Some(Flash::err(e))),
        }
    });
}

/// Trim a form string into an optional, dropping it when blank.
fn opt_str(value: String) -> Option<String> {
    let trimmed = value.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
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
