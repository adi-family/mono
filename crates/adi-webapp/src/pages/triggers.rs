//! The Triggers page: create, edit, delete, and fire trigger definitions — named background
//! code blocks fired by an event source (a webhook call, a Telegram bot, a cron schedule, or a
//! manual fire). ▶ Fire spawns the code block detached on the server; the Log view shows the
//! last fire's output, re-polled each second while open. Webhook triggers are live at
//! `/api/hooks/<name>`; the Telegram/cron listener runtimes are future work.

use std::collections::BTreeMap;

use adi_webapp_api::types::{SaveTrigger, TriggerDto, TriggersState};
use leptos::prelude::*;
use wasm_bindgen_futures::spawn_local;

use crate::fetch;
use crate::routing::scroll_top;
use crate::state::{Flash, State, TriggersForm, TriggersLogView};
use crate::ui::{
    TextField, apply_mutation, data_table, flash_view, fmt_date, placeholder_row, tile,
    updated_text,
};

/// The kind-specific extra settings the form offers, client-side keyed by kind id. The kinds
/// themselves come from the server; these are just the input decorations for their known extras.
fn kind_extras(kind: &str) -> &'static [(&'static str, &'static str, &'static str)] {
    match kind {
        "webhook" => &[(
            "secret",
            "Secret",
            "optional — callers must pass ?secret=…",
        )],
        "telegram" => &[
            ("token_env", "Bot token env", "env var holding the bot token"),
            ("chat_id", "Chat id", "chat to listen on"),
        ],
        "cron" => &[("schedule", "Schedule", "e.g. */5 * * * *")],
        _ => &[],
    }
}

