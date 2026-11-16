//! The Triggers page: create, edit, delete, and run trigger definitions.
//!
//! A trigger is a code block plus one fact about *how it launches* — as a **webhook** (live at
//! `/api/hooks/<name>`, the request body becomes its payload) or in the **background** (a
//! long-lived process the app keeps alive while the trigger is enabled). What it *does* comes
//! from its code block, which a **preset** prefills: applying "Telegram bot" fills the kind,
//! the runtime, a working script, and the settings that script reads.
//!
//! Background triggers show live status (up, its pid, how long, how many restarts) and offer
//! Restart in their row menu; webhook triggers offer Fire. The Log view shows the most recent
//! output for either, re-polled each second while open.

use std::collections::BTreeMap;

use adi_ui::Lang;
use adi_ui::{Row as TableRow, Table};
use adi_webapp_api::types::{HookSecretStatus, SaveTrigger, TriggerDto, TriggerPreset, TriggersState};
use leptos::html::Input;
use leptos::prelude::*;
use wasm_bindgen_futures::spawn_local;

use crate::fetch;
use crate::routing::scroll_top;
use crate::state::{Flash, State, TriggersForm, TriggersLogView, read_error};
use crate::ui::{
    Key, LogPhase, TextField, apply_mutation, copy_row, field_hint, flash_view, fmt_date,
    fmt_uptime, log_pane, menu_item, row_actions, rows_or_status, sort_rows, updated_text,
};

/// The Triggers page's columns; the trailing blank one holds the row's ⋯ menu.
pub(crate) const COLS: &[&str] = &["Name", "Launches", "Project", "Status", "Last run", ""];

/// The same in a project's own Triggers panel, which drops the Project column — every row there
/// is filed under the open project, so it would say the same thing all the way down.
pub(crate) const PROJECT_COLS: &[&str] = &["Name", "Launches", "Status", "Last run", ""];

/// The `extra` keys the relay section owns, rendered through its
/// own dedicated fields below rather than the generic freeform list — `relay_endpoint` is
/// server-written and must never be a hand-editable text box, and the other three deserve a
/// select/number input and a licence field rather than a bare `<input>`.
const RELAY_EXTRA_KEYS: &[&str] = &["relay", "relay_retention", "relay_endpoint", "licence_code"];

/// The reserved hook-secret `extra` key [`hook_secret_field`] renders through its own dedicated
/// input — never the generic freeform list, and never (for `secret`, the legacy inline form) a
/// value the server even hands the client. `secret_ref` is listed here purely so it's excluded
/// from the generic list too; it isn't sensitive and round-trips through `extra` like any other
/// key, see [`current_extras`].
const HOOK_SECRET_REF_KEY: &str = "secret_ref";

/// The four hosted relays the task names, plus `custom:<url>` for a self-host
/// operator's own instance — must mirror `adi-app/src/relay.rs::resolve_url` exactly. Duplicated
/// rather than shared: this wasm frontend can't depend on that native-only crate, and the
/// vocabulary is small and settled (the same four names the save form's `<option>`s below offer).
fn resolve_relay_url(relay: &str) -> Option<String> {
    match relay {
        "public-eu" => Some("https://eu.webhooks.withadi.dev".to_string()),
        "public-us" => Some("https://us.webhooks.withadi.dev".to_string()),
        "enterprise-eu" => Some("https://eu.ent.webhooks.withadi.dev".to_string()),
        "enterprise-us" => Some("https://us.ent.webhooks.withadi.dev".to_string()),
        other => other
            .strip_prefix("custom:")
            .map(str::trim)
            .filter(|u| !u.is_empty())
            .map(str::to_string),
    }
}

