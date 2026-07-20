//! The Triggers page: create, edit, delete, and run trigger definitions.
//!
//! A trigger is a code block plus one fact about *how it launches* — as a **webhook** (live at
//! `/api/hooks/<name>`, the request body becomes its payload) or in the **background** (a
//! long-lived process the app keeps alive while the trigger is enabled). What it *does* comes
//! from its code block, which a **preset** prefills: applying "Telegram bot" fills the kind,
//! the runtime, a working script, and the settings that script reads.
//!
//! Background triggers show live status (up, its pid, how long, how many restarts) and a
//! Restart action; webhook triggers show ▶ Fire. The Log view shows the most recent output for
//! either, re-polled each second while open.

use std::collections::BTreeMap;

use adi_webapp_api::types::{SaveTrigger, TriggerDto, TriggerPreset, TriggersState};
use leptos::prelude::*;
use wasm_bindgen_futures::spawn_local;

use crate::fetch;
use crate::highlight::Lang;
use crate::routing::scroll_top;
use crate::state::{Flash, State, TriggersForm, TriggersLogView};
use crate::ui::{
    TextField, apply_mutation, code_editor, data_table, field_hint, flash_view, fmt_date,
    fmt_uptime, placeholder_row, updated_text,
};

/// The settings input every webhook offers regardless of preset — the platform itself reads it
/// to guard the endpoint, so it isn't any preset's business.
const WEBHOOK_SECRET: (&str, &str, &str) =
    ("secret", "Secret", "optional — callers must pass ?secret=…");

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
        // The settings inputs are derived from the whole form (kind + preset), so they read
        // `extra` through it rather than from a loose signal here.
        extra: _,
        events,
        editing,
        busy,
    } = form;
    let projects = state.projects;
    view! {
        {move || log_view(log)}

        <section class="adi-panel">
            <div class="adi-panel__head">
                <span class="adi-chip adi-mono" title="Triggers defined">
                    {move || triggers.get().map_or_else(|| "\u{2014}".to_string(),
                        |t| t.triggers.len().to_string())}
                </span>
                <span class="adi-updated">{move || updated_text(triggers, secs_since)}</span>
            </div>

            {data_table(&["Name", "Launches", "Project", "Status", "Last run", ""], move || trigger_rows(state, form, log))}
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
                <button class="adi-btn adi-btn--link" type="button"
                    on:click=move |_| clear_trigger_form(form)>"New trigger"</button>
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
                };
                editing.set(Some(nm.clone()));
                apply_triggers(state, Some(busy), format!("Saved trigger “{nm}”."),
                    fetch::save_trigger(body));
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
                <TextField id="trigger-description" label="Description" placeholder="what this trigger does"
                    wide=true field_class="adi-field--grow" value=description />
                <label class="adi-field adi-field--check">
                    <input type="checkbox"
                        prop:checked=move || enabled.get()
                        on:change=move |ev| enabled.set(event_target_checked(&ev)) />
                    <span class="adi-field__label">"Enabled"</span>
                </label>
                {move || extra_fields(state, form)}
                {move || (kind.get() == "event").then(|| view! {
                    <div class="adi-field" style="flex:1 1 100%; min-width:0">
                        <label class="adi-field__label" for="trigger-events">"Events"</label>
                        <textarea class="adi-input adi-mono" id="trigger-events" rows="3"
                            placeholder="adi.tasks.*"
                            prop:value=move || events.get()
                            on:input=move |ev| events.set(event_target_value(&ev))></textarea>
                        {field_hint("one pattern per line — * matches one segment, ** the tail (e.g. adi.tasks.*)")}
                    </div>
                })}
                {move || (kind.get() == "event").then(|| event_catalog_view(state, form))}
                <div class="adi-field" style="flex:1 1 100%; min-width:0">
                    <label class="adi-field__label" for="trigger-code">"Code block"</label>
                    // The same editor the store file page uses, so a trigger's code gets the
                    // highlighting its language deserves — and follows the runtime picker above.
                    {code_editor(move || code_lang(&runtime.get()), code, "adi-code--form", "trigger-code")}
                    {field_hint(move || code_hint(&kind.get(), &runtime.get()))}
                </div>
                <button class="adi-btn adi-btn--primary" type="submit" prop:disabled=move || busy.get()>
                    {move || if editing.get().is_some() { "Update trigger" } else { "Create trigger" }}
                </button>
            </form>
            {flash_view(flash)}
            <div class="adi-hint">
                "A webhook trigger is live at " <code>"/api/hooks/<name>"</code>
                " (POST or GET; add " <code>"?secret=…"</code> " when one is set). A background
                 trigger runs for as long as it is enabled — the app restarts it if it exits,
                 and editing its code restarts it. Every setting above reaches the code block as "
                <code>"$ADI_<KEY>"</code> "."
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
                <button class="adi-btn" type="button" title=title
                    data-status=selected.then_some("ready")
                    on:click=move |_| apply_preset(form, &applied)>{label}</button>
            }
        })
        .collect::<Vec<_>>();
    view! {
        <div class="adi-field" style="flex:1 1 100%; min-width:0">
            <span class="adi-field__label">"Start from a preset"</span>
            <div class="adi-table__actions" style="display:flex; flex-wrap:wrap; gap:var(--space-2)">
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
            let title = if e.payload.trim().is_empty() {
                e.summary
            } else {
                format!("{}\npayload: {}", e.summary, e.payload)
            };
            view! {
                <button class="adi-btn" type="button" title=title
                    on:click=move |_| add_event_pattern(form.events, &insert)>{label}</button>
            }
        })
        .collect::<Vec<_>>();
    view! {
        <div class="adi-field" style="flex:1 1 100%; min-width:0">
            <span class="adi-field__label">"Available events"</span>
            <div class="adi-table__actions" style="display:flex; flex-wrap:wrap; gap:var(--space-2)">
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

/// The settings inputs for the form as it stands: whatever the applied preset declares, plus a
/// webhook's `secret`. A trigger with no preset still shows any settings it already carries, so
/// editing an old definition never silently drops them.
fn settings_fields(state: State, form: TriggersForm) -> Vec<(String, String, String)> {
    let mut fields: Vec<(String, String, String)> = Vec::new();
    let mut push = |key: String, label: String, hint: String| {
        if !fields.iter().any(|(k, _, _)| *k == key) {
            fields.push((key, label, hint));
        }
    };

    if form.kind.get() == "webhook" {
        let (key, label, hint) = WEBHOOK_SECRET;
        push(key.to_string(), label.to_string(), hint.to_string());
    }
    if let Some(id) = form.preset.get()
        && let Some(st) = state.triggers.get()
        && let Some(preset) = st.presets.iter().find(|p| p.id == id)
    {
        for f in &preset.fields {
            push(f.key.clone(), f.label.clone(), f.hint.clone());
        }
    }
    // Anything already set that no preset claims — an old definition, or a key added by hand.
    for key in form.extra.get().keys() {
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

/// The settings that get submitted: only the keys the form is currently offering, trimmed and
/// non-empty — switching presets never leaks the previous one's settings into the save.
fn current_extras(state: State, form: TriggersForm) -> BTreeMap<String, String> {
    let offered = settings_fields(state, form);
    form.extra
        .get()
        .into_iter()
        .map(|(k, v)| (k, v.trim().to_string()))
        .filter(|(k, v)| !v.is_empty() && offered.iter().any(|(key, _, _)| key == k))
        .collect()
}

/// Render the triggers table body: a loading/empty placeholder, or one row per trigger.
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
            let launches = launch_label(&t);
            let project_cell = match &t.project {
                Some(p) if !p.trim().is_empty() => {
                    let p = p.clone();
                    view! { <span class="adi-chip adi-mono">{p}</span> }.into_any()
                }
                _ => view! { <span class="adi-muted">"—"</span> }.into_any(),
            };
            let hook_hint = (t.kind == "webhook").then(|| format!("/api/hooks/{}", t.name));
            let fired = t.last_fired_at.map_or_else(|| "—".to_string(), fmt_date);
            let description = t.description.clone();
            view! {
                <tr>
                    <td title=description>
                        <span>{t.name.clone()}</span>
                        {hook_hint.map(|h| view! {
                            <span class="adi-muted adi-mono" style="font-size:var(--text-sm); display:block">{h}</span>
                        })}
                    </td>
                    <td class="adi-mono">{launches}</td>
                    <td>{project_cell}</td>
                    <td>{status_cell(&t)}</td>
                    <td class="adi-mono adi-muted">{fired}</td>
                    <td class="adi-table__actions">
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
            <span class="adi-muted adi-mono" style="font-size:var(--text-sm); display:block">{detail}</span>
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
            <span class="adi-muted" style="font-size:var(--text-sm); display:block">"check the log"</span>
        }
        .into_any();
    }
    view! { <span class="adi-tstatus" data-status="ready">"Listening"</span> }.into_any()
}

/// The per-row run actions, shared between the global Triggers table and a project's panel.
/// A background trigger offers Restart (its process is supervised); a webhook offers ▶ Fire
/// (nothing else would ever run it by hand). A background trigger that *isn't* running can
/// still be fired once, which is how you test one without enabling it.
pub(crate) fn trigger_actions(state: State, log: TriggersLogView, t: &TriggerDto) -> AnyView {
    let log_name = t.name.clone();
    let toggle = t.clone();
    let toggle_label = if t.enabled { "Disable" } else { "Enable" };
    let run_action = if t.running {
        let name = t.name.clone();
        view! {
            <button class="adi-btn adi-btn--link" title="replace the running process"
                on:click=move |_| restart_trigger(state, name.clone())>"↻ Restart"</button>
        }
        .into_any()
    } else {
        let name = t.name.clone();
        view! {
            <button class="adi-btn adi-btn--link" title="run the code block once, now"
                on:click=move |_| fire_trigger(state, name.clone())>"▶ Fire"</button>
        }
        .into_any()
    };
    view! {
        {run_action}
        " "
        <button class="adi-btn adi-btn--link" title="show the most recent output"
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

/// Run a trigger's code block once, by hand (the ▶ Fire action). The success flash comes from
/// the server — its message carries the spawned pid.
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

/// Replace a supervised background trigger's process (the ↻ Restart action). The new pid shows
/// up on the next refresh, once the supervisor has actually cycled it.
fn restart_trigger(state: State, name: String) {
    spawn_local(async move {
        match fetch::restart_trigger(name).await {
            Ok(res) => {
                state.triggers.set(Some(res.state));
                state.flash.set(Some(Flash::ok(res.message)));
            }
            Err(e) => state.flash.set(Some(Flash::err(e))),
        }
    });
}

/// Flip a trigger's enabled flag by re-saving its full definition (the server preserves
/// `created_at`). For a background trigger this is its power switch: the supervisor starts or
/// stops the process to match.
fn toggle_trigger(state: State, t: &TriggerDto) {
    let verb = if t.enabled { "Disabled" } else { "Enabled" };
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
    };
    apply_triggers(
        state,
        None,
        format!("{verb} {}.", t.name),
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
            <div class="adi-empty">"This trigger has never run — its log is empty."</div>
        }
        .into_any(),
        Some(s) => view! { <pre class="adi-term">{s.output}</pre> }.into_any(),
    };
    Some(
        view! {
            <section class="adi-panel">
                <div class="adi-panel__head">
                    <h2 class="adi-panel__title">{format!("Log — {name}")}</h2>
                    <span class="adi-spacer"></span>
                    {(!fired_at.is_empty()).then(|| view! {
                        <span class="adi-muted" style="font-size:var(--text-sm)">{format!("last wrote {fired_at}")}</span>
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
    form.runtime.set(runtime_or_default(&t.runtime));
    form.preset.set(t.preset.clone());
    form.project.set(t.project.clone().unwrap_or_default());
    form.description.set(t.description.clone());
    form.code.set(t.code.clone());
    form.enabled.set(t.enabled);
    form.extra.set(t.extra.clone());
    form.events.set(t.events.join("\n"));
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
    form.events.set(String::new());
    form.editing.set(None);
}
