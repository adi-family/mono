//! The Agents page create/edit form: the server-driven field schema and its renderers.

use std::collections::BTreeMap;

use adi_webapp_api::types::{
    AgentBackendOption, AgentDto, AgentFormField, AgentFormFieldKind, AgentFormOption, AgentsState,
};
use leptos::prelude::*;

use crate::routing::scroll_top;
use crate::state::{AgentsForm, State};
use crate::ui::field_hint;

use super::{argument_text, scalar_argument_text};

/// The `harness:adi` backend id — the one whose form fields are additionally scoped to the
/// `provider` argument. Must match the id served by the API's form spec.
const ADI_HARNESS: &str = "harness:adi";

/// Render the agent form from the server-provided schema.
pub(crate) fn agent_form_fields(state: State, form: AgentsForm) -> AnyView {
    let Some(st) = state.agents.get() else {
        return view! {
            <div class="adi-muted" style="padding:0 0 var(--space-2)">"Loading agent form..."</div>
        }
        .into_any();
    };
    let backend = form.backend.get();
    let provider = form
        .argument_values
        .get()
        .get("provider")
        .cloned()
        .unwrap_or_default();
    let backends = st.form.backends.clone();
    st.form
        .fields
        .into_iter()
        .filter(|field| field_applies(field, &backend, &provider))
        .map(|field| render_agent_field(field, backends.clone(), state, form))
        .collect::<Vec<_>>()
        .into_any()
}

/// Dispatch one schema field to the small renderer for its control kind.
fn render_agent_field(
    field: AgentFormField,
    backends: Vec<AgentBackendOption>,
    state: State,
    form: AgentsForm,
) -> AnyView {
    match field.kind {
        AgentFormFieldKind::Select if field.name == "backend" => {
            render_backend_select(field, backends, form)
        }
        AgentFormFieldKind::Select if field.name == "project" => {
            render_project_select(field, state, form)
        }
        AgentFormFieldKind::Select => render_agent_select(field, form),
        AgentFormFieldKind::Checkbox => render_agent_checkbox(field, form),
        AgentFormFieldKind::Textarea => render_agent_textarea(field, form),
        AgentFormFieldKind::ToolPicker => render_agent_tools(field, form),
        AgentFormFieldKind::ModelPicker => render_agent_model(field, backends, form),
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

/// The project selector: the schema names the field, but its options are the registered
/// projects, which only the client knows live — filled from the projects state (as the
/// Triggers form does). An empty value files the agent globally.
fn render_project_select(field: AgentFormField, state: State, form: AgentsForm) -> AnyView {
    let id = field_id(&field.name);
    let label_for = id.clone();
    let label = field.label.clone();
    let hint = field.hint.clone();
    let show_hint = !hint.is_empty();
    let projects = state.projects;
    view! {
        <div class="adi-field" style=field_style(&field)>
            <label class="adi-field__label" for=label_for>{label}</label>
            <select class="adi-input" id=id
                prop:value=move || form.project.get()
                on:change=move |ev| form.project.set(event_target_value(&ev))>
                <option value="">"— global —"</option>
                {move || projects.get().map(|p| p.projects.into_iter()
                    .filter(|proj| !proj.is_archived())
                    .map(|proj| {
                        let id = proj.id.clone();
                        let label = proj.name.clone();
                        view! { <option value=id>{label}</option> }
                    }).collect::<Vec<_>>()).unwrap_or_default()}
            </select>
            {show_hint.then(|| field_hint(hint))}
        </div>
    }
    .into_any()
}

/// Render a server-described select bound to a backend argument form value.
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

/// Render a text/number input bound to a backend argument form value.
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
            {show_hint.then(|| field_hint(hint))}
        </div>
    }
    .into_any()
}

/// Render a checkbox. `starred` is ADI metadata; every other checkbox is a backend argument.
fn render_agent_checkbox(field: AgentFormField, form: AgentsForm) -> AnyView {
    let label = field.label.clone();
    let name_for_value = field.name.clone();
    let name_for_change = field.name.clone();
    view! {
        <label class="adi-field adi-field--check">
            <input type="checkbox"
                prop:checked=move || agent_field_bool(form, &name_for_value)
                on:change=move |ev| set_agent_field_bool(form, &name_for_change, event_target_checked(&ev)) />
            <span class="adi-field__label">{label}</span>
        </label>
    }
    .into_any()
}

/// Render a textarea bound to a backend argument form value.
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

