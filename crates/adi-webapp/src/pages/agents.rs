//! The Agents page: create, edit, and delete agent definitions (docs/adi-agents.md §5) — pick a
//! backend, a system prompt, a tool scope, and backend-specific params. No run/orchestration here;
//! this only edits the stored spec. The form adapts its params to the chosen backend kind.

use std::collections::BTreeMap;

use adi_webapp_api::types::{
    AgentBackendOption, AgentDto, AgentFormField, AgentFormFieldKind, AgentFormOption, AgentsState,
    SaveAgent,
};
use leptos::prelude::*;

use crate::fetch;
use crate::routing::scroll_top;
use crate::state::{AgentsForm, Flash, State};
use crate::ui::{apply_mutation, data_table, flash_view, placeholder_row, tile, updated_text};

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
        extra,
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
                let st = agents.get();
                let kind = agent_backend_kind_from_state(st.as_ref(), &be)
                    .unwrap_or_else(|| agent_backend_kind(&be).to_string());
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
                    extra: agent_extra_values(st.as_ref(), &be, extra.get()),
                };
                editing.set(Some(nm.clone()));
                apply_agents(state, Some(busy), format!("Saved agent “{nm}”."), fetch::save_agent(body));
            }>
                {move || agent_form_fields(state, form)}
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

/// Render the agent form from the server-provided schema.
fn agent_form_fields(state: State, form: AgentsForm) -> AnyView {
    let Some(st) = state.agents.get() else {
        return view! {
            <div class="adi-muted" style="padding:0 0 8px">"Loading agent form..."</div>
        }
        .into_any();
    };
    let backend = form.backend.get();
    let backends = st.form.backends.clone();
    st.form
        .fields
        .into_iter()
        .filter(|field| field_applies(field, &backend))
        .map(|field| render_agent_field(field, backends.clone(), form))
        .collect::<Vec<_>>()
        .into_any()
}

/// Dispatch one schema field to the small renderer for its control kind.
fn render_agent_field(
    field: AgentFormField,
    backends: Vec<AgentBackendOption>,
    form: AgentsForm,
) -> AnyView {
    match field.kind {
        AgentFormFieldKind::Select if field.name == "backend" => {
            render_backend_select(field, backends, form)
        }
        AgentFormFieldKind::Select => render_agent_select(field, form),
        AgentFormFieldKind::Checkbox => render_agent_checkbox(field, form),
        AgentFormFieldKind::Textarea => render_agent_textarea(field, form),
        AgentFormFieldKind::Text | AgentFormFieldKind::Number => {
            render_agent_input(field, backends, form)
        }
    }
}

/// The backend selector: its options and model placeholders are owned by the API.
fn render_backend_select(
    field: AgentFormField,
    backends: Vec<AgentBackendOption>,
    form: AgentsForm,
) -> AnyView {
    let id = field_id(&field.name);
    let label_for = id.clone();
    let label = field.label.clone();
    let first_label = if backends.is_empty() {
        "Loading backends..."
    } else {
        "— pick a backend —"
    };
    view! {
        <div class="adi-field" style=field_style(&field)>
            <label class="adi-field__label" for=label_for>{label}</label>
            <select class="adi-input" id=id
                prop:value=move || form.backend.get()
                on:change=move |ev| form.backend.set(event_target_value(&ev))>
                <option value="">{first_label}</option>
                {backends.into_iter().map(|backend| {
                    let id = backend.id;
                    let label = backend.label;
                    view! { <option value=id>{label}</option> }
                }).collect::<Vec<_>>()}
            </select>
        </div>
    }
    .into_any()
}

/// Render a server-described select bound to either a first-class form signal or `extra`.
fn render_agent_select(field: AgentFormField, form: AgentsForm) -> AnyView {
    let id = field_id(&field.name);
    let label_for = id.clone();
    let label = field.label.clone();
    let name_for_value = field.name.clone();
    let name_for_change = field.name.clone();
    let options = field.options.clone();
    view! {
        <div class="adi-field" style=field_style(&field)>
            <label class="adi-field__label" for=label_for>{label}</label>
            <select class="adi-input" id=id
                prop:value=move || agent_field_value(form, &name_for_value)
                on:change=move |ev| set_agent_field_value(form, &name_for_change, event_target_value(&ev))>
                {options.into_iter().map(|opt| option_view(opt)).collect::<Vec<_>>()}
            </select>
        </div>
    }
    .into_any()
}

