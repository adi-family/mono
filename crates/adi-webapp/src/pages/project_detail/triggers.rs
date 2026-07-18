//! The Triggers panel of the project detail page.

use adi_webapp_api::types::{SaveTrigger, TriggersState};
use leptos::prelude::*;

use crate::fetch;
use crate::pages::triggers::trigger_actions;
use crate::state::{Flash, State, TriggersLogView};
use crate::ui::{TextField, apply_mutation, data_table, fmt_date, placeholder_row};

/// The project detail page's quick trigger create form (name, kind, code; the project is fixed
/// to the open project). Full editing — description, secrets, enable/disable — lives on the
/// Triggers page. `Copy` so it threads into the panel view and its submit handler.
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
pub(crate) fn triggers_panel(state: State, form: QuickTriggerForm, log: TriggersLogView) -> AnyView {
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
            {data_table(&["Name", "Kind", "Status", "Last fired", ""], move || project_trigger_rows(state, log))}
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
                    code: code.get(),
                    description: String::new(),
                    enabled: true,
                    project: Some(id),
                    extra: std::collections::BTreeMap::new(),
                };
                name.set(String::new());
                code.set(String::new());
                apply_mutation(state, Some(busy), format!("Created trigger “{nm}”."),
                    |s: State, ts: TriggersState| s.triggers.set(Some(ts)), fetch::save_trigger(body));
            }>
                <TextField id="ptrigger-name" label="Name" placeholder="deploy-hook" mono=true
                    hint="also the webhook URL segment" value=name />
                <div class="adi-field">
                    <label class="adi-field__label" for="ptrigger-kind">"Kind"</label>
                    <select class="adi-input" id="ptrigger-kind"
                        prop:value=move || kind.get()
                        on:change=move |ev| kind.set(event_target_value(&ev))>
                        <option value="">"— pick a kind —"</option>
                        {move || triggers.get().map(|t| t.kinds.into_iter().map(|k| {
                            let id = k.id.clone();
                            view! { <option value=id>{k.label}</option> }
                        }).collect::<Vec<_>>()).unwrap_or_default()}
                    </select>
                </div>
                <TextField id="ptrigger-code" label="Code block" placeholder="echo deployed" mono=true wide=true
                    field_style="flex:1 1 260px; min-width:0"
                    hint="runs as sh -c, detached" value=code />
                <button class="adi-btn adi-btn--primary" type="submit" prop:disabled=move || busy.get()>
                    "Add trigger"
                </button>
            </form>
            <div class="adi-muted" style="padding:0 18px 14px; font-size:12.5px">
                "These appear in the global " <code>"Triggers"</code> " list too. Webhook triggers are "
                "live at " <code>"/api/hooks/<name>"</code> "; secrets, descriptions, and editing live "
                "on the Triggers page."
            </div>
        </section>
    }
    .into_any()
}

/// Rows for the project's trigger table: this project's triggers with the shared
/// Fire/Log/Enable-Disable actions. Loading/empty placeholders otherwise.
fn project_trigger_rows(state: State, log: TriggersLogView) -> AnyView {
    let id = state.current_project.get();
    let Some(st) = state.triggers.get() else {
        return placeholder_row("5", "Loading…");
    };
    let mine: Vec<_> = st
        .triggers
        .into_iter()
        .filter(|t| t.project.as_deref() == Some(id.as_str()))
        .collect();
    if mine.is_empty() {
        return placeholder_row("5", "No triggers in this project yet — add one below.");
    }
    mine.into_iter()
        .map(|t| {
            let kind = t.kind.clone();
            let hook_hint = (t.kind == "webhook").then(|| format!("/api/hooks/{}", t.name));
            let status = if t.enabled { "Enabled" } else { "Disabled" };
            let status_data = if t.enabled { "ready" } else { "archived" };
            let fired = t.last_fired_at.map_or_else(|| "—".to_string(), fmt_date);
            let description = t.description.clone();
            view! {
                <tr>
                    <td title=description>
                        <span>{t.name.clone()}</span>
                        {hook_hint.map(|h| view! {
                            <span class="adi-muted adi-mono" style="font-size:11.5px; display:block">{h}</span>
                        })}
                    </td>
                    <td class="adi-mono">{kind}</td>
                    <td><span class="adi-tstatus" data-status=status_data>{status}</span></td>
                    <td class="adi-mono adi-muted">{fired}</td>
                    <td style="text-align:right; white-space:nowrap">
                        {trigger_actions(state, log, &t)}
                    </td>
                </tr>
            }
        })
        .collect::<Vec<_>>()
        .into_any()
}