/// Render the tool picker: a row of toggle chips for the well-known tools over a free-text input,
/// both editing the one space-separated tool spec. The chips manage bare tool names; the input
/// stays authoritative for everything, so scoped rules like `Bash(git *)` are typed there and the
/// chips leave them untouched.
fn render_agent_tools(field: AgentFormField, form: AgentsForm) -> AnyView {
    let id = field_id(&field.name);
    let label_for = id.clone();
    let label = field.label.clone();
    let placeholder = field.placeholder.clone();
    let hint = field.hint.clone();
    let show_hint = !hint.is_empty();
    let options = field.options.clone();
    let name = field.name.clone();
    let name_value = name.clone();
    let name_input = name.clone();
    view! {
        <div class="adi-field" style="flex:1 1 100%; min-width:0">
            <label class="adi-field__label" for=label_for>{label}</label>
            <div class="adi-toolpick">
                {options.into_iter().map(|opt| {
                    let tool = opt.value;
                    let text = opt.label;
                    let name_pressed = name.clone();
                    let name_toggle = name.clone();
                    let tool_pressed = tool.clone();
                    view! {
                        <button type="button" class="adi-toolpick__chip"
                            aria-pressed=move || tool_selected(&agent_field_value(form, &name_pressed), &tool_pressed).to_string()
                            on:click=move |_| {
                                let next = toggle_tool(&agent_field_value(form, &name_toggle), &tool);
                                set_agent_field_value(form, &name_toggle, next);
                            }>
                            {text}
                        </button>
                    }
                }).collect::<Vec<_>>()}
            </div>
            <input class="adi-input adi-input--wide adi-mono" id=id placeholder=placeholder autocomplete="off"
                prop:value=move || agent_field_value(form, &name_value)
                on:input=move |ev| set_agent_field_value(form, &name_input, event_target_value(&ev)) />
            {show_hint.then(|| field_hint(hint))}
        </div>
    }
    .into_any()
}

/// Split a space-separated tool spec into tokens, keeping a parenthesised specifier (e.g.
/// `Bash(git *)`, which itself contains spaces) whole.
fn split_tool_tokens(spec: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut depth = 0i32;
    for ch in spec.chars() {
        match ch {
            '(' => {
                depth += 1;
                current.push(ch);
            }
            ')' => {
                depth = (depth - 1).max(0);
                current.push(ch);
            }
            c if c.is_whitespace() && depth == 0 => {
                if !current.is_empty() {
                    tokens.push(std::mem::take(&mut current));
                }
            }
            c => current.push(c),
        }
    }
    if !current.is_empty() {
        tokens.push(current);
    }
    tokens
}

/// Whether the bare tool token `tool` is present in `spec` — its exact name, not a scoped
/// `Tool(...)` specifier, which the chip deliberately leaves to the text input.
fn tool_selected(spec: &str, tool: &str) -> bool {
    split_tool_tokens(spec).iter().any(|t| t == tool)
}

/// Toggle the bare `tool` token in `spec`, keeping every other token (scoped rules included) and
/// their order. Returns the new space-joined spec.
fn toggle_tool(spec: &str, tool: &str) -> String {
    let mut tokens = split_tool_tokens(spec);
    if let Some(pos) = tokens.iter().position(|t| t == tool) {
        tokens.remove(pos);
    } else {
        tokens.push(tool.to_string());
    }
    tokens.join(" ")
}

/// Render the Model field as single-select suggestion chips over its free-text input. The chips
/// are the selected backend's `model_suggestions`; clicking one sets the model, clicking the
/// active one clears it (back to the backend default). Any other model is still typed by hand,
/// and the input keeps the backend-specific placeholder.
fn render_agent_model(
    field: AgentFormField,
    backends: Vec<AgentBackendOption>,
    form: AgentsForm,
) -> AnyView {
    let id = field_id(&field.name);
    let label_for = id.clone();
    let label = field.label.clone();
    let hint = field.hint.clone();
    let show_hint = !hint.is_empty();
    let backend = form.backend.get();
    let placeholder = field_placeholder(&field, &backends, &backend);
    let class = field_class(&field);
    let suggestions = selected_backend(&backends, &backend)
        .map(|b| b.model_suggestions.clone())
        .unwrap_or_default();
    view! {
        <div class="adi-field" style=field_style(&field)>
            <label class="adi-field__label" for=label_for>{label}</label>
            {(!suggestions.is_empty()).then(|| view! {
                <div class="adi-toolpick">
                    {suggestions.into_iter().map(|model| {
                        let for_pressed = model.clone();
                        let for_click = model.clone();
                        view! {
                            <button type="button" class="adi-toolpick__chip"
                                aria-pressed=move || (form.model.get() == for_pressed).to_string()
                                on:click=move |_| {
                                    if form.model.get() == for_click {
                                        form.model.set(String::new());
                                    } else {
                                        form.model.set(for_click.clone());
                                    }
                                }>
                                {model}
                            </button>
                        }
                    }).collect::<Vec<_>>()}
                </div>
            })}
            <input class=class id=id placeholder=placeholder autocomplete="off"
                prop:value=move || form.model.get()
                on:input=move |ev| form.model.set(event_target_value(&ev)) />
            {show_hint.then(|| field_hint(hint))}
        </div>
    }
    .into_any()
}