/// Render a text/number input bound to either a first-class form signal or `extra`.
fn render_agent_input(
    field: AgentFormField,
    backends: Vec<AgentBackendOption>,
    form: AgentsForm,
) -> AnyView {
    let id = field_id(&field.name);
    let label_for = id.clone();
    let label = field.label.clone();
    let placeholder = field_placeholder(&field, &backends, &form.backend.get());
    let class = field_class(&field);
    let style = field_style(&field);
    let inputmode = if field.numeric || matches!(field.kind, AgentFormFieldKind::Number) {
        "numeric"
    } else {
        "text"
    };
    let hint = field.hint.clone();
    let show_hint = !hint.is_empty();
    let name_for_value = field.name.clone();
    let name_for_input = field.name.clone();
    view! {
        <div class="adi-field" style=style>
            <label class="adi-field__label" for=label_for>{label}</label>
            <input class=class id=id placeholder=placeholder autocomplete="off" inputmode=inputmode
                prop:value=move || agent_field_value(form, &name_for_value)
                on:input=move |ev| set_agent_field_value(form, &name_for_input, event_target_value(&ev)) />
            {show_hint.then(|| view! { <span class="adi-field__hint">{hint}</span> })}
        </div>
    }
    .into_any()
}

/// Render a checkbox. `starred` is first-class; any other checkbox is stored as an extra bool.
fn render_agent_checkbox(field: AgentFormField, form: AgentsForm) -> AnyView {
    let label = field.label.clone();
    let name_for_value = field.name.clone();
    let name_for_change = field.name.clone();
    view! {
        <label class="adi-field" style="flex-direction:row; align-items:center; gap:7px; align-self:center">
            <input type="checkbox"
                prop:checked=move || agent_field_bool(form, &name_for_value)
                on:change=move |ev| set_agent_field_bool(form, &name_for_change, event_target_checked(&ev)) />
            <span class="adi-field__label" style="margin:0">{label}</span>
        </label>
    }
    .into_any()
}

/// Render a textarea bound to either a first-class form signal or `extra`.
fn render_agent_textarea(field: AgentFormField, form: AgentsForm) -> AnyView {
    let id = field_id(&field.name);
    let label_for = id.clone();
    let label = field.label.clone();
    let placeholder = field.placeholder.clone();
    let style = field_style(&field);
    let name_for_value = field.name.clone();
    let name_for_input = field.name.clone();
    view! {
        <div class="adi-field" style=style>
            <label class="adi-field__label" for=label_for>{label}</label>
            <textarea class="adi-textarea" id=id placeholder=placeholder
                prop:value=move || agent_field_value(form, &name_for_value)
                on:input=move |ev| set_agent_field_value(form, &name_for_input, event_target_value(&ev))></textarea>
        </div>
    }
    .into_any()
}

fn option_view(opt: AgentFormOption) -> impl IntoView {
    let value = opt.value;
    let label = opt.label;
    view! { <option value=value>{label}</option> }
}

fn field_applies(field: &AgentFormField, backend: &str) -> bool {
    if field.backend_ids.is_empty() && field.backend_kinds.is_empty() {
        return true;
    }
    if backend.is_empty() {
        return false;
    }
    field.backend_ids.iter().any(|id| id == backend)
        || field
            .backend_kinds
            .iter()
            .any(|kind| kind == agent_backend_kind(backend))
}

fn field_id(name: &str) -> String {
    format!("agent-{}", name.replace('_', "-"))
}

fn field_style(field: &AgentFormField) -> String {
    if field.wide || matches!(field.kind, AgentFormFieldKind::Textarea) {
        "flex:1 1 100%; min-width:0".into()
    } else {
        String::new()
    }
}

fn field_class(field: &AgentFormField) -> String {
    let mut class = String::from("adi-input");
    if field.wide {
        class.push_str(" adi-input--wide");
    }
    if field.mono {
        class.push_str(" adi-mono");
    }
    class
}