/// The Triggers page: the definitions table, the (optional) log view, and the create/edit form.
pub(crate) fn triggers_view(state: State, form: TriggersForm, log: TriggersLogView) -> AnyView {
    let triggers = state.triggers;
    let secs_since = state.secs_since;
    let flash = state.flash;
    let TriggersForm {
        name,
        kind,
        runtime,
        preset,
        project,
        description,
        code,
        enabled,
        // The settings inputs (including the hook secret) are derived from the whole form
        // (kind + preset), so they read `extra`/`hook_secret_status`/`clear_hook_secret` through
        // it rather than from a loose signal here.
        extra: _,
        events,
        trigger_on,
        editing,
        busy,
        relay_key,
        hook_secret_status: _,
        clear_hook_secret: _,
    } = form;
    let projects = state.projects;

    // Fetch this node's relay public key once, the first time the relay section could possibly
    // show (a webhook trigger is open) — a per-node constant, so one fetch covers every trigger
    // edited for the rest of this page's life.
    Effect::new(move |_| {
        if kind.get() == "webhook" && relay_key.get_untracked().is_none() {
            spawn_local(async move {
                if let Ok(res) = fetch::relay_key().await {
                    relay_key.set(Some(res.public_key));
                }
            });
        }
    });

    view! {
        {move || log_view(log)}

        <section class="adi-panel">
            <div class="adi-panel__head">
                <span class="adi-updated" title="Triggers defined">
                    {move || triggers.get().map_or_else(|| "\u{2014}".to_string(),
                        |t| format!("{} defined", t.triggers.len()))}
                </span>
                <span class="adi-spacer"></span>
                <span class="adi-updated">{move || updated_text(triggers, secs_since)}</span>
            </div>

            <Table state=state.tables.triggers>{move || trigger_rows(state, form, log)}</Table>
        </section>

        <section class="adi-panel">
            <div class="adi-panel__head">
                <h2 class="adi-panel__title">
                    {move || match editing.get() {
                        Some(n) => format!("Editing “{n}”"),
                        None => "New trigger".to_string(),
                    }}
                </h2>
                <span class="adi-spacer"></span>
                {move || editing.get().is_some().then(|| view! {
                    <button class="adi-btn adi-btn--quiet" type="button"
                        on:click=move |_| clear_trigger_form(form)>"New trigger"</button>
                })}
            </div>

            {move || preset_picker(state, form)}

            <form class="adi-form" on:submit=move |ev| {
                ev.prevent_default();
                let nm = name.get().trim().to_string();
                if nm.is_empty() {
                    flash.set(Some(Flash::err("A trigger name is required.".to_string())));
                    return;
                }
                let kd = kind.get();
                if kd.trim().is_empty() {
                    flash.set(Some(Flash::err("Pick how this trigger launches.".to_string())));
                    return;
                }
                let proj = project.get().trim().to_string();
                let body = SaveTrigger {
                    name: nm.clone(),
                    kind: kd.trim().to_string(),
                    runtime: runtime.get().trim().to_string(),
                    code: code.get(),
                    preset: preset.get(),
                    description: description.get().trim().to_string(),
                    enabled: enabled.get(),
                    project: (!proj.is_empty()).then_some(proj),
                    extra: current_extras(state, form),
                    events: parse_event_patterns(&events.get()),
                    trigger_on: trigger_on.get(),
                    clear_hook_secret: form.clear_hook_secret.get(),
                };
                form.clear_hook_secret.set(false);
                editing.set(Some(nm.clone()));
                apply_triggers(state, Some(busy), fetch::save_trigger(body));
            }>
                <TextField id="trigger-name" label="Name" placeholder="deploy-hook" mono=true
                    hint="also the webhook URL segment" value=name />
                <div class="adi-field">
                    <label class="adi-field__label" for="trigger-kind">"Launches"</label>
                    <select class="adi-input" id="trigger-kind"
                        prop:value=move || kind.get()
                        on:change=move |ev| kind.set(event_target_value(&ev))>
                        <option value="">"— how does it launch? —"</option>
                        {move || triggers.get().map(|t| t.kinds.into_iter().map(|k| {
                            let id = k.id.clone();
                            view! { <option value=id>{k.label}</option> }
                        }).collect::<Vec<_>>()).unwrap_or_default()}
                    </select>
                    {field_hint(move || kind_hint(triggers.get().as_ref(), &kind.get()))}
                </div>
                <div class="adi-field">
                    <label class="adi-field__label" for="trigger-runtime">"Language"</label>
                    <select class="adi-input" id="trigger-runtime"
                        prop:value=move || runtime_or_default(&runtime.get())
                        on:change=move |ev| runtime.set(event_target_value(&ev))>
                        {move || triggers.get().map(|t| t.runtimes.into_iter().map(|r| {
                            let id = r.id.clone();
                            view! { <option value=id>{r.label}</option> }
                        }).collect::<Vec<_>>()).unwrap_or_default()}
                    </select>
                    {field_hint(move || runtime_hint(triggers.get().as_ref(), &runtime.get()))}
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
                                let label = proj.name.clone();
                                view! { <option value=id>{label}</option> }
                            }).collect::<Vec<_>>()).unwrap_or_default()}
                    </select>
                    {field_hint("shows on that project's page")}
                </div>
                {move || matches!(kind.get().as_str(), "event" | "webhook")
                    .then(|| trigger_on_field(state, form))}
                <TextField id="trigger-description" label="Description" placeholder="what this trigger does"
                    wide=true field_class="adi-field--grow" value=description />
                <label class="adi-field adi-field--check">
                    <input type="checkbox" class="adi-check"
                        prop:checked=move || enabled.get()
                        on:change=move |ev| enabled.set(event_target_checked(&ev)) />
                    <span class="adi-field__label">"Enabled"</span>
                </label>
                {move || (kind.get() == "webhook").then(|| hook_secret_field(form))}
                {move || extra_fields(state, form)}
                {move || (kind.get() == "webhook").then(|| relay_fields(form))}
                {move || (kind.get() == "event").then(|| view! {
                    <div class="adi-field adi-field--full">
                        <label class="adi-field__label" for="trigger-events">"Events"</label>
                        <textarea class="adi-input adi-input--wide adi-mono" id="trigger-events" rows="3"
                            placeholder="adi.tasks.*"
                            prop:value=move || events.get()
                            on:input=move |ev| events.set(event_target_value(&ev))></textarea>
                        {field_hint("one pattern per line — * matches one segment, ** the tail (e.g. adi.tasks.*)")}
                    </div>
                })}
                {move || (kind.get() == "event").then(|| event_catalog_view(state, form))}
                <div class="adi-field adi-field--full">
                    <label class="adi-field__label" for="trigger-code">"Code block"</label>
                    // The same editor the store file page uses, so a trigger's code gets the
                    // highlighting its language deserves — and follows the runtime picker above.
                    <adi_ui::CodeEditor value=code
                        lang=Signal::derive(move || code_lang(&runtime.get()))
                        height=adi_ui::CodeHeight::Form id="trigger-code"
                        class="island bg-raise"/>
                    {field_hint(move || code_hint(&kind.get(), &runtime.get()))}
                </div>
                <button class="adi-btn adi-btn--primary" type="submit" prop:disabled=move || busy.get()>
                    {move || if editing.get().is_some() { "Update trigger" } else { "Create trigger" }}
                </button>
            </form>
            {flash_view(flash)}
            <div class="adi-hint">
                "A webhook trigger is live at " <code>"/api/hooks/<name>"</code>
                " (POST or GET; present the hook secret, when one is set, as either an "
                <code>"X-ADI-Hook-Secret"</code> " header or " <code>"?secret=…"</code> " — the
                 header doesn't end up in a proxy or access log the way a query string can). A
                 background trigger runs for as long as it is enabled — the app restarts it if
                 it exits, and editing its code restarts it. Every setting above reaches the
                 code block as " <code>"$ADI_<KEY>"</code> " — except the hook secret itself,
                 which the code block never sees."
            </div>
        </section>
    }
    .into_any()
}