/// The Triggers page: tiles, the (optional) log view, the definitions table, and the
/// create/edit form.
pub(crate) fn triggers_view(state: State, form: TriggersForm, log: TriggersLogView) -> AnyView {
    let triggers = state.triggers;
    let secs_since = state.secs_since;
    let flash = state.flash;
    let TriggersForm {
        name,
        kind,
        project,
        description,
        code,
        enabled,
        extra,
        editing,
        busy,
    } = form;
    let projects = state.projects;
    view! {
        {trigger_tiles(state)}

        {move || log_view(log)}

        <section class="adi-panel">
            <div class="adi-panel__head">
                <h2 class="adi-panel__title">"Trigger definitions"</h2>
                <span class="adi-updated">{move || updated_text(state.ports, secs_since)}</span>
            </div>

            {data_table(&["Name", "Kind", "Project", "Status", "Last fired", ""], move || trigger_rows(state, form, log))}

            <div class="adi-panel__head" style="border-top:1px solid var(--border)">
                <h2 class="adi-panel__title">
                    {move || match editing.get() {
                        Some(n) => format!("Editing “{n}”"),
                        None => "New trigger".to_string(),
                    }}
                </h2>
                <span class="adi-spacer"></span>
                <button class="adi-btn adi-btn--link" type="button"
                    on:click=move |_| clear_trigger_form(form)>"New trigger"</button>
            </div>

            <form class="adi-form" on:submit=move |ev| {
                ev.prevent_default();
                let nm = name.get().trim().to_string();
                if nm.is_empty() {
                    flash.set(Some(Flash::err("A trigger name is required.".to_string())));
                    return;
                }
                let kd = kind.get();
                if kd.trim().is_empty() {
                    flash.set(Some(Flash::err("Pick a kind.".to_string())));
                    return;
                }
                let proj = project.get().trim().to_string();
                let body = SaveTrigger {
                    name: nm.clone(),
                    kind: kd.trim().to_string(),
                    code: code.get(),
                    description: description.get().trim().to_string(),
                    enabled: enabled.get(),
                    project: (!proj.is_empty()).then_some(proj),
                    extra: current_extras(&kd, extra.get()),
                };
                editing.set(Some(nm.clone()));
                apply_triggers(state, Some(busy), format!("Saved trigger “{nm}”."),
                    fetch::save_trigger(body));
            }>
                <TextField id="trigger-name" label="Name" placeholder="deploy-hook" mono=true
                    hint="also the webhook URL segment" value=name />
                <div class="adi-field">
                    <label class="adi-field__label" for="trigger-kind">"Kind"</label>
                    <select class="adi-input" id="trigger-kind"
                        prop:value=move || kind.get()
                        on:change=move |ev| kind.set(event_target_value(&ev))>
                        <option value="">"— pick a kind —"</option>
                        {move || triggers.get().map(|t| t.kinds.into_iter().map(|k| {
                            let id = k.id.clone();
                            view! { <option value=id>{k.label}</option> }
                        }).collect::<Vec<_>>()).unwrap_or_default()}
                    </select>
                    <span class="adi-field__hint">{move || kind_hint(triggers.get().as_ref(), &kind.get())}</span>
                </div>
                <div class="adi-field">
                    <label class="adi-field__label" for="trigger-project">"Project"</label>
                    <select class="adi-input" id="trigger-project"
                        prop:value=move || project.get()
                        on:change=move |ev| project.set(event_target_value(&ev))>
                        <option value="">"— global —"</option>
                        {move || projects.get().map(|p| p.projects.into_iter()
                            .filter(|proj| !proj.is_archived())
                            .map(|proj| {
                                let id = proj.id.clone();
                                let label = if proj.name == proj.id { proj.id.clone() } else { format!("{} · {}", proj.id, proj.name) };
                                view! { <option value=id>{label}</option> }
                            }).collect::<Vec<_>>()).unwrap_or_default()}
                    </select>
                    <span class="adi-field__hint">"shows on that project's page"</span>
                </div>
                <TextField id="trigger-description" label="Description" placeholder="what this trigger does"
                    wide=true field_style="flex:1 1 220px; min-width:0" value=description />
                <label class="adi-field" style="flex-direction:row; align-items:center; gap:7px; align-self:center">
                    <input type="checkbox"
                        prop:checked=move || enabled.get()
                        on:change=move |ev| enabled.set(event_target_checked(&ev)) />
                    <span class="adi-field__label" style="margin:0">"Enabled"</span>
                </label>
                {move || extra_fields(form)}
                <div class="adi-field" style="flex:1 1 100%; min-width:0">
                    <label class="adi-field__label" for="trigger-code">"Code block"</label>
                    <textarea class="adi-textarea adi-mono" id="trigger-code"
                        placeholder="The shell code that runs when the trigger fires…"
                        prop:value=move || code.get()
                        on:input=move |ev| code.set(event_target_value(&ev))></textarea>
                    <span class="adi-field__hint">
                        "runs as sh -c, detached; payload lands in $ADI_PAYLOAD_FILE (and $ADI_PAYLOAD when small)"
                    </span>
                </div>
                <button class="adi-btn adi-btn--primary" type="submit" prop:disabled=move || busy.get()>
                    {move || if editing.get().is_some() { "Update trigger" } else { "Create trigger" }}
                </button>
            </form>
            {flash_view(flash)}
            <div class="adi-muted" style="padding:0 18px 14px; font-size:12.5px">
                "A webhook trigger is live at " <code>"/api/hooks/<name>"</code>
                " (POST or GET; add " <code>"?secret=…"</code> " when one is set). ▶ Fire runs the
                 code block by hand; its output lands in the per-trigger log. Telegram and cron
                 listeners are future work — their definitions are stored now."
            </div>
        </section>
    }
    .into_any()
}

/// The stat-tile strip: totals, enabled count, live webhooks, and how many ever fired.
fn trigger_tiles(state: State) -> impl IntoView {
    let triggers = state.triggers;
    view! {
        <section class="adi-tiles">
            {tile("Triggers",
                move || triggers.get().map_or_else(|| "—".to_string(), |t| t.triggers.len().to_string()),
                "defined")}
            {tile("Enabled",
                move || triggers.get().map_or_else(|| "—".to_string(), |t| count(&t, |x| x.enabled).to_string()),
                "may be fired by their source")}
            {tile("Webhooks",
                move || triggers.get().map_or_else(|| "—".to_string(), |t| count(&t, |x| x.kind == "webhook").to_string()),
                "live at /api/hooks/<name>")}
            {tile("Fired",
                move || triggers.get().map_or_else(|| "—".to_string(), |t| count(&t, |x| x.last_fired_at.is_some()).to_string()),
                "at least once")}
        </section>
    }
}

/// Count triggers matching a predicate.
fn count(st: &TriggersState, pred: impl Fn(&TriggerDto) -> bool) -> usize {
    st.triggers.iter().filter(|t| pred(t)).count()
}

/// The hint for the currently selected kind, from the server-owned kind options.
fn kind_hint(st: Option<&TriggersState>, kind: &str) -> String {
    st.and_then(|st| st.kinds.iter().find(|k| k.id == kind))
        .map(|k| k.hint.clone())
        .unwrap_or_default()
}

