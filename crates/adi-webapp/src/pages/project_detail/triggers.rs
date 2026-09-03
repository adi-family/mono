//! The Triggers panel of the project detail page.

use adi_ui::{EmptyRow, Row as TableRow, Table};
use adi_webapp_api::types::{SaveTrigger, TriggersState};
use leptos::prelude::*;

use crate::fetch;
use crate::pages::triggers::{trigger_cell, trigger_key, trigger_menu_items, trigger_toggle_item};
use crate::state::{Flash, State, TriggersLogView};
use crate::ui::{Key, TextField, apply_mutation, row_actions, sort_rows};

/// The project detail page's quick trigger create form (name, kind, code; the project is fixed
/// to the open project). Full editing — presets, runtimes, settings, enable/disable — lives on
/// the Triggers page. `Copy` so it threads into the panel view and its submit handler.
#[derive(Clone, Copy)]
pub(crate) struct QuickTriggerForm {
    pub(crate) name: RwSignal<String>,
    pub(crate) kind: RwSignal<String>,
    pub(crate) code: RwSignal<String>,
    pub(crate) busy: RwSignal<bool>,
}

/// The Triggers panel on a project's detail page: the triggers filed under this project (from
/// the shared list at `/api/triggers`) with live Fire/Log/Enable actions, plus a quick create
/// form pre-scoped to it.
pub(crate) fn triggers_panel(
    state: State,
    form: QuickTriggerForm,
    log: TriggersLogView,
) -> AnyView {
    let QuickTriggerForm {
        name,
        kind,
        code,
        busy,
    } = form;
    let triggers = state.triggers;
    view! {
        <section class="adi-panel">
            <div class="adi-panel__head">
                <h2 class="adi-panel__title">"Triggers"</h2>
                <span class="adi-updated">"filed under this project"</span>
            </div>
            <Table state=state.tables.project_triggers>{move || project_trigger_rows(state, log)}</Table>
            <form class="adi-form" on:submit=move |ev| {
                ev.prevent_default();
                let id = state.current_project.get_untracked();
                if id.is_empty() {
                    return;
                }
                let nm = name.get().trim().to_string();
                if nm.is_empty() {
                    state.flash.set(Some(Flash::err("A trigger name is required.".to_string())));
                    return;
                }
                let kd = kind.get().trim().to_string();
                if kd.is_empty() {
                    state.flash.set(Some(Flash::err("Pick a kind.".to_string())));
                    return;
                }
                let body = SaveTrigger {
                    name: nm.clone(),
                    kind: kd,
                    // The quick form only writes shell blocks; pick a preset on the Triggers
                    // page to start from TypeScript.
                    runtime: String::new(),
                    code: code.get(),
                    preset: None,
                    description: String::new(),
                    enabled: true,
                    project: Some(id),
                    extra: std::collections::BTreeMap::new(),
                    // The quick project form only creates webhook/background triggers; event
                    // subscriptions are edited on the full Triggers page.
                    events: Vec::new(),
                    // No project restriction from the quick form — set it on the Triggers page.
                    trigger_on: Vec::new(),
                };
                name.set(String::new());
                code.set(String::new());
                apply_mutation(state, Some(busy), format!("Created trigger “{nm}”."),
                    |s: State, ts: TriggersState| s.triggers.set(Some(ts)), fetch::save_trigger(body));
            }>
                <TextField id="ptrigger-name" label="Name" placeholder="deploy-hook" mono=true
                    hint="also the webhook URL segment" value=name />
                <div class="adi-field">
                    <label class="adi-field__label" for="ptrigger-kind">"Launches"</label>
                    <select class="adi-input" id="ptrigger-kind"
                        prop:value=move || kind.get()
                        on:change=move |ev| kind.set(event_target_value(&ev))>
                        <option value="">"— how does it launch? —"</option>
                        {move || triggers.get().map(|t| t.kinds.into_iter().map(|k| {
                            let id = k.id.clone();
                            view! { <option value=id>{k.label}</option> }
                        }).collect::<Vec<_>>()).unwrap_or_default()}
                    </select>
                </div>
                <TextField id="ptrigger-code" label="Code block" placeholder="echo deployed" mono=true wide=true
                    field_class="adi-field--grow"
                    hint="runs as sh -c" value=code />
                <button class="adi-btn adi-btn--primary" type="submit" prop:disabled=move || busy.get()>
                    "Add trigger"
                </button>
            </form>
            <div class="adi-hint">
                "These appear in the global triggers list too. Webhook triggers are live at "
                <code>"/api/hooks/<name>"</code> "; presets, TypeScript, settings, and editing "
                "live on the Triggers page."
            </div>
        </section>
    }
    .into_any()
}

/// Rows for the project's trigger table: this project's triggers with the shared
/// Fire/Log/Enable-Disable actions. Loading/empty placeholders otherwise.
fn project_trigger_rows(state: State, log: TriggersLogView) -> AnyView {
    let table = state.tables.project_triggers;
    let id = state.current_project.get();
    let Some(st) = state.triggers.get() else {
        return view! { <EmptyRow state=table>"Loading…"</EmptyRow> }.into_any();
    };
    let mut mine: Vec<_> = st
        .triggers
        .into_iter()
        .filter(|t| t.project.as_deref() == Some(id.as_str()))
        .collect();
    if mine.is_empty() {
        return view! { <EmptyRow state=table>"No triggers in this project yet — add one below."</EmptyRow> }.into_any();
    }
    sort_rows(&mut mine, table.sort.get(), trigger_key, |t| {
        Key::text(&t.name)
    });
    mine.into_iter()
        .map(|t| {
            let mut items = trigger_menu_items(state, log, &t);
            items.push(trigger_toggle_item(state, &t));
            let actions = row_actions(state, format!("trigger:{}", t.name), (), items);
            view! { <TableRow state=table cell=move |col| trigger_cell(col, &t) actions=actions/> }
                .into_any()
        })
        .collect::<Vec<_>>()
        .into_any()
}