/// The preset row: one button per preset, which fills the whole form with a working starting
/// point. This is where the kinds' old variety lives now — "Telegram bot" is a preset, not a
/// kind, so its code is visible and editable instead of hidden behind a listener.
fn preset_picker(state: State, form: TriggersForm) -> AnyView {
    let Some(st) = state.triggers.get() else {
        return ().into_any();
    };
    if st.presets.is_empty() {
        return ().into_any();
    }
    let current = form.preset.get();
    let buttons = st
        .presets
        .into_iter()
        .map(|p| {
            let selected = current.as_deref() == Some(p.id.as_str());
            let title = p.description.clone();
            let label = p.label.clone();
            let applied = p.clone();
            view! {
                <button class="adi-preset" type="button" title=title
                    aria-pressed=selected.to_string()
                    on:click=move |_| apply_preset(form, &applied)>{label}</button>
            }
        })
        .collect::<Vec<_>>();
    view! {
        <div class="adi-field adi-field--full">
            <span class="adi-field__label">"Start from a preset"</span>
            <div class="adi-field__choices">
                {buttons}
            </div>
            {field_hint("fills the form with a working code block — edit it however you like")}
        </div>
    }
    .into_any()
}

/// Apply a preset to the form: its kind, runtime, code, and the default values of the settings
/// it declares. The name and project are left alone — those are the user's, not the preset's.
fn apply_preset(form: TriggersForm, preset: &TriggerPreset) {
    form.kind.set(preset.kind.clone());
    form.runtime.set(preset.runtime.clone());
    form.code.set(preset.code.clone());
    form.preset.set(Some(preset.id.clone()));
    // An event preset ships suggested patterns; every other kind clears them.
    form.events.set(preset.events.join("\n"));
    if form.description.get_untracked().trim().is_empty() {
        form.description.set(preset.description.clone());
    }
    form.extra.update(|values| {
        for field in &preset.fields {
            if !field.default.is_empty() {
                values
                    .entry(field.key.clone())
                    .or_insert_with(|| field.default.clone());
            }
        }
    });
}

/// Split the Events textarea into subscription patterns: one per line or comma, trimmed, with
/// blanks dropped. The inverse of the `join("\n")` used when a trigger or preset is loaded in.
fn parse_event_patterns(text: &str) -> Vec<String> {
    text.split(['\n', ','])
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .map(str::to_string)
        .collect()
}

/// The event catalog reference shown under the Events patterns for an event trigger: one chip per
/// platform event (click to add it to the patterns; hover shows the payload example), plus a note
/// on how an event reaches the code block. Answers "what can I subscribe to, and what will I get?".
fn event_catalog_view(state: State, form: TriggersForm) -> AnyView {
    let Some(st) = state.triggers.get() else {
        return ().into_any();
    };
    if st.event_types.is_empty() {
        return ().into_any();
    }
    let chips = st
        .event_types
        .into_iter()
        .map(|e| {
            let name = e.name;
            let label = name.clone();
            let insert = name.clone();
            let example = e.example.to_string();
            let title = if example.is_empty() || example == "null" {
                e.summary
            } else {
                format!("{}\nexample: {}", e.summary, example)
            };
            // An event name is a machine string, so the chip is the mono one.
            view! {
                <button class="adi-toolpick__chip" type="button" title=title
                    on:click=move |_| add_event_pattern(form.events, &insert)>{label}</button>
            }
        })
        .collect::<Vec<_>>();
    view! {
        <div class="adi-field adi-field--full">
            <span class="adi-field__label">"Available events"</span>
            <div class="adi-toolpick">
                {chips}
            </div>
            {field_hint("click to add a pattern · each event arrives in $ADI_PAYLOAD, its name in $ADI_EVENT")}
        </div>
    }
    .into_any()
}

/// Add `name` to the Events patterns textarea if it isn't already present, normalizing the box to
/// one pattern per line.
fn add_event_pattern(events: RwSignal<String>, name: &str) {
    events.update(|text| {
        let mut patterns: Vec<String> = text
            .lines()
            .map(|l| l.trim().to_string())
            .filter(|l| !l.is_empty())
            .collect();
        if !patterns.iter().any(|p| p == name) {
            patterns.push(name.to_string());
        }
        *text = patterns.join("\n");
    });
}