/// The kind-specific extra inputs for the currently chosen kind.
fn extra_fields(form: TriggersForm) -> AnyView {
    kind_extras(&form.kind.get())
        .iter()
        .map(|&(key, label, hint)| {
            let for_value = key;
            let for_input = key;
            let show_hint = !hint.is_empty();
            view! {
                <div class="adi-field">
                    <label class="adi-field__label" for=format!("trigger-{key}")>{label}</label>
                    <input class="adi-input adi-mono" id=format!("trigger-{key}") autocomplete="off"
                        prop:value=move || form.extra.get().get(for_value).cloned().unwrap_or_default()
                        on:input=move |ev| set_extra(form.extra, for_input, event_target_value(&ev)) />
                    {show_hint.then(|| view! { <span class="adi-field__hint">{hint}</span> })}
                </div>
            }
        })
        .collect::<Vec<_>>()
        .into_any()
}

fn set_extra(extra: RwSignal<BTreeMap<String, String>>, key: &str, value: String) {
    extra.update(|values| {
        if value.trim().is_empty() {
            values.remove(key);
        } else {
            values.insert(key.to_string(), value);
        }
    });
}

/// The extras that get submitted: only the keys the chosen kind knows, trimmed and non-empty —
/// switching kinds in the form never leaks another kind's settings into the save.
fn current_extras(kind: &str, values: BTreeMap<String, String>) -> BTreeMap<String, String> {
    let known = kind_extras(kind);
    values
        .into_iter()
        .map(|(k, v)| (k, v.trim().to_string()))
        .filter(|(k, v)| !v.is_empty() && known.iter().any(|&(key, _, _)| key == k))
        .collect()
}

/// Render the triggers table body: a loading/empty placeholder, or one row per trigger with
/// Fire, Log, Enable/Disable, Edit, and Delete actions.
fn trigger_rows(state: State, form: TriggersForm, log: TriggersLogView) -> AnyView {
    let Some(st) = state.triggers.get() else {
        return placeholder_row("6", "Loading…");
    };
    if st.triggers.is_empty() {
        return placeholder_row("6", "No triggers yet — define one below.");
    }
    st.triggers
        .into_iter()
        .map(|t| {
            let del_name = t.name.clone();
            let t_edit = t.clone();
            let kind = t.kind.clone();
            let project_cell = match &t.project {
                Some(p) if !p.trim().is_empty() => {
                    let p = p.clone();
                    view! { <span class="adi-chip adi-mono">{p}</span> }.into_any()
                }
                _ => view! { <span class="adi-muted">"—"</span> }.into_any(),
            };
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
                    <td>{project_cell}</td>
                    <td><span class="adi-tstatus" data-status=status_data>{status}</span></td>
                    <td class="adi-mono adi-muted">{fired}</td>
                    <td style="text-align:right; white-space:nowrap">
                        {trigger_actions(state, log, &t)}
                        <button class="adi-btn adi-btn--link"
                            on:click=move |_| load_trigger_into_form(form, &t_edit)>"Edit"</button>
                        " "
                        <button class="adi-btn adi-btn--link" on:click=move |_| {
                            apply_triggers(state, None, format!("Deleted {del_name}."),
                                fetch::delete_trigger(del_name.clone()));
                        }>"Delete"</button>
                    </td>
                </tr>
            }
        })
        .collect::<Vec<_>>()
        .into_any()
}

/// The Fire / Log / Enable-Disable action buttons for one trigger row — shared between the
/// global Triggers table and a project's Triggers panel.
pub(crate) fn trigger_actions(state: State, log: TriggersLogView, t: &TriggerDto) -> AnyView {
    let fire_name = t.name.clone();
    let log_name = t.name.clone();
    let toggle = t.clone();
    let toggle_label = if t.enabled { "Disable" } else { "Enable" };
    view! {
        <button class="adi-btn adi-btn--link" title="run the code block now"
            on:click=move |_| fire_trigger(state, fire_name.clone())>"▶ Fire"</button>
        " "
        <button class="adi-btn adi-btn--link" title="show the last fire's output"
            on:click=move |_| open_log(log, log_name.clone())>"Log"</button>
        " "
        <button class="adi-btn adi-btn--link"
            on:click=move |_| toggle_trigger(state, &toggle)>{toggle_label}</button>
        " "
    }
    .into_any()
}

/// Run a triggers mutation: set the returned list and a success flash, or an error flash;
/// toggles `busy` around the request when a form is driving it.
fn apply_triggers<F>(state: State, busy: Option<RwSignal<bool>>, ok_msg: String, fut: F)
where
    F: std::future::Future<Output = Result<TriggersState, String>> + 'static,
{
    apply_mutation(state, busy, ok_msg, |s, t| s.triggers.set(Some(t)), fut);
}

