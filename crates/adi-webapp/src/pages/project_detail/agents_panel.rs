//! The Agents panel of the project detail page.

use adi_webapp_api::types::{AgentsState, SaveAgent};
use leptos::prelude::*;

use crate::fetch;
use crate::pages::agents::{agent_actions, load_agent_into_form};
use crate::routing::{Route, push_state, scroll_top};
use crate::state::{AgentsForm, AgentsWatch, Flash, State};
use crate::ui::{TextField, apply_mutation, data_table, menu_item, placeholder_row, row_actions};

/// The project detail page's quick agent create form (name, backend, system prompt; the project
/// is fixed to the open project). Full editing — models, permission modes, backend params —
/// lives on the Agents page. `Copy` so it threads into the panel view and its submit handler.
#[derive(Clone, Copy)]
pub(crate) struct QuickAgentForm {
    pub(crate) name: RwSignal<String>,
    pub(crate) backend: RwSignal<String>,
    pub(crate) system_prompt: RwSignal<String>,
    pub(crate) busy: RwSignal<bool>,
}

/// The Agents panel on a project's detail page: the agents filed under this project (from the
/// shared list at `/api/agents`) with live Run/View/Stop actions, plus a quick create form
/// pre-scoped to it.
pub(crate) fn agents_panel(
    state: State,
    form: QuickAgentForm,
    watch: AgentsWatch,
    edit_form: AgentsForm,
    route: RwSignal<Route>,
) -> AnyView {
    let QuickAgentForm {
        name,
        backend,
        system_prompt,
        busy,
    } = form;
    let agents = state.agents;
    view! {
        <section class="adi-panel">
            <div class="adi-panel__head">
                <h2 class="adi-panel__title">"Agents"</h2>
                <span class="adi-updated">"filed under this project"</span>
            </div>
            {data_table(&["Name", "Backend", "Model", "Status", ""], move || project_agent_rows(state, watch, edit_form, route))}
            <form class="adi-form" on:submit=move |ev| {
                ev.prevent_default();
                let id = state.current_project.get_untracked();
                if id.is_empty() {
                    return;
                }
                let nm = name.get().trim().to_string();
                if nm.is_empty() {
                    state.flash.set(Some(Flash::err("An agent name is required.".to_string())));
                    return;
                }
                let be = backend.get().trim().to_string();
                if be.is_empty() {
                    state.flash.set(Some(Flash::err("Pick a backend.".to_string())));
                    return;
                }
                let mut arguments = std::collections::BTreeMap::new();
                let prompt = system_prompt.get();
                if !prompt.is_empty() {
                    arguments.insert("system_prompt".to_string(), prompt.into());
                }
                let body = SaveAgent {
                    name: nm.clone(),
                    backend: be,
                    arguments,
                    tags: Vec::new(),
                    starred: false,
                    project: Some(id),
                    bin_tools: Vec::new(),
                    secrets: Vec::new(),
                    // This panel only creates; renaming lives on the Agents page.
                    rename_from: None,
                };
                name.set(String::new());
                system_prompt.set(String::new());
                apply_mutation(state, Some(busy), format!("Created agent “{nm}”."),
                    |s: State, a: AgentsState| s.agents.set(Some(a)), fetch::save_agent(body));
            }>
                <TextField id="pagent-name" label="Name" placeholder="athz-solver" mono=true
                    hint="a task tagged this name auto-starts it" value=name />
                <div class="adi-field">
                    <label class="adi-field__label" for="pagent-backend">"Backend"</label>
                    <select class="adi-input" id="pagent-backend"
                        prop:value=move || backend.get()
                        on:change=move |ev| backend.set(event_target_value(&ev))>
                        <option value="">"— pick a backend —"</option>
                        {move || agents.get().map(|a| a.form.backends.into_iter().map(|b| {
                            let id = b.id.clone();
                            view! { <option value=id>{b.label}</option> }
                        }).collect::<Vec<_>>()).unwrap_or_default()}
                    </select>
                </div>
                <TextField id="pagent-prompt" label="System prompt" placeholder="optional seed prompt" wide=true
                    field_class="adi-field--grow" value=system_prompt />
                <button class="adi-btn adi-btn--primary" type="submit" prop:disabled=move || busy.get()>
                    "Add agent"
                </button>
            </form>
            <div class="adi-hint">
                "These appear in the global " <code>"Agents"</code> " list too. Models, permission
                 modes, and other backend params live on the Agents page."
            </div>
        </section>
    }
    .into_any()
}

/// Rows for the project's agent table: this project's agents with the shared Run/View/Stop
/// actions, plus an Edit that hands the agent to the full form on the Agents page. Loading/empty
/// placeholders otherwise.
fn project_agent_rows(
    state: State,
    watch: AgentsWatch,
    edit_form: AgentsForm,
    route: RwSignal<Route>,
) -> AnyView {
    let id = state.current_project.get();
    let Some(st) = state.agents.get() else {
        return placeholder_row("5", "Loading…");
    };
    let mine: Vec<_> = st
        .agents
        .into_iter()
        .filter(|a| a.project.as_deref() == Some(id.as_str()))
        .collect();
    if mine.is_empty() {
        return placeholder_row("5", "No agents in this project yet — add one below.");
    }
    mine.into_iter()
        .map(|a| {
            let name_disp = if a.starred {
                format!("★ {}", a.name)
            } else {
                a.name.clone()
            };
            let backend = a.backend.clone();
            let model = a
                .arguments
                .get("model")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_string();
            let status = if a.running {
                view! { <span class="adi-tstatus" data-status="ready">"Running"</span> }.into_any()
            } else {
                view! { <span class="adi-muted">"—"</span> }.into_any()
            };
            // The full 49-field form lives on the Agents page; Edit loads this agent into it and
            // takes you there, rather than duplicating the schema-driven form in this panel.
            let a_edit = a.clone();
            let edit = menu_item(state, "Edit", false, move || {
                load_agent_into_form(edit_form, &a_edit);
                push_state(Route::Agents.path());
                route.set(Route::Agents);
                scroll_top();
            });
            let actions = row_actions(state, format!("agent:{}", a.name),
                agent_actions(state, watch, &a), vec![edit]);
            view! {
                <tr>
                    <td>{name_disp}</td>
                    <td class="adi-mono">{backend}</td>
                    <td class="adi-mono adi-muted">{model}</td>
                    <td>{status}</td>
                    <td class="adi-table__actions">{actions}</td>
                </tr>
            }
        })
        .collect::<Vec<_>>()
        .into_any()
}
