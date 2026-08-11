//! The Agents page: create, edit, delete, and launch agent definitions (docs/adi-agents.md §5) —
//! pick a backend (`executor:what`), a system prompt, a CLI command scope, and backend-specific
//! params. ▶ Run starts either an interactive pty session or a headless background process;
//! deeper orchestration is future work. The form adapts its params to the chosen backend, and
//! for the `harness:adi` backend also to its chosen provider.

use std::collections::BTreeMap;

use adi_webapp_api::types::{AgentDto, SaveAgent, SecretDto, SecretRef, ToolDto};
use leptos::prelude::*;
use adi_ui::{EmptyRow, Row as TableRow, Table};
use wasm_bindgen_futures::spawn_local;

use crate::fetch;
use crate::state::{AgentsForm, AgentsWatch, Flash, State};
use crate::ui::{
    Key, flash_view, menu_item, row_actions,
    sort_rows, updated_text,
};

/// The Agents page's columns; the trailing blank one holds Run / View / Stop and the kebab.
pub(crate) const COLS: &[&str] = &["Name", "Backend", "Model", "Project", "Tags", ""];

mod actions;
mod form;

use actions::apply_agents;
pub(crate) use actions::{
    CHAT_COLS, CHAT_RUN_COLS, NEWEST_FIRST, RUN_COLS, agent_actions, all_chats_view,
    chat_home_view, live_view, poll_watch, project_run_limit_view, reset_chat_home, run_limit_view,
};
// The onboarding wizard renders the same fields from the same schema, so its half of the form
// lives here rather than as a second copy of these renderers.
pub(crate) use form::{
    agent_argument_values, agent_environment_fields, agent_param_applies, agent_schema_fields,
    load_agent_into_form, parsed_env_vars, parsed_path_dirs, set_agent_field_value,
};
use form::{agent_form_fields, clear_agent_form};