/// The project-restriction field for event/webhook triggers: one checkbox per live project,
/// toggling membership in the `trigger_on` allowlist. None checked = unrestricted (fires for
/// every project); otherwise the trigger fires only when the fire's payload names a checked
/// project. Renders nothing until the project list loads.
fn trigger_on_field(state: State, form: TriggersForm) -> AnyView {
    let projects = state.projects;
    let trigger_on = form.trigger_on;
    view! {
        <div class="adi-field adi-field--full">
            <span class="adi-field__label">"Run only for projects"</span>
            <div class="adi-field__choices">
                {move || projects.get().map(|p| p.projects.into_iter()
                    .filter(|proj| !proj.is_archived())
                    .map(|proj| {
                        let checked_id = proj.id.clone();
                        let toggle_id = proj.id.clone();
                        let label = proj.name.clone();
                        view! {
                            <label class="adi-field adi-field--check">
                                <input type="checkbox" class="adi-check"
                                    prop:checked=move || trigger_on.get().iter().any(|x| *x == checked_id)
                                    on:change=move |ev| toggle_trigger_on(trigger_on, &toggle_id, event_target_checked(&ev)) />
                                <span class="adi-field__label">{label}</span>
                            </label>
                        }
                    }).collect::<Vec<_>>()).unwrap_or_default()}
            </div>
            {field_hint("none checked = every project · otherwise fires only when the payload names a checked project")}
        </div>
    }
    .into_any()
}

/// Add or remove a project id from the `trigger_on` allowlist as its checkbox is toggled.
fn toggle_trigger_on(trigger_on: RwSignal<Vec<String>>, id: &str, on: bool) {
    trigger_on.update(|list| {
        if on {
            if !list.iter().any(|x| x == id) {
                list.push(id.to_string());
            }
        } else {
            list.retain(|x| x != id);
        }
    });
}

/// The hint for the currently selected kind, from the server-owned kind options.
fn kind_hint(st: Option<&TriggersState>, kind: &str) -> String {
    st.and_then(|st| st.kinds.iter().find(|k| k.id == kind))
        .map(|k| k.hint.clone())
        .unwrap_or_default()
}

/// The hint for the currently selected runtime.
fn runtime_hint(st: Option<&TriggersState>, runtime: &str) -> String {
    let runtime = runtime_or_default(runtime);
    st.and_then(|st| st.runtimes.iter().find(|r| r.id == runtime))
        .map(|r| r.hint.clone())
        .unwrap_or_default()
}

/// The highlighter language for a runtime, so the editor colours a trigger's code the way the
/// interpreter will read it.
fn code_lang(runtime: &str) -> Lang {
    if runtime_or_default(runtime) == "ts" {
        Lang::Ts
    } else {
        Lang::Sh
    }
}

/// An unset runtime shows as shell — the same default the server saves.
fn runtime_or_default(runtime: &str) -> String {
    if runtime.trim().is_empty() {
        "sh".to_string()
    } else {
        runtime.to_string()
    }
}

/// What to tell the user about the code block they're writing, given how it will be launched.
fn code_hint(kind: &str, runtime: &str) -> String {
    let payload = match kind {
        "webhook" => "the request body lands in $ADI_PAYLOAD_FILE",
        "event" => "the event lands in $ADI_PAYLOAD; its name in $ADI_EVENT",
        _ => "keep it running — the app restarts it if it exits",
    };
    let how = if runtime_or_default(runtime) == "ts" {
        "run with bun"
    } else {
        "run as sh -c"
    };
    format!("{how}; {payload}; settings arrive as $ADI_<KEY>")
}

/// The settings inputs for the form as it stands: whatever the applied preset declares. A
/// trigger with no preset still shows any settings it already carries, so editing an old
/// definition never silently drops them. A webhook's hook secret is excluded — rendered through
/// [`hook_secret_field`]'s own dedicated input, never this generic freeform one.
fn settings_fields(state: State, form: TriggersForm) -> Vec<(String, String, String)> {
    let mut fields: Vec<(String, String, String)> = Vec::new();
    let mut push = |key: String, label: String, hint: String| {
        if !fields.iter().any(|(k, _, _)| *k == key) {
            fields.push((key, label, hint));
        }
    };

    if let Some(id) = form.preset.get()
        && let Some(st) = state.triggers.get()
        && let Some(preset) = st.presets.iter().find(|p| p.id == id)
    {
        for f in &preset.fields {
            push(f.key.clone(), f.label.clone(), f.hint.clone());
        }
    }
    // Anything already set that no preset claims — an old definition, or a key added by hand.
    // The relay keys and the hook-secret reference are excluded: each renders through its own
    // dedicated input below, never this generic freeform one (`relay_endpoint` especially must
    // stay read-only, and the hook secret has its own status line a bare `<input>` can't show).
    for key in form.extra.get().keys() {
        if RELAY_EXTRA_KEYS.contains(&key.as_str()) || key == HOOK_SECRET_REF_KEY {
            continue;
        }
        push(key.clone(), key.clone(), "custom setting".to_string());
    }
    fields
}