fn option_view(opt: AgentFormOption) -> impl IntoView {
    let value = opt.value;
    let label = opt.label;
    view! { <option value=value>{label}</option> }
}

/// Whether a schema field is visible for the chosen backend. A field with no filters is always
/// visible; otherwise it shows on a backend-id match, an executor match, or — for the adi
/// harness only — a match on its chosen provider.
fn field_applies(field: &AgentFormField, backend: &str, provider: &str) -> bool {
    if field.backend_ids.is_empty() && field.executors.is_empty() && field.providers.is_empty() {
        return true;
    }
    if backend.is_empty() {
        return false;
    }
    field.backend_ids.iter().any(|id| id == backend)
        || field
            .executors
            .iter()
            .any(|executor| executor == agent_executor(backend))
        || (backend == ADI_HARNESS
            && !provider.is_empty()
            && field.providers.iter().any(|p| p == provider))
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

/// Whether the schema exposes a field named `name` for `backend` (and provider). The submit uses
/// this to decide whether a backend-conditional first-class param (`permission_mode` /
/// `temperature`) applies to the chosen backend, keeping the gating in sync with the
/// server-owned field scoping.
pub(crate) fn agent_param_applies(
    st: Option<&AgentsState>,
    backend: &str,
    provider: &str,
    name: &str,
) -> bool {
    st.is_some_and(|st| {
        st.form
            .fields
            .iter()
            .any(|f| f.name == name && field_applies(f, backend, provider))
    })
}

fn agent_field_value(form: AgentsForm, name: &str) -> String {
    match name {
        "name" => form.name.get(),
        "backend" => form.backend.get(),
        "project" => form.project.get(),
        "model" => form.model.get(),
        "permission_mode" => form.permission_mode.get(),
        "temperature" => form.temperature.get(),
        "max_turns" => form.max_turns.get(),
        "tags" => form.tags.get(),
        "tools" => form.tools.get(),
        "system_prompt" => form.system_prompt.get(),
        other => form
            .argument_values
            .get()
            .get(other)
            .cloned()
            .unwrap_or_default(),
    }
}

fn set_agent_field_value(form: AgentsForm, name: &str, value: String) {
    match name {
        "name" => form.name.set(value),
        "backend" => form.backend.set(value),
        "project" => form.project.set(value),
        "model" => form.model.set(value),
        "permission_mode" => form.permission_mode.set(value),
        "temperature" => form.temperature.set(value),
        "max_turns" => form.max_turns.set(value),
        "tags" => form.tags.set(value),
        "tools" => form.tools.set(value),
        "system_prompt" => form.system_prompt.set(value),
        other => set_agent_argument_value(form.argument_values, other, value),
    }
}

fn agent_field_bool(form: AgentsForm, name: &str) -> bool {
    match name {
        "starred" => form.starred.get(),
        other => form
            .argument_values
            .get()
            .get(other)
            .is_some_and(|v| v == "true"),
    }
}

fn set_agent_field_bool(form: AgentsForm, name: &str, value: bool) {
    match name {
        "starred" => form.starred.set(value),
        other => set_agent_argument_value(
            form.argument_values,
            other,
            if value { "true".into() } else { String::new() },
        ),
    }
}

fn set_agent_argument_value(
    argument_values: RwSignal<BTreeMap<String, String>>,
    name: &str,
    value: String,
) {
    argument_values.update(|values| {
        if value.is_empty() {
            values.remove(name);
        } else {
            values.insert(name.to_string(), value);
        }
    });
}

pub(crate) fn agent_argument_values(
    st: Option<&AgentsState>,
    backend: &str,
    mut arguments: BTreeMap<String, serde_json::Value>,
    scalar_values: BTreeMap<String, String>,
    form: AgentsForm,
    permission_mode_applies: bool,
    temperature_applies: bool,
) -> BTreeMap<String, serde_json::Value> {
    let provider = scalar_values.get("provider").cloned().unwrap_or_default();

    // Remove every form-owned argument before rebuilding the values that apply to this backend.
    // Unknown structured arguments remain untouched so editing does not flatten backend manifests.
    for name in [
        "system_prompt",
        "tools",
        "model",
        "permission_mode",
        "temperature",
        "max_turns",
    ] {
        arguments.remove(name);
    }
    for name in scalar_values.keys() {
        arguments.remove(name);
    }
    if let Some(st) = st {
        for field in &st.form.fields {
            if is_argument_field(&field.name) {
                arguments.remove(&field.name);
            }
        }
    }

    insert_text_argument(
        &mut arguments,
        "system_prompt",
        form.system_prompt.get(),
        false,
    );
    insert_text_argument(&mut arguments, "tools", form.tools.get(), true);
    insert_text_argument(&mut arguments, "model", form.model.get(), true);
    if permission_mode_applies {
        insert_text_argument(
            &mut arguments,
            "permission_mode",
            form.permission_mode.get(),
            true,
        );
    }
    if temperature_applies && let Ok(value) = form.temperature.get().trim().parse::<f64>() {
        arguments.insert("temperature".into(), value.into());
    }
    if let Ok(value) = form.max_turns.get().trim().parse::<u64>() {
        arguments.insert("max_turns".into(), value.into());
    }

    for (name, value) in scalar_values {
        let name = name.trim().to_string();
        let value = value.trim().to_string();
        if name.is_empty() || value.is_empty() {
            continue;
        }
        let field = st.and_then(|st| {
            st.form.fields.iter().find(|field| {
                field.name == name
                    && is_scalar_argument_field(&field.name)
                    && field_applies(field, backend, &provider)
            })
        });
        let Some(field) = field else {
            continue;
        };
        let value = match field.kind {
            AgentFormFieldKind::Checkbox => serde_json::Value::Bool(value == "true"),
            AgentFormFieldKind::Number => match value.parse::<f64>() {
                Ok(value) => value.into(),
                Err(_) => continue,
            },
            _ => value.into(),
        };
        arguments.insert(name, value);
    }
    arguments
}

fn insert_text_argument(
    arguments: &mut BTreeMap<String, serde_json::Value>,
    name: &str,
    mut value: String,
    trim: bool,
) {
    if trim {
        value = value.trim().to_string();
    }
    if !value.is_empty() {
        arguments.insert(name.to_string(), value.into());
    }
}

fn is_argument_field(name: &str) -> bool {
    !matches!(name, "name" | "backend" | "project" | "tags" | "starred")
}

fn is_scalar_argument_field(name: &str) -> bool {
    !matches!(
        name,
        "name"
            | "backend"
            | "project"
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

/// Load an existing agent into the create/edit form (the Edit action).
pub(crate) fn load_agent_into_form(form: AgentsForm, a: &AgentDto) {
    form.name.set(a.name.clone());
    form.backend.set(a.backend.clone());
    form.project.set(a.project.clone().unwrap_or_default());
    form.model.set(argument_text(&a.arguments, "model"));
    form.permission_mode
        .set(argument_text(&a.arguments, "permission_mode"));
    form.temperature
        .set(argument_text(&a.arguments, "temperature"));
    form.max_turns.set(argument_text(&a.arguments, "max_turns"));
    form.tags.set(a.tags.join(", "));
    form.tools.set(argument_text(&a.arguments, "tools"));
    form.bin_tools.set(a.bin_tools.iter().cloned().collect());
    form.system_prompt
        .set(argument_text(&a.arguments, "system_prompt"));
    form.starred.set(a.starred);
    form.arguments.set(a.arguments.clone());
    form.argument_values.set(
        a.arguments
            .iter()
            .filter(|(name, _)| is_scalar_argument_field(name))
            .filter_map(|(name, value)| {
                scalar_argument_text(value).map(|value| (name.clone(), value))
            })
            .collect(),
    );
    form.editing.set(Some(a.name.clone()));
    scroll_top();
}

/// Reset the create/edit form back to a blank "New agent" state.
pub(crate) fn clear_agent_form(form: AgentsForm) {
    form.name.set(String::new());
    form.backend.set(String::new());
    form.project.set(String::new());
    form.model.set(String::new());
    form.permission_mode.set(String::new());
    form.temperature.set(String::new());
    form.max_turns.set(String::new());
    form.tags.set(String::new());
    form.tools.set(String::new());
    form.bin_tools.set(std::collections::BTreeSet::new());
    form.system_prompt.set(String::new());
    form.starred.set(false);
    form.arguments.set(BTreeMap::new());
    form.argument_values.set(BTreeMap::new());
    form.editing.set(None);
}

/// The executor (`tmux`/`process`/`harness`) — the part before the `:` in a backend id; `""` if
/// none.
fn agent_executor(backend: &str) -> &str {
    match backend.split_once(':') {
        Some((executor, _)) => executor,
        None => "",
    }
}
