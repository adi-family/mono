//! The Agents page: create, edit, delete, and launch agent definitions (docs/adi-agents.md §5) —
//! pick a backend (`executor:what`), a system prompt, a CLI command scope, and backend-specific
//! params. ▶ Run starts either an interactive tmux session or a headless background process;
//! deeper orchestration is future work. The form adapts its params to the chosen backend, and
//! for the `harness:adi` backend also to its chosen provider.

use std::collections::BTreeMap;

use adi_webapp_api::types::{
    AgentBackendOption, AgentDto, AgentFormField, AgentFormFieldKind, AgentFormOption, AgentsState,
    SaveAgent,
};
use leptos::prelude::*;
use wasm_bindgen_futures::spawn_local;

use crate::fetch;
use crate::routing::scroll_top;
use crate::state::{AgentCodeEditor, AgentsForm, AgentsWatch, Flash, State};
use crate::ui::{apply_mutation, data_table, flash_view, placeholder_row, tile, updated_text};

/// The `harness:adi` backend id — the one whose form fields are additionally scoped to the
/// `provider` argument. Must match the id served by the API's form spec.
const ADI_HARNESS: &str = "harness:adi";

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
        {agent_tiles(state)}

        {move || live_view(state, watch)}

        {move || code_editor_view(state, code)}

        <section class="adi-panel">
            <div class="adi-panel__head">
                <h2 class="adi-panel__title">"Agent definitions"</h2>
                <span class="adi-updated">{move || updated_text(state.ports, secs_since)}</span>
            </div>

            {data_table(&["Name", "Backend", "Model", "Project", "Tags", ""], move || agent_rows(state, form, watch, code))}

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
                "▶ Run launches tmux backends in an interactive " <code>"adi-agent-<name>"</code>
                " session, or process backends as headless Claude/Codex CLI jobs. ● View is tmux
                 only; background-process output is written under "
                <code>"~/.adi/mono/sessions/process"</code> "."
            </div>
        </section>
    }
    .into_any()
}

/// The stat-tile strip: totals, per-executor counts, starred, and live runs.
fn agent_tiles(state: State) -> impl IntoView {
    let agents = state.agents;
    view! {
        <section class="adi-tiles">
            {tile("Agents",
                move || agents.get().map_or_else(|| "—".to_string(), |a| a.agents.len().to_string()),
                "defined")}
            {tile("tmux",
                move || agents.get().map_or_else(|| "—".to_string(), |a| agent_count_executor(&a, "tmux").to_string()),
                "vendor CLI in a session")}
            {tile("process",
                move || agents.get().map_or_else(|| "—".to_string(), |a| agent_count_executor(&a, "process").to_string()),
                "headless vendor CLI")}
            {tile("harness",
                move || agents.get().map_or_else(|| "—".to_string(), |a| agent_count_executor(&a, "harness").to_string()),
                "agentic loop (SDK / ADI)")}
            {tile("wasm",
                move || agents.get().map_or_else(|| "—".to_string(), |a| agent_count_executor(&a, "wasm").to_string()),
                "workforce employees")}
            {tile("Starred",
                move || agents.get().map_or_else(|| "—".to_string(), |a| agent_starred(&a).to_string()),
                "pinned")}
            {tile("Running",
                move || agents.get().map_or_else(|| "—".to_string(), |a| agent_running(&a).to_string()),
                "live sessions / processes")}
        </section>
    }
}

/// Count agents whose executor (`tmux`/`process`/`harness`/`wasm`) matches.
fn agent_count_executor(st: &AgentsState, executor: &str) -> usize {
    st.agents.iter().filter(|a| a.executor == executor).count()
}

/// Count starred agents.
fn agent_starred(st: &AgentsState) -> usize {
    st.agents.iter().filter(|a| a.starred).count()
}