/// Render the settings inputs. Each value reaches the code block as `ADI_<KEY>`.
fn extra_fields(state: State, form: TriggersForm) -> AnyView {
    settings_fields(state, form)
        .into_iter()
        .map(|(key, label, hint)| {
            let for_value = key.clone();
            let for_input = key.clone();
            let show_hint = !hint.is_empty();
            view! {
                <div class="adi-field">
                    <label class="adi-field__label" for=format!("trigger-{key}")>{label}</label>
                    <input class="adi-input adi-mono" id=format!("trigger-{key}") autocomplete="off"
                        prop:value=move || form.extra.get().get(&for_value).cloned().unwrap_or_default()
                        on:input=move |ev| set_extra(form.extra, &for_input, event_target_value(&ev)) />
                    {show_hint.then(|| field_hint(hint))}
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

/// The relay section: a picker for the four hosted relays or a
/// self-hosted URL, a retention field, an enterprise relay's licence code, and — once `adi-app`
/// has registered this trigger and written back `relay_endpoint` — the public URL to hand the
/// sender and this node's own public key for a self-host operator's `authorized_keys`. Shown for
/// webhook triggers only: a background/event trigger has no inbound endpoint for a relay to front.
/// The webhook's own hook secret: a reference to an `adi-secrets` secret by name, resolved
/// fresh on every request — never a plaintext value, which the server never even sends to this
/// client (see `handlers::triggers::trigger_dto`'s own doc). An inline, legacy secret shows only
/// as a status line with no input bound to it, since there's no value here to bind one to; the
/// trigger keeps working until it's migrated (`adi-mono triggers migrate-hook-secret`).
fn hook_secret_field(form: TriggersForm) -> AnyView {
    let extra = form.extra;
    let status = form.hook_secret_status;
    let secret_ref = move || extra.get().get(HOOK_SECRET_REF_KEY).cloned().unwrap_or_default();

    view! {
        <div class="adi-field">
            <label class="adi-field__label" for="trigger-hook-secret">"Hook secret"</label>
            <input class="adi-input adi-mono" id="trigger-hook-secret" autocomplete="off"
                placeholder="name of a secret in adi-secrets"
                prop:value=secret_ref
                on:input=move |ev| {
                    form.clear_hook_secret.set(false);
                    let v = event_target_value(&ev);
                    let now_set = !v.trim().is_empty();
                    set_extra(extra, HOOK_SECRET_REF_KEY, v);
                    if now_set {
                        status.set(HookSecretStatus::Ref);
                    }
                } />
            {move || hook_secret_hint(status.get())}
            {move || (status.get() != HookSecretStatus::None).then(|| view! {
                <button class="adi-btn adi-btn--quiet" type="button" on:click=move |_| {
                    form.clear_hook_secret.set(true);
                    set_extra(extra, HOOK_SECRET_REF_KEY, String::new());
                    status.set(HookSecretStatus::None);
                }>"Remove secret"</button>
            })}
        </div>
    }
    .into_any()
}

/// The explanatory line beside the Hook secret field, one per [`HookSecretStatus`] the server
/// could possibly report.
fn hook_secret_hint(status: HookSecretStatus) -> AnyView {
    field_hint(match status {
        HookSecretStatus::None => {
            "optional — callers must present ?secret=… or an X-ADI-Hook-Secret header when one is set"
        }
        HookSecretStatus::Ref => "set — resolved from adi-secrets on every request",
        HookSecretStatus::Inline => {
            "set (inline, legacy) — migrate with `adi-mono triggers migrate-hook-secret <name>` to move it into adi-secrets"
        }
        HookSecretStatus::RefMissing => {
            "names a secret that isn't set in adi-secrets — every call to this hook is refused until it is"
        }
    })
}

fn relay_fields(form: TriggersForm) -> AnyView {
    let extra = form.extra;
    let relay_value = move || extra.get().get("relay").cloned().unwrap_or_default();
    let is_custom = move || {
        let v = relay_value();
        v == "custom" || v.starts_with("custom:")
    };
    let select_value = move || if is_custom() { "custom".to_string() } else { relay_value() };
    let custom_url = move || relay_value().strip_prefix("custom:").unwrap_or_default().to_string();
    let endpoint = move || extra.get().get("relay_endpoint").cloned().filter(|s| !s.is_empty());
    let public_url =
        move || endpoint().and_then(|id| resolve_relay_url(&relay_value()).map(|base| format!("{base}/h/{id}")));

    let url_field: NodeRef<Input> = NodeRef::new();
    let key_field: NodeRef<Input> = NodeRef::new();

    view! {
        <div class="adi-field">
            <label class="adi-field__label" for="trigger-relay">"Webhook relay"</label>
            <select class="adi-input" id="trigger-relay"
                prop:value=select_value
                on:change=move |ev| {
                    let v = event_target_value(&ev);
                    set_extra(extra, "relay", if v == "custom" { "custom:".to_string() } else { v });
                }>
                <option value="">"— direct only, no relay —"</option>
                <option value="public-eu">"Public (EU)"</option>
                <option value="public-us">"Public (US)"</option>
                <option value="enterprise-eu">"Enterprise (EU)"</option>
                <option value="enterprise-us">"Enterprise (US)"</option>
                <option value="custom">"Self-hosted (custom URL)"</option>
            </select>
            {field_hint("lets an outside sender (Stripe, GitHub, …) reach this trigger without exposing \
                         this node directly — blank keeps it direct-only, as today")}
        </div>
        {move || is_custom().then(|| view! {
            <div class="adi-field">
                <label class="adi-field__label" for="trigger-relay-url">"Relay URL"</label>
                <input class="adi-input adi-mono" id="trigger-relay-url" autocomplete="off"
                    placeholder="https://relay.example.com"
                    prop:value=custom_url
                    on:input=move |ev| set_extra(extra, "relay", format!("custom:{}", event_target_value(&ev))) />
                {field_hint("your own self-hosted relay instance")}
            </div>
        })}
        {move || (!relay_value().is_empty()).then(|| view! {
            <div class="adi-field">
                <label class="adi-field__label" for="trigger-relay-retention">"Retention (seconds)"</label>
                <input class="adi-input adi-mono" type="number" min="0" id="trigger-relay-retention"
                    prop:value=move || extra.get().get("relay_retention").cloned().unwrap_or_default()
                    on:input=move |ev| set_extra(extra, "relay_retention", event_target_value(&ev)) />
                {field_hint("how long an undelivered record is kept on the relay — blank uses the relay's own default")}
            </div>
        })}
        {move || relay_value().starts_with("enterprise-").then(|| view! {
            <div class="adi-field">
                <label class="adi-field__label" for="trigger-relay-licence">"Licence code"</label>
                <input class="adi-input adi-mono" id="trigger-relay-licence" autocomplete="off"
                    prop:value=move || extra.get().get("licence_code").cloned().unwrap_or_default()
                    on:input=move |ev| set_extra(extra, "licence_code", event_target_value(&ev)) />
                {field_hint("checked once, at registration, against this install's ADI Enterprise licence")}
            </div>
        })}
        {move || public_url().map(|url| view! {
            <div class="adi-field adi-field--full">
                <span class="adi-field__label">"Public URL — give this to the sender"</span>
                {copy_row(url_field, move || url.clone())}
            </div>
        })}
        {move || (!relay_value().is_empty()).then(|| view! {
            <div class="adi-field adi-field--full">
                <span class="adi-field__label">"This node's public key — for a self-host relay's authorized_keys"</span>
                {move || form.relay_key.get().map_or_else(
                    || view! { <span class="adi-muted">"loading…"</span> }.into_any(),
                    |key| copy_row(key_field, move || key.clone()).into_any(),
                )}
            </div>
        })}
    }
    .into_any()
}

/// The settings that get submitted: the keys the form is currently offering, plus the relay keys
/// (`relay_fields` owns those through its own dedicated inputs, never `settings_fields`'
/// generic list) — trimmed and non-empty, so switching presets never leaks the previous one's
/// settings into the save.
fn current_extras(state: State, form: TriggersForm) -> BTreeMap<String, String> {
    let offered = settings_fields(state, form);
    form.extra
        .get()
        .into_iter()
        .map(|(k, v)| (k, v.trim().to_string()))
        .filter(|(k, v)| {
            !v.is_empty()
                && (RELAY_EXTRA_KEYS.contains(&k.as_str())
                    || k == HOOK_SECRET_REF_KEY
                    || offered.iter().any(|(key, _, _)| key == k))
        })
        .collect()
}

/// Render the triggers table body: a loading/empty placeholder, or one row per trigger.
fn trigger_rows(state: State, form: TriggersForm, log: TriggersLogView) -> AnyView {
    let table = state.tables.triggers;
    let mut triggers = match rows_or_status(
        table,
        state.triggers.get().map(|v| v.triggers),
        "No triggers yet — define one below.",
        read_error(state, "/api/triggers"),
    ) {
        Ok(rows) => rows,
        Err(placeholder) => return placeholder,
    };
    sort_rows(&mut triggers, table.sort.get(), trigger_key, |t| {
        Key::text(&t.name)
    });
    triggers
        .into_iter()
        .map(|t| {
            let del_name = t.name.clone();
            let t_edit = t.clone();
            let edit = menu_item(state, "Edit", false, move || {
                load_trigger_into_form(form, &t_edit)
            });
            let delete = menu_item(state, "Delete", true, move || {
                apply_triggers(
                    state,
                    None,
                    fetch::delete_trigger(del_name.clone()),
                );
            });
            let mut items = trigger_menu_items(state, log, &t);
            items.extend([trigger_toggle_item(state, &t), edit, delete]);
            let actions = row_actions(state, format!("trigger:{}", t.name), (), items);
            view! { <TableRow state=table cell=move |col| trigger_cell(col, &t) actions=actions/> }
                .into_any()
        })
        .collect::<Vec<_>>()
        .into_any()
}

/// A trigger's sort key under `col`. Shared with a project's Triggers panel, which shows a subset
/// of the same columns over the same rows.
pub(crate) fn trigger_key(t: &TriggerDto, col: &str) -> Key {
    match col {
        "Launches" => Key::text(launch_label(t)),
        "Project" => Key::maybe(t.project.as_deref()),
        // Running first on a descending sort — "what is actually up?" is the question this column
        // gets clicked for. A disabled trigger sorts below a merely-stopped one.
        "Status" => Key::Int(i64::from(t.running) + i64::from(t.enabled)),
        "Last run" => Key::num(t.last_fired_at.unwrap_or(0)),
        // "Name", and any header without a key of its own.
        _ => Key::text(&t.name),
    }
}

/// One trigger's cell under `col`. Matching the header text — the same key the sort uses — is
/// what lets the user hide and reorder columns without the row builder knowing about it, and is
/// also how a project's narrower panel works from the same code. Shared with that panel.
pub(crate) fn trigger_cell(col: &str, t: &TriggerDto) -> AnyView {
    match col {
        // The kind is a word; the runtime is the interpreter's name, so it is the mono half.
        "Launches" => view! {
            <span>
                {t.kind.clone()}
                <span class="adi-muted">" · "</span>
                <span class="adi-mono adi-muted">{runtime_or_default(&t.runtime)}</span>
            </span>
        }
        .into_any(),
        "Project" => match &t.project {
            Some(p) if !p.trim().is_empty() => {
                view! { <span><span class="adi-chip">{p.clone()}</span></span> }.into_any()
            }
            _ => view! { <span><span class="adi-muted">"—"</span></span> }.into_any(),
        },
        "Status" => view! { <span>{status_cell(t)}</span> }.into_any(),
        "Last run" => {
            let fired = t.last_fired_at.map_or_else(|| "—".to_string(), fmt_date);
            view! { <span class="adi-muted adi-tabnums">{fired}</span> }.into_any()
        }
        // "Name", and anything the layout offers that this match doesn't name.
        _ => {
            let hook_hint = (t.kind == "webhook").then(|| format!("/api/hooks/{}", t.name));
            view! {
                <span title=t.description.clone()>
                    <span>{t.name.clone()}</span>
                    {hook_hint.map(|h| view! {
                        <span class="adi-cell__sub adi-mono adi-muted">{h}</span>
                    })}
                </span>
            }
            .into_any()
        }
    }
}

/// How a trigger launches, and in what language — the two facts its kind and runtime carry.
fn launch_label(t: &TriggerDto) -> String {
    let runtime = runtime_or_default(&t.runtime);
    format!("{} · {runtime}", t.kind)
}

/// A trigger's status cell. A background trigger reports its supervised process (up, for how
/// long, and whether it keeps dying); a webhook just reports whether its endpoint is open.
pub(crate) fn status_cell(t: &TriggerDto) -> AnyView {
    if t.running {
        let uptime = t.uptime_secs.map_or_else(String::new, fmt_uptime);
        let detail = match (t.pid, t.restarts) {
            (Some(pid), 0) => format!("pid {pid} · up {uptime}"),
            (Some(pid), n) => format!("pid {pid} · up {uptime} · {n} restart(s)"),
            (None, _) => uptime,
        };
        return view! {
            <span class="adi-tstatus" data-status="ready">"Running"</span>
            <span class="adi-cell__sub adi-tabnums">{detail}</span>
        }
        .into_any();
    }
    if !t.enabled {
        return view! { <span class="adi-tstatus" data-status="archived">"Disabled"</span> }
            .into_any();
    }
    if t.kind == "background" {
        // Enabled but not up: either it is coming up, or its code block can't start at all.
        return view! {
            <span class="adi-tstatus" data-status="archived">"Stopped"</span>
            <span class="adi-cell__sub">"check the log"</span>
        }
        .into_any();
    }
    view! { <span class="adi-tstatus" data-status="ready">"Listening"</span> }.into_any()
}

/// The run items of a trigger's ⋯ menu, shared between the global Triggers table and a project's
/// panel. A background trigger offers Restart (its process is supervised); a webhook offers Fire
/// (nothing else would ever run it by hand). A background trigger that *isn't* running can still
/// be fired once, which is how you test one without enabling it. Log follows either.
pub(crate) fn trigger_menu_items(
    state: State,
    log: TriggersLogView,
    t: &TriggerDto,
) -> Vec<AnyView> {
    let log_name = t.name.clone();
    let run_action = if t.running {
        let name = t.name.clone();
        menu_item(state, "Restart", false, move || {
            restart_trigger(state, name.clone());
        })
    } else {
        let name = t.name.clone();
        menu_item(state, "Fire", false, move || {
            fire_trigger(state, name.clone())
        })
    };
    vec![
        run_action,
        menu_item(state, "Log", false, move || open_log(log, log_name.clone())),
    ]
}

/// The Enable/Disable toggle as a kebab menu item — shared by the global table and a project's
/// panel, so a trigger can be paused (or re-armed) from either without a second inline button.
pub(crate) fn trigger_toggle_item(state: State, t: &TriggerDto) -> AnyView {
    let toggle = t.clone();
    let label = if t.enabled { "Disable" } else { "Enable" };
    menu_item(state, label, false, move || toggle_trigger(state, &toggle))
}

/// Run a triggers mutation: set the returned list, or flash the error;
/// toggles `busy` around the request when a form is driving it.
fn apply_triggers<F>(state: State, busy: Option<RwSignal<bool>>, fut: F)
where
    F: std::future::Future<Output = Result<TriggersState, String>> + 'static,
{
    apply_mutation(state, busy, |s, t| s.triggers.set(Some(t)), fut);
}

/// Run a trigger's code block once, by hand (the Fire action). Says nothing: the row's own Last
/// run and status follow the fire, which is the report.
fn fire_trigger(state: State, name: String) {
    spawn_local(async move {
        match fetch::fire_trigger(name).await {
            Ok(res) => {
                state.triggers.set(Some(res.state));
                state.flash.set(None);
            }
            Err(e) => state.flash.set(Some(Flash::err(e))),
        }
    });
}

/// Replace a supervised background trigger's process (the Restart action). The new pid shows
/// up on the next refresh, once the supervisor has actually cycled it.
fn restart_trigger(state: State, name: String) {
    spawn_local(async move {
        match fetch::restart_trigger(name).await {
            Ok(res) => {
                state.triggers.set(Some(res.state));
                state.flash.set(None);
            }
            Err(e) => state.flash.set(Some(Flash::err(e))),
        }
    });
}

/// Flip a trigger's enabled flag by re-saving its full definition (the server preserves
/// `created_at`). For a background trigger this is its power switch: the supervisor starts or
/// stops the process to match.
fn toggle_trigger(state: State, t: &TriggerDto) {
    let body = SaveTrigger {
        name: t.name.clone(),
        kind: t.kind.clone(),
        runtime: t.runtime.clone(),
        code: t.code.clone(),
        preset: t.preset.clone(),
        description: t.description.clone(),
        enabled: !t.enabled,
        project: t.project.clone(),
        extra: t.extra.clone(),
        events: t.events.clone(),
        trigger_on: t.trigger_on.clone(),
        // A toggle never touches the hook secret; `extra` already carries a reference verbatim
        // (just a name) and the server carries an untouched inline value forward on its own.
        clear_hook_secret: false,
    };
    apply_triggers(
        state,
        None,
        fetch::save_trigger(body),
    );
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

/// The log panel: the watched trigger's most recent output, refreshed each second. For a
/// background trigger that is a running history across restarts; for a webhook it is the last
/// delivery. Renders nothing while no trigger is being watched.
///
/// Reads [`LogPhase`] rather than the snapshot directly, so the section is rebuilt only when the
/// trigger goes from no-snapshot-yet to never-fired to having a log, not on every poll that
/// merely lands more output; the log itself leans on [`log_pane`] to stay one element for as
/// long as it's open.
pub(crate) fn log_view(log: TriggersLogView) -> Option<AnyView> {
    let name = log.name.get()?;
    let phase = Memo::new(move |_| LogPhase::from_ran(log.log.with(|s| s.as_ref().map(|s| s.fired))));
    Some(
        view! {
            <section class="adi-panel">
                <div class="adi-panel__head">
                    <h2 class="adi-panel__title">{format!("Log — {name}")}</h2>
                    <span class="adi-spacer"></span>
                    {move || {
                        let fired_at = log.log.with(|s| s.as_ref().and_then(|s| s.fired_at)).map(fmt_date).unwrap_or_default();
                        (!fired_at.is_empty()).then(|| view! {
                            <span class="adi-updated">{format!("last wrote {fired_at}")}</span>
                        })
                    }}
                    <button class="adi-btn adi-btn--quiet" on:click=move |_| log.close()>"Close"</button>
                </div>
                {move || match phase.get() {
                    LogPhase::Loading => view! { <div class="adi-empty">"Loading…"</div> }.into_any(),
                    LogPhase::Empty => view! {
                        <div class="adi-empty">"This trigger has never run — its log is empty."</div>
                    }
                    .into_any(),
                    LogPhase::Ready => log_pane("adi-term", move || {
                        log.log.with(|s| s.as_ref().map(|s| s.output.clone()).unwrap_or_default())
                    })
                    .into_any(),
                }}
            </section>
        }
        .into_any(),
    )
}

/// Load an existing trigger into the create/edit form (the Edit action).
fn load_trigger_into_form(form: TriggersForm, t: &TriggerDto) {
    form.name.set(t.name.clone());
    form.kind.set(t.kind.clone());
    form.runtime.set(runtime_or_default(&t.runtime));
    form.preset.set(t.preset.clone());
    form.project.set(t.project.clone().unwrap_or_default());
    form.description.set(t.description.clone());
    form.code.set(t.code.clone());
    form.enabled.set(t.enabled);
    // `t.extra` never carries the inline hook secret's value (the server drops it entirely) —
    // `secret_ref`, not being sensitive, rides along as-is when that's how this trigger is gated.
    form.extra.set(t.extra.clone());
    form.hook_secret_status.set(t.hook_secret_status);
    form.clear_hook_secret.set(false);
    form.events.set(t.events.join("\n"));
    form.trigger_on.set(t.trigger_on.clone());
    form.editing.set(Some(t.name.clone()));
    scroll_top();
}

/// Reset the create/edit form back to a blank "New trigger" state.
fn clear_trigger_form(form: TriggersForm) {
    form.name.set(String::new());
    form.kind.set(String::new());
    form.runtime.set(String::new());
    form.preset.set(None);
    form.project.set(String::new());
    form.description.set(String::new());
    form.code.set(String::new());
    form.enabled.set(true);
    form.extra.set(BTreeMap::new());
    form.hook_secret_status.set(HookSecretStatus::None);
    form.clear_hook_secret.set(false);
    form.events.set(String::new());
    form.trigger_on.set(Vec::new());
    form.editing.set(None);
}