fn field_placeholder(
    field: &AgentFormField,
    backends: &[AgentBackendOption],
    backend: &str,
) -> String {
    if field.name == "model"
        && let Some(selected) = selected_backend(backends, backend)
        && !selected.model_placeholder.is_empty()
    {
        return selected.model_placeholder.clone();
    }
    field.placeholder.clone()
}

fn selected_backend<'a>(
    backends: &'a [AgentBackendOption],
    backend: &str,
) -> Option<&'a AgentBackendOption> {
    backends.iter().find(|b| b.id == backend)
}

fn agent_backend_kind_from_state(st: Option<&AgentsState>, backend: &str) -> Option<String> {
    st.and_then(|st| selected_backend(&st.form.backends, backend).map(|b| b.kind.clone()))
}

fn agent_field_value(form: AgentsForm, name: &str) -> String {
    match name {
        "name" => form.name.get(),
        "backend" => form.backend.get(),
        "model" => form.model.get(),
        "permission_mode" => form.permission_mode.get(),
        "temperature" => form.temperature.get(),
        "max_turns" => form.max_turns.get(),
        "tags" => form.tags.get(),
        "tools" => form.tools.get(),
        "system_prompt" => form.system_prompt.get(),
        other => form.extra.get().get(other).cloned().unwrap_or_default(),
    }
}

fn set_agent_field_value(form: AgentsForm, name: &str, value: String) {
    match name {
        "name" => form.name.set(value),
        "backend" => form.backend.set(value),
        "model" => form.model.set(value),
        "permission_mode" => form.permission_mode.set(value),
        "temperature" => form.temperature.set(value),
        "max_turns" => form.max_turns.set(value),
        "tags" => form.tags.set(value),
        "tools" => form.tools.set(value),
        "system_prompt" => form.system_prompt.set(value),
        other => set_agent_extra(form.extra, other, value),
    }
}

fn agent_field_bool(form: AgentsForm, name: &str) -> bool {
    match name {
        "starred" => form.starred.get(),
        other => form.extra.get().get(other).is_some_and(|v| v == "true"),
    }
}

fn set_agent_field_bool(form: AgentsForm, name: &str, value: bool) {
    match name {
        "starred" => form.starred.set(value),
        other => set_agent_extra(
            form.extra,
            other,
            if value { "true".into() } else { String::new() },
        ),
    }
}

fn set_agent_extra(extra: RwSignal<BTreeMap<String, String>>, name: &str, value: String) {
    extra.update(|values| {
        if value.is_empty() {
            values.remove(name);
        } else {
            values.insert(name.to_string(), value);
        }
    });
}

fn agent_extra_values(
    st: Option<&AgentsState>,
    backend: &str,
    values: BTreeMap<String, String>,
) -> BTreeMap<String, String> {
    values
        .into_iter()
        .map(|(k, v)| (k.trim().to_string(), v.trim().to_string()))
        .filter(|(k, v)| {
            !k.is_empty()
                && !v.is_empty()
                && st.is_none_or(|st| {
                    st.form.fields.iter().any(|field| {
                        field.name == *k
                            && is_extra_field(&field.name)
                            && field_applies(field, backend)
                    })
                })
        })
        .collect()
}

fn is_extra_field(name: &str) -> bool {
    !matches!(
        name,
        "name"
            | "backend"
            | "model"
            | "permission_mode"
            | "temperature"
            | "max_turns"
            | "tags"
            | "tools"
            | "system_prompt"
            | "starred"
    )
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
    form.extra.set(a.extra.clone());
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
    form.extra.set(BTreeMap::new());
    form.editing.set(None);
}

/// The backend kind (`cli`/`api`) — the part before the `:` in a backend id; `""` if none.
fn agent_backend_kind(backend: &str) -> &str {
    match backend.split_once(':') {
        Some((kind, _)) => kind,
        None => "",
    }
}

/// Trim a form string into an optional, dropping it when blank.
fn opt_str(value: String) -> Option<String> {
    let trimmed = value.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}