/// The Agents page: create, edit, delete, and launch agent definitions (docs/adi-agents.md §5) —
/// pick a backend (`executor:what`), a system prompt, a CLI command scope, and backend-specific
/// params. ▶ Run starts either an interactive pty session or a headless background process;
/// deeper orchestration is future work. The form adapts its params to the chosen backend, and
/// for the `harness:adi` backend also to its chosen provider.
pub(crate) fn agents_view(state: State, form: AgentsForm, watch: AgentsWatch) -> AnyView {
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
        {all_chats_view(state, watch, None)}

        {move || live_view(state, watch)}

        <section class="adi-panel">
            <div class="adi-panel__head">
                <span class="adi-chip adi-mono" title="Agents defined">
                    {move || agents.get().map_or_else(|| "\u{2014}".to_string(),
                        |a| a.agents.len().to_string())}
                </span>
                {run_limit_view(state)}
                <span class="adi-spacer"></span>
                <span class="adi-updated">{move || updated_text(agents, secs_since)}</span>
            </div>

            <Table state=state.tables.agents>{move || agent_rows(state, form, watch)}</Table>
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
                let spec = agents.get().map(|st| st.form);
                let prov = argument_values.get().get("provider").cloned().unwrap_or_default();
                // Whether each backend-conditional first-class param applies is driven by the
                // server schema (does a field of that name apply to this backend?), so rescoping a
                // field in the API also stops its value being sent for backends it no longer fits.
                let pm_applies = agent_param_applies(spec.as_ref(), &be, &prov, "permission_mode");
                let temp_applies = agent_param_applies(spec.as_ref(), &be, &prov, "temperature");
                let body = SaveAgent {
                    name: nm.clone(),
                    backend: be.clone(),
                    arguments: agent_argument_values(
                        spec.as_ref(),
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
                    // The adi tools ticked on for this agent — its own `.bin` at launch. This is
                    // the form that owns the checkboxes, so it states the set even when empty.
                    bin_tools: Some(form.bin_tools.get().into_iter().collect()),
                    // The secrets ticked on for this agent — injected into its runs as env vars.
                    secrets: form.secrets.get().into_iter()
                        .map(|(project, name)| SecretRef { project, name })
                        .collect(),
                    // This is the one form that edits the run environment, so it always states it —
                    // `Some(empty)` clears, where the `None` other forms send means "leave as is".
                    path: Some(parsed_path_dirs(&form.path.get())),
                    env: Some(parsed_env_vars(&form.env.get())),
                    // …and the same for whether this agent may stop and wait for somebody.
                    unattended: Some(form.unattended.get()),
                    // Editing with the name field changed is a rename, not a second agent.
                    rename_from: editing.get(),
                };
                editing.set(Some(nm.clone()));
                apply_agents(state, Some(busy), format!("Saved agent “{nm}”."), fetch::save_agent(body));
            }>
                {move || agent_form_fields(state, form)}
                {move || agent_tool_checkboxes(state, form)}
                {move || agent_secret_checkboxes(state, form)}
                {move || agent_environment_fields(form)}
                <button class="adi-btn adi-btn--primary" type="submit" prop:disabled=move || busy.get()>
                    {move || if editing.get().is_some() { "Update agent" } else { "Create agent" }}
                </button>
            </form>
            {flash_view(flash)}
            <div class="adi-hint">
                "▶ Run launches pty backends in an interactive " <code>"adi-agent-<name>"</code>
                " session you type into. A "<code>"process"</code>" backend is a template: ▶ Run…
                 starts an independent one-shot run from a task you give it — one "<code>"--print"</code>
                " turn, never continued — and several can run at once. A "<code>"harness"</code>"
                 backend instead starts an answerable "<strong>"conversation"</strong>": ▶ Chat…
                 sends a first message, the agent answers, and you reply to continue the same thread.
                 Each run/conversation keeps its own log + transcript under "
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
            <div class="adi-hint">
                "Each one is asked " <code>"llm help"</code> " (then " <code>"help"</code> ", then "
                <code>"--help"</code> ") at launch, and what it answers is appended to this agent's "
                "system prompt — so it knows what the commands are without being told twice."
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
/// View (live session), Edit (loads it into the form), and Delete actions.
fn agent_rows(state: State, form: AgentsForm, watch: AgentsWatch) -> AnyView {
    let table = state.tables.agents;
    let Some(st) = state.agents.get() else {
        return view! { <EmptyRow state=table>"Loading…"</EmptyRow> }.into_any();
    };
    if st.agents.is_empty() {
        return view! { <EmptyRow state=table>"No agents yet — define one below."</EmptyRow> }.into_any();
    }
    let mut agents = st.agents;
    sort_rows(&mut agents, table.sort.get(), agent_key, |a| {
        Key::text(&a.name)
    });
    agents
        .into_iter()
        .map(|a| {
            let del_name = a.name.clone();
            let a_edit = a.clone();
            // Run/View/Stop stay inline (the live controls); Edit and the destructive Delete
            // move into the kebab.
            let items = vec![
                menu_item(state, "Edit", false, move || load_agent_into_form(form, &a_edit)),
                menu_item(state, "Delete", true, move || {
                    apply_agents(state, None, format!("Deleted {del_name}."),
                        fetch::delete_agent(del_name.clone()));
                }),
            ];
            let actions = row_actions(state, format!("agent:{}", a.name), agent_actions(state, watch, &a), items);
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
        "Backend" => view! { <span class="font-mono">{a.backend.clone()}</span> }.into_any(),
        "Model" => view! {
            <span class="font-mono text-meta">{argument_text(&a.arguments, "model")}</span>
        }
        .into_any(),
        "Project" => match &a.project {
            Some(p) if !p.trim().is_empty() => {
                view! { <span><span class="adi-chip adi-mono">{p.clone()}</span></span> }.into_any()
            }
            _ => view! { <span><span class="adi-muted">"—"</span></span> }.into_any(),
        },
        "Tags" => view! { <span class="text-meta">{a.tags.join(", ")}</span> }.into_any(),
        // "Name", and anything the layout offers that this match doesn't name.
        _ => {
            let name = if a.starred {
                format!("★ {}", a.name)
            } else {
                a.name.clone()
            };
            view! { <span>{name}</span> }.into_any()
        }
    }
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

/// One special-key button in the send bar, pressing a single key in the session.
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