/// Count agents with a live tmux session or detached process.
fn agent_running(st: &AgentsState) -> usize {
    st.agents.iter().filter(|a| a.running).count()
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
            {show_hint.then(|| view! { <span class="adi-field__hint">{hint}</span> })}
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
            {show_hint.then(|| view! { <span class="adi-field__hint">{hint}</span> })}
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
        <label class="adi-field" style="flex-direction:row; align-items:center; gap:7px; align-self:center">
            <input type="checkbox"
                prop:checked=move || agent_field_bool(form, &name_for_value)
                on:change=move |ev| set_agent_field_bool(form, &name_for_change, event_target_checked(&ev)) />
            <span class="adi-field__label" style="margin:0">{label}</span>
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
fn agent_param_applies(
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

fn agent_argument_values(
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

/// Render the agents table body: a loading/empty placeholder, or one row per agent with Run or
/// View (live session), Code (wasm employees), Edit (loads it into the form), and Delete actions.
fn agent_rows(state: State, form: AgentsForm, watch: AgentsWatch, code: AgentCodeEditor) -> AnyView {
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
                    <td style="text-align:right; white-space:nowrap">
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

/// The employee-code editor panel (the `{ } Code` action on a wasm agent's row): a textarea
/// over the agent's `src` file with Save / Build / Reload / Close, plus the last build's
/// output. `None` while closed.
fn code_editor_view(state: State, code: AgentCodeEditor) -> Option<AnyView> {
    let name = code.open.get()?;
    let dirty = move || code.buffer.get() != code.original.get();
    let build_name = name.clone();
    let reload_name = name.clone();
    let save_name = name.clone();
    view! {
        <section class="adi-panel">
            <div class="adi-panel__head">
                <h2 class="adi-panel__title">{format!("Employee code — {name}")}</h2>
                <span class="adi-updated">"TypeScript → esbuild → jco → WASM component"</span>
            </div>
            <div class="adi-form" style="justify-content:flex-start; align-items:center">
                <span class="adi-chip adi-mono">{move || code.path.get()}</span>
                <span class="adi-muted" style="font-size:13px">
                    {move || if dirty() { "unsaved changes".to_string() } else { "saved".to_string() }}
                </span>
                <span class="adi-spacer" style="flex:1"></span>
                <button class="adi-btn adi-btn--primary" type="button"
                    prop:disabled=move || code.busy.get() || !dirty()
                    on:click=move |_| save_code(state, code, save_name.clone())>"Save"</button>
                <button class="adi-btn adi-btn--primary" type="button"
                    title="save if needed, then compile the source to its component"
                    prop:disabled=move || code.busy.get()
                    on:click=move |_| build_code(state, code, build_name.clone())>"⚙ Build"</button>
                <button class="adi-btn adi-btn--ghost" type="button"
                    prop:disabled=move || code.busy.get()
                    on:click=move |_| open_code_editor(state, code, reload_name.clone())>"Reload"</button>
                <button class="adi-btn adi-btn--link" type="button"
                    on:click=move |_| code.close()>"Close"</button>
            </div>
            <div class="adi-panel__body">
                <textarea class="adi-textarea adi-mono" spellcheck="false" autocomplete="off"
                    prop:value=move || code.buffer.get()
                    on:input=move |ev| code.buffer.set(event_target_value(&ev))></textarea>
                {move || code.build.get().map(|(ok, output)| view! {
                    <div class="adi-muted" style="font-size:13px; padding:8px 0 4px">
                        {if ok { "build succeeded" } else { "build failed" }}
                    </div>
                    <pre class="adi-term">{output}</pre>
                })}
            </div>
        </section>
    }
    .into_any()
    .into()
}

/// Open (or reload) the employee-code editor on a wasm agent: fetch the `src` file through the
/// agent code API into the buffer, then scroll up to where the panel renders.
fn open_code_editor(state: State, code: AgentCodeEditor, name: String) {
    code.busy.set(true);
    scroll_top();
    spawn_local(async move {
        match fetch::agent_code(name).await {
            Ok(c) => {
                code.open.set(Some(c.name));
                code.path.set(c.path);
                code.original.set(c.code.clone());
                code.buffer.set(c.code);
                code.build.set(None);
            }
            Err(e) => state.flash.set(Some(Flash::err(e))),
        }
        code.busy.set(false);
    });
}

/// Save the code editor's buffer back to the agent's `src` file (the Save action).
fn save_code(state: State, code: AgentCodeEditor, name: String) {
    let content = code.buffer.get_untracked();
    code.busy.set(true);
    spawn_local(async move {
        match fetch::save_agent_code(name, content).await {
            Ok(c) => {
                code.original.set(c.code);
                state.flash.set(Some(Flash::ok(format!("Saved {}.", c.path))));
            }
            Err(e) => state.flash.set(Some(Flash::err(e))),
        }
        code.busy.set(false);
    });
}

/// Compile the source to its component (the ⚙ Build action): save the buffer first when dirty,
/// then run the server-side build and show its output under the editor. A successful first
/// build fills the agent's `wasm` argument, so the fresh state lands in the list too.
fn build_code(state: State, code: AgentCodeEditor, name: String) {
    let content = code.buffer.get_untracked();
    let dirty = content != code.original.get_untracked();
    code.busy.set(true);
    spawn_local(async move {
        if dirty {
            match fetch::save_agent_code(name.clone(), content).await {
                Ok(c) => code.original.set(c.code),
                Err(e) => {
                    state.flash.set(Some(Flash::err(e)));
                    code.busy.set(false);
                    return;
                }
            }
        }
        match fetch::build_agent(name).await {
            Ok(res) => {
                state.agents.set(Some(res.state));
                code.build.set(Some((res.ok, res.output)));
                state.flash.set(Some(if res.ok {
                    Flash::ok(format!("Built {}.", res.wasm))
                } else {
                    Flash::err("Build failed — see the output below.".to_string())
                }));
            }
            Err(e) => state.flash.set(Some(Flash::err(e))),
        }
        code.busy.set(false);
    });
}

/// The Run / View / Stop action buttons for one agent row. Process runs are deliberately
/// non-interactive, so only tmux runs expose View; both kinds can be stopped.
pub(crate) fn agent_actions(state: State, watch: AgentsWatch, a: &AgentDto) -> AnyView {
    let run_name = a.name.clone();
    let show_run = a.runnable && !a.running;
    let running = a.running;
    let interactive = a.executor == "tmux";
    let stop_title = if interactive {
        "kill the tmux session"
    } else {
        "stop the background process"
    };
    view! {
        {running.then(|| {
            let watch_name = run_name.clone();
            let stop_name = run_name.clone();
            view! {
                {interactive.then(|| view! {
                    <button class="adi-btn adi-btn--link" title="watch the live tmux session"
                        on:click=move |_| open_watch(watch, watch_name.clone())>"● View"</button>
                    " "
                })}
                <button class="adi-btn adi-btn--link" title=stop_title
                    on:click=move |_| stop_agent(state, watch, stop_name.clone())>"■ Stop"</button>
                " "
            }
        })}
        {show_run.then(|| { let run_name = run_name.clone(); view! {
            <button class="adi-btn adi-btn--link"
                on:click=move |_| run_agent(state, run_name.clone())>"▶ Run"</button>
            " "
        }})}
    }
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

/// Launch an agent (the ▶ Run action). The server supplies an executor-specific success message.
fn run_agent(state: State, name: String) {
    spawn_local(async move {
        match fetch::run_agent(name).await {
            Ok(res) => {
                state.agents.set(Some(res.state));
                state.flash.set(Some(Flash::ok(res.message)));
            }
            Err(e) => state.flash.set(Some(Flash::err(e))),
        }
    });
}

/// Stop a running agent, refresh the list, and close its live view if one is open.
fn stop_agent(state: State, watch: AgentsWatch, name: String) {
    if watch.name.get_untracked().as_deref() == Some(name.as_str()) {
        watch.close();
    }
    apply_agents(state, None, format!("Stopped {name}."), fetch::stop_agent(name));
}

/// Open the live view on an agent (the ● View action): show the panel, fetch the first snapshot
/// immediately (the 1s poll takes over from there), and scroll up to where the panel renders.
fn open_watch(watch: AgentsWatch, name: String) {
    watch.peek.set(None);
    watch.name.set(Some(name));
    poll_watch(watch);
    scroll_top();
}

/// Fetch a fresh pane snapshot for the watched agent, if any. The shell calls this every second;
/// it no-ops while the live view is closed. A response landing after the view moved to another
/// agent (or closed) is dropped instead of flashing the wrong pane.
pub(crate) fn poll_watch(watch: AgentsWatch) {
    let Some(name) = watch.name.get_untracked() else {
        return;
    };
    spawn_local(async move {
        if let Ok(peek) = fetch::peek_agent(name).await
            && watch.name.get_untracked().as_deref() == Some(peek.name.as_str())
        {
            watch.peek.set(Some(peek));
        }
    });
}

/// The live-view panel: a 1s-refreshed capture of the watched agent's tmux pane, with a send
/// bar to type into the session. Renders nothing while no agent is being watched. Shared with
/// a project's Agents panel.
pub(crate) fn live_view(state: State, watch: AgentsWatch) -> Option<AnyView> {
    let name = watch.name.get()?;
    let peek = watch.peek.get();
    let attach = peek.as_ref().map(|p| p.attach.clone()).unwrap_or_default();
    let running = peek.as_ref().is_some_and(|p| p.running);
    let body = match peek {
        None => view! { <div class="adi-empty">"Connecting…"</div> }.into_any(),
        Some(p) if !p.running => view! {
            <div class="adi-empty">"The session has ended — run the agent again to restart it."</div>
        }
        .into_any(),
        Some(p) => view! { <pre class="adi-term">{p.output}</pre> }.into_any(),
    };
    Some(
        view! {
            <section class="adi-panel">
                <div class="adi-panel__head">
                    <h2 class="adi-panel__title">{format!("Live view — {name}")}</h2>
                    <span class="adi-spacer"></span>
                    {(!attach.is_empty()).then(|| view! {
                        <code class="adi-mono adi-muted" style="font-size:12px">{attach}</code>
                    })}
                    <button class="adi-btn adi-btn--link" on:click=move |_| watch.close()>"Close"</button>
                </div>
                {body}
                {running.then(|| send_bar(state, watch))}
            </section>
        }
        .into_any(),
    )
}

/// The live view's input row: a text field (submit types it into the session, without a trailing
/// Enter — the ⏎ quick key sends that) plus the special keys interactive TUIs need (Enter, arrows
/// for menus, Esc, Ctrl-C).
fn send_bar(state: State, watch: AgentsWatch) -> impl IntoView {
    view! {
        <form class="adi-form" style="padding:10px 18px 14px; border-top:1px solid var(--border)"
            on:submit=move |ev| {
                ev.prevent_default();
                let text = watch.input.get();
                watch.input.set(String::new());
                send_to_agent(state, watch, text, "");
            }>
            <input class="adi-input adi-mono" style="flex:1 1 auto" autocomplete="off"
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
        <button class="adi-btn adi-btn--ghost adi-mono" type="button" style="padding:8px 10px"
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

/// Load an existing agent into the create/edit form (the Edit action).
fn load_agent_into_form(form: AgentsForm, a: &AgentDto) {
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
fn clear_agent_form(form: AgentsForm) {
    form.name.set(String::new());
    form.backend.set(String::new());
    form.project.set(String::new());
    form.model.set(String::new());
    form.permission_mode.set(String::new());
    form.temperature.set(String::new());
    form.max_turns.set(String::new());
    form.tags.set(String::new());
    form.tools.set(String::new());
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