/// Fire a trigger by hand (the ▶ Fire action). The success flash comes from the server — its
/// message carries the spawned pid.
fn fire_trigger(state: State, name: String) {
    spawn_local(async move {
        match fetch::fire_trigger(name).await {
            Ok(res) => {
                state.triggers.set(Some(res.state));
                state.flash.set(Some(Flash::ok(res.message)));
            }
            Err(e) => state.flash.set(Some(Flash::err(e))),
        }
    });
}

/// Flip a trigger's enabled flag by re-saving its full definition (the server preserves
/// `created_at`).
fn toggle_trigger(state: State, t: &TriggerDto) {
    let verb = if t.enabled { "Disabled" } else { "Enabled" };
    let body = SaveTrigger {
        name: t.name.clone(),
        kind: t.kind.clone(),
        code: t.code.clone(),
        description: t.description.clone(),
        enabled: !t.enabled,
        project: t.project.clone(),
        extra: t.extra.clone(),
    };
    apply_triggers(state, None, format!("{verb} {}.", t.name), fetch::save_trigger(body));
}

/// Open the log view on a trigger (the Log action): show the panel, fetch the first snapshot
/// immediately (the 1s poll takes over from there), and scroll up to where the panel renders.
pub(crate) fn open_log(log: TriggersLogView, name: String) {
    log.log.set(None);
    log.name.set(Some(name));
    poll_trigger_log(log);
    scroll_top();
}

/// Fetch a fresh log snapshot for the watched trigger, if any. The shell calls this every
/// second; it no-ops while the log view is closed. A response landing after the view moved to
/// another trigger (or closed) is dropped instead of flashing the wrong log.
pub(crate) fn poll_trigger_log(log: TriggersLogView) {
    let Some(name) = log.name.get_untracked() else {
        return;
    };
    spawn_local(async move {
        if let Ok(snapshot) = fetch::trigger_log(name).await
            && log.name.get_untracked().as_deref() == Some(snapshot.name.as_str())
        {
            log.log.set(Some(snapshot));
        }
    });
}

/// The log panel: the last fire's output for the watched trigger, refreshed each second.
/// Renders nothing while no trigger is being watched. Shared with a project's Triggers panel.
pub(crate) fn log_view(log: TriggersLogView) -> Option<AnyView> {
    let name = log.name.get()?;
    let snapshot = log.log.get();
    let fired_at = snapshot
        .as_ref()
        .and_then(|s| s.fired_at)
        .map(fmt_date)
        .unwrap_or_default();
    let body = match snapshot {
        None => view! { <div class="adi-empty">"Loading…"</div> }.into_any(),
        Some(s) if !s.fired => view! {
            <div class="adi-empty">"This trigger has never fired — its log is empty."</div>
        }
        .into_any(),
        Some(s) => view! { <pre class="adi-term">{s.output}</pre> }.into_any(),
    };
    Some(
        view! {
            <section class="adi-panel">
                <div class="adi-panel__head">
                    <h2 class="adi-panel__title">{format!("Fire log — {name}")}</h2>
                    <span class="adi-spacer"></span>
                    {(!fired_at.is_empty()).then(|| view! {
                        <span class="adi-muted" style="font-size:12px">{format!("last fired {fired_at}")}</span>
                    })}
                    <button class="adi-btn adi-btn--link" on:click=move |_| log.close()>"Close"</button>
                </div>
                {body}
            </section>
        }
        .into_any(),
    )
}

/// Load an existing trigger into the create/edit form (the Edit action).
fn load_trigger_into_form(form: TriggersForm, t: &TriggerDto) {
    form.name.set(t.name.clone());
    form.kind.set(t.kind.clone());
    form.project.set(t.project.clone().unwrap_or_default());
    form.description.set(t.description.clone());
    form.code.set(t.code.clone());
    form.enabled.set(t.enabled);
    form.extra.set(t.extra.clone());
    form.editing.set(Some(t.name.clone()));
    scroll_top();
}

/// Reset the create/edit form back to a blank "New trigger" state.
fn clear_trigger_form(form: TriggersForm) {
    form.name.set(String::new());
    form.kind.set(String::new());
    form.project.set(String::new());
    form.description.set(String::new());
    form.code.set(String::new());
    form.enabled.set(true);
    form.extra.set(BTreeMap::new());
    form.editing.set(None);
}
