//! The LLM backends page: the registry of ways to answer a turn, the live record of which are
//! spent right now, and the two global switches over how a run moves between them.
//!
//! A **backend** is one complete way to answer a turn — a login, a model, the dials that model runs
//! with, how it says "I am out", and how to ask whether it is back. It is set up once here and
//! reused by every agent. It is never built on another backend: reuse happens in an agent's ordered
//! list, on the agent's own page, and the only thing a row there may respell is the model and the
//! dials. So this page shows one flat table and one whole-object editor — there is no parent to
//! pick, and nothing on it inherits.
//!
//! The column worth the page is **Status**. A hold is written down centrally, against the
//! credential rather than the agent, so sixteen agents sharing a login see one answer between them:
//! the first run to hit the limit records it, and the other fifteen skip that backend without
//! spending a turn to find out. That is why the table is polled — the interesting changes here are
//! made by the background prober and by other people's runs, not by this screen.

use adi_webapp_api::types::{
    ContextWarningDto, LimitRuleDto, LlmBackendDto, LlmBackendsDto, ProbeDto, SaveLlmBackend,
    SaveLlmSettings,
};
use adi_ui::{Row as TableRow, Table};
use leptos::prelude::*;

use crate::fetch;
use crate::routing::{Route, go_global};
use crate::state::{Flash, LlmBackendsForm, State, read_error};
use crate::ui::{
    Key, TextField, apply_mutation, confirm, field_hint, flash_view, menu_item, row_actions,
    rows_or_status, sort_rows,
};

/// The registry table. `Login` is the credential a hold is keyed by, so two rows reading the same
/// there are the same subscription — moving from one to the other when it runs out buys nothing,
/// and this column is how that is seen before it is configured rather than after.
pub(crate) const COLS: &[&str] = &[
    "Backend", "Runtime", "Login", "Model", "Dials", "Context", "Status", "Used by", "",
];

/// How often the prober may sweep, as the page offers it. `0` is a real setting and not a blank —
/// it leaves every hold to expire on its own deadline and never spends a request asking.
const SWEEP_CHOICES: [(u64, &str); 6] = [
    (0, "Never — let holds expire on their own"),
    (60, "Every minute"),
    (300, "Every 5 minutes"),
    (900, "Every 15 minutes"),
    (1_800, "Every 30 minutes"),
    (3_600, "Every hour"),
];

/// What a limit rule's error means, and what each answer costs. Only `quota` and `rate` move a run
/// down the list on their own; the other three stop and ask, which is the cautious direction.
const CLASSES: [(&str, &str); 5] = [
    ("quota", "quota — the plan is spent; move down the list"),
    ("rate", "rate — too fast for now; move down the list"),
    ("auth", "auth — the login is broken; stop and ask"),
    ("transient", "transient — a blip; leave it alone"),
    ("unknown", "unknown — not understood; stop and ask"),
];

/// How wide the hold a rule writes reaches.
const SCOPES: [(&str, &str); 2] = [
    ("model", "model — only this model is spent"),
    ("login", "login — the whole subscription is"),
];

/// Where the wait comes from when a rule fires.
const RESUMES: [(&str, &str); 3] = [
    ("from_message", "from the provider's own words, else the fallback"),
    ("retry_after", "from the Retry-After header, else the fallback"),
    ("fixed", "always the fallback below"),
];

/// The LLM backends page: the registry, the editor under it, the warnings a *pair* of records can
/// see, and the two global switches.
pub(crate) fn llm_backends_view(
    state: State,
    form: LlmBackendsForm,
    route: RwSignal<Route>,
) -> AnyView {
    let backends = state.llm_backends;
    view! {
        <section class="adi-panel">
            <div class="adi-panel__head">
                <h2 class="adi-panel__title">"Backends"</h2>
                <span class="adi-updated">{move || summary(backends)}</span>
                <span class="adi-spacer"></span>
                <button class="adi-btn adi-btn--ghost" type="button"
                    on:click=move |_| go_global(state, route, Route::Llm)>
                    "See the traffic"
                </button>
            </div>
            <Table state=state.tables.llm_backends>{move || rows_view(state, form)}</Table>
            <p class="adi-hint">
                "A backend is one whole way to answer a turn. Agents do not carry model settings of \
                 their own — each one lists these in the order it should try them, on its own page."
            </p>
        </section>

        {move || warnings_view(backends)}

        {editor_view(state, form)}
        {settings_view(state)}
    }
    .into_any()
}

/// "6 backends · 1 limited", or nothing until the first load lands.
fn summary(backends: RwSignal<Option<LlmBackendsDto>>) -> String {
    let Some(state) = backends.get() else {
        return String::new();
    };
    let mut text = match state.backends.len() {
        1 => "1 backend".to_string(),
        n => format!("{n} backends"),
    };
    let held = state.backends.iter().filter(|b| b.hold.is_some()).count();
    if held > 0 {
        text.push_str(&format!(" \u{b7} {held} limited"));
    }
    text
}

// ------------------------------------------------------------------- table

/// The registry's rows: a placeholder, or one per backend with its ⋯ menu.
fn rows_view(state: State, form: LlmBackendsForm) -> AnyView {
    let table = state.tables.llm_backends;
    let mut backends = match rows_or_status(
        table,
        state.llm_backends.get().map(|v| v.backends),
        "No backends yet — describe one below, then list it on an agent.",
        read_error(state, "/api/llm/backends"),
    ) {
        Ok(rows) => rows,
        Err(placeholder) => return placeholder,
    };
    // A held backend sorts to the top by default: it is the row that changed on its own, and the
    // only one on the page anybody is waiting on.
    sort_rows(
        &mut backends,
        table.sort.get(),
        |b, col| match col {
            "Runtime" => Key::text(&b.runtime),
            "Login" => Key::text(&b.credential),
            "Model" => Key::text(&b.model),
            "Dials" => Key::count(b.params.len()),
            "Context" => Key::num(b.context_tokens),
            "Status" => Key::num(b.hold.as_ref().map_or(0, |h| h.until)),
            "Used by" => Key::count(b.used_by.len()),
            _ => Key::text(name_of(b)),
        },
        |b| Key::text(format!("{}{}", u8::from(b.hold.is_none()), name_of(b))),
    );
    backends
        .into_iter()
        .map(|b| {
            let row = b.clone();
            let edit = menu_item(state, "Edit", false, move || {
                form.edit(&row);
                scroll_to_editor();
            });
            let held = b.hold.is_some();
            let release_id = b.id.clone();
            let release = held.then(|| {
                menu_item(state, "Release the hold", false, move || {
                    let id = release_id.clone();
                    apply(
                        state,
                        None,
                        format!("Released the hold on {id}."),
                        fetch::release_llm_hold(id.clone()),
                    );
                })
            });
            let delete_id = b.id.clone();
            let used_by = b.used_by.len();
            let delete = menu_item(state, "Delete", true, move || {
                let id = delete_id.clone();
                // The server refuses this while an agent still lists it; saying so here saves the
                // round trip and, more to the point, names the consequence before the click.
                let question = if used_by == 0 {
                    format!("Delete the backend {id}?")
                } else {
                    format!(
                        "{id} is still listed by {used_by} agent(s), which will refuse the \
                         delete. Try anyway?"
                    )
                };
                if !confirm(&question) {
                    return;
                }
                apply(
                    state,
                    None,
                    format!("Deleted the backend {id}."),
                    fetch::delete_llm_backend(id.clone()),
                );
            });
            let menu_key = format!("llm-backend:{}", b.id);
            let items = [Some(edit), release, Some(delete)]
                .into_iter()
                .flatten()
                .collect();
            view! {
                <TableRow state=table cell=move |col| cell(col, &b)
                    actions=row_actions(state, menu_key, (), items)/>
            }
            .into_any()
        })
        .collect::<Vec<_>>()
        .into_any()
}

/// What to call a backend: its label when it has one, else the id it is filed under.
fn name_of(b: &LlmBackendDto) -> String {
    if b.label.is_empty() {
        b.id.clone()
    } else {
        b.label.clone()
    }
}

/// One backend's cell under `col`. Matching the header text — the same key the sort uses — is what
/// lets the user hide and reorder columns without the row builder knowing about it.
fn cell(col: &str, b: &LlmBackendDto) -> AnyView {
    match col {
        "Runtime" => view! { <span class="adi-mono adi-muted">{b.runtime.clone()}</span> }.into_any(),
        "Login" => view! {
            <span class="adi-mono adi-muted" title=b.credential.clone()>{short(&b.credential)}</span>
        }
        .into_any(),
        "Model" => {
            if b.model.is_empty() {
                view! { <span class="adi-muted">"\u{2014}"</span> }.into_any()
            } else {
                view! { <span class="adi-mono">{b.model.clone()}</span> }.into_any()
            }
        }
        "Dials" => dials_cell(b),
        "Context" => {
            if b.context_tokens == 0 {
                // Unstated, not zero — and an unstated context is what makes the agent form's
                // step-down warning stay quiet about this row rather than shout about it.
                view! { <span class="adi-muted" title="Unstated">"\u{2014}"</span> }.into_any()
            } else {
                view! { <span class="adi-tabnums">{tokens(b.context_tokens)}</span> }.into_any()
            }
        }
        "Status" => status_cell(b),
        "Used by" => used_by_cell(b),
        // "Backend", and anything the layout offers that this match doesn't name.
        _ => {
            let sub = (!b.label.is_empty()).then(|| {
                view! { <span class="adi-mono adi-muted">{b.id.clone()}</span> }
            });
            view! { <span>{name_of(b)}</span> {sub} }.into_any()
        }
    }
}

/// The dials, as `temperature, max_tokens` — the names, not the values. The values are on the
/// editor; what a table is for is noticing that this backend has dials and that one does not.
fn dials_cell(b: &LlmBackendDto) -> AnyView {
    if b.params.is_empty() {
        return view! { <span class="adi-muted">"\u{2014}"</span> }.into_any();
    }
    let names: Vec<&str> = b.params.keys().map(String::as_str).collect();
    let title = names.join(", ");
    let full = title.clone();
    view! { <span class="adi-mono adi-muted" title=full>{title}</span> }.into_any()
}

/// The live hold, which is the column the page is really for: available, or spent until a time,
/// with the provider's own words behind the tooltip.
fn status_cell(b: &LlmBackendDto) -> AnyView {
    match &b.hold {
        None => view! {
            <span class="adi-status" data-state="online">
                <span class="adi-status__led"></span>
                <span>"available"</span>
            </span>
        }
        .into_any(),
        Some(hold) => {
            let title = if hold.reason.is_empty() {
                format!("Recorded by {}", hold.set_by)
            } else {
                hold.reason.clone()
            };
            let attempts = (hold.attempts > 0).then(|| {
                view! {
                    <span class="adi-status__uptime">
                        {format!("probed {}\u{d7}", hold.attempts)}
                    </span>
                }
            });
            view! {
                <span class="adi-status" data-state="down" title=title>
                    <span class="adi-status__led"></span>
                    <span>{hold.describe.clone()}</span>
                    {attempts}
                </span>
            }
            .into_any()
        }
    }
}

/// Which agents list this backend — who an edit here reaches.
fn used_by_cell(b: &LlmBackendDto) -> AnyView {
    if b.used_by.is_empty() {
        return view! {
            <span class="adi-muted" title="No agent lists this backend, so nothing runs on it.">
                "unused"
            </span>
        }
        .into_any();
    }
    let title = b.used_by.join(", ");
    b.used_by
        .iter()
        .take(3)
        .map(|agent| view! { <span class="adi-chip">{agent.clone()}</span> }.into_any())
        .chain((b.used_by.len() > 3).then(|| {
            view! { <span class="adi-muted" title=title>{format!("+{}", b.used_by.len() - 3)}</span> }
                .into_any()
        }))
        .collect::<Vec<_>>()
        .into_any()
}

// ---------------------------------------------------------------- warnings

/// The two things only a *pair* of records can say: a row naming a backend that is not here, and a
/// list that steps down in context. Both are warnings and neither blocks anything — a chain simply
/// skips a row it cannot resolve, and a short conversation still fits a smaller window. Hidden
/// entirely when there is nothing to say, so the page does not carry an empty complaint panel.
fn warnings_view(backends: RwSignal<Option<LlmBackendsDto>>) -> AnyView {
    let Some(state) = backends.get() else {
        return ().into_any();
    };
    if state.context_warnings.is_empty() && state.dangling.is_empty() {
        return ().into_any();
    }
    let dangling = (!state.dangling.is_empty()).then(|| {
        let rows: Vec<_> = state
            .dangling
            .iter()
            .map(|d| {
                view! {
                    <li>
                        <span class="adi-chip">{d.agent.clone()}</span>
                        " lists "
                        <span class="adi-mono">{d.backend.clone()}</span>
                        ", which is not here — that row is skipped."
                    </li>
                }
            })
            .collect();
        view! {
            <div class="adi-field">
                <span class="adi-field__label">"Rows naming nothing"</span>
                <ul class="adi-hint">{rows}</ul>
            </div>
        }
    });
    let shrink = (!state.context_warnings.is_empty()).then(|| {
        let rows: Vec<_> = state
            .context_warnings
            .iter()
            .map(|w: &ContextWarningDto| {
                view! {
                    <li>
                        <span class="adi-chip">{w.agent.clone()}</span>
                        " "
                        {w.message.clone()}
                    </li>
                }
            })
            .collect();
        view! {
            <div class="adi-field">
                <span class="adi-field__label">"Lists that step down in context"</span>
                <ul class="adi-hint">{rows}</ul>
            </div>
        }
    });
    view! {
        <section class="adi-panel">
            <div class="adi-panel__head">
                <h2 class="adi-panel__title">"Worth knowing"</h2>
                <span class="adi-spacer"></span>
                <span class="adi-updated">"Neither of these stops anything running"</span>
            </div>
            <div class="adi-panel__body">
                {dangling}
                {shrink}
                <p class="adi-hint">
                    "A conversation moves whole when a backend runs out — nothing is summarized or \
                     cut. If the history will not fit the next backend's window the switch fails and \
                     asks, so a smaller window later in a list is a warning, not a mistake."
                </p>
            </div>
        </section>
    }
    .into_any()
}

// ------------------------------------------------------------------ editor

/// The whole-object editor: every field of one backend, on one form. Whole-object because a backend
/// *is* a handful of fields on one page — unlike an agent, which four different forms save a piece
/// of each, so nothing here is omit-to-keep.
fn editor_view(state: State, form: LlmBackendsForm) -> AnyView {
    let runtimes = move || {
        state
            .agents
            .get()
            .map(|a| a.form.backends)
            .unwrap_or_default()
    };
    view! {
        <section class="adi-panel" id="llm-backend-editor">
            <div class="adi-panel__head">
                <h2 class="adi-panel__title">
                    {move || match form.editing.get() {
                        id if id.is_empty() => "Add a backend".to_string(),
                        id => format!("Edit {id}"),
                    }}
                </h2>
                <span class="adi-spacer"></span>
                {move || (!form.editing.get().is_empty()).then(|| view! {
                    <button class="adi-btn adi-btn--ghost" type="button"
                        on:click=move |_| form.clear()>"New backend"</button>
                })}
            </div>

            <form class="adi-panel__body" on:submit=move |ev| {
                ev.prevent_default();
                submit(state, form);
            }>
                <div class="adi-form adi-form--first">
                    <TextField id="llmb-id" label="Name" mono=true placeholder="anthropic"
                        hint="What agents list it by, and its filename in llm/backends/. \
                              Renaming it re-points every agent row that names it."
                        value=form.id />
                    <TextField id="llmb-label" label="Shown as" placeholder="Anthropic \u{b7} Opus"
                        field_class="adi-field--grow" value=form.label />
                    <div class="adi-field">
                        <label class="adi-field__label" for="llmb-runtime">"Runtime"</label>
                        <select class="adi-input adi-mono" id="llmb-runtime"
                            prop:value=move || form.runtime.get()
                            on:change=move |ev| form.runtime.set(event_target_value(&ev))>
                            <option value="">"\u{2014} pick a runtime \u{2014}"</option>
                            {move || runtimes().into_iter().map(|b| {
                                view! { <option value=b.id>{b.label}</option> }
                            }).collect::<Vec<_>>()}
                        </select>
                    </div>
                </div>

                <div class="adi-form">
                    <TextField id="llmb-model" label="Model" mono=true placeholder="claude-opus-5"
                        field_class="adi-field--grow" value=form.model />
                    <TextField id="llmb-context" label="Context (tokens)" numeric=true
                        placeholder="200000"
                        hint="How much history this backend can hold. Leave it empty when you do \
                              not know — an unstated window warns about nothing."
                        value=form.context_tokens />
                </div>

                <div class="adi-field">
                    <span class="adi-field__label">"Login"</span>
                    {field_hint(
                        "Which credential answers. Two backends naming the same one share a hold, \
                         so moving from one to the other when it runs out buys nothing. An agent \
                         row may respell the model and the dials, never this."
                    )}
                    <div class="adi-form adi-form--first">
                        <TextField id="llmb-settings" label="CLI settings file" mono=true
                            placeholder="~/.claude/settings.json" field_class="adi-field--grow"
                            value=form.settings />
                        <TextField id="llmb-provider" label="Provider" mono=true placeholder="anthropic"
                            value=form.provider />
                        <TextField id="llmb-base-url" label="Base URL" mono=true
                            placeholder="https://api.anthropic.com" field_class="adi-field--grow"
                            value=form.base_url />
                        <TextField id="llmb-key-env" label="API key variable" mono=true
                            placeholder="ANTHROPIC_API_KEY"
                            hint="The name of the variable holding the key \u{2014} never the key \
                                  itself. Store that with adi-mono secrets set."
                            value=form.api_key_env />
                    </div>
                </div>

                <div class="adi-field">
                    <label class="adi-field__label" for="llmb-params">"Dials"</label>
                    {field_hint(
                        "Everything the runtime understands that is not the model or the login, \
                         as a JSON object. An agent row may override these per row."
                    )}
                    <textarea class="adi-textarea adi-mono" id="llmb-params" rows="4"
                        placeholder="{ \"temperature\": 0.2 }"
                        prop:value=move || form.params.get()
                        on:input=move |ev| form.params.set(event_target_value(&ev))></textarea>
                </div>

                {rules_view(form)}
                {probe_view(form)}

                <div class="adi-form">
                    <button class="adi-btn adi-btn--primary" type="submit"
                        prop:disabled=move || form.busy.get()>
                        {move || if form.editing.get().is_empty() { "Add backend" } else { "Save backend" }}
                    </button>
                    {move || (!form.editing.get().is_empty()).then(|| view! {
                        <button class="adi-btn adi-btn--ghost" type="button"
                            on:click=move |_| form.clear()>"Cancel"</button>
                    })}
                </div>
            </form>
            {flash_view(state.flash)}
        </section>
    }
    .into_any()
}

/// The limit rules, in the order they are tried. A backend with none classifies every error as
/// `unknown`, which stops and asks rather than moving down the list — safe, and useless, so the
/// empty state says so instead of looking finished.
fn rules_view(form: LlmBackendsForm) -> AnyView {
    view! {
        <div class="adi-field">
            <span class="adi-field__label">"How it says it is out"</span>
            {field_hint(
                "Each rule matches the error text a failed run came back with; the first match \
                 decides. Only quota and rate move a conversation down the list on their own."
            )}
            {move || {
                let rules = form.rules.get();
                if rules.is_empty() {
                    return view! {
                        <p class="adi-hint">
                            "No rules — every failure on this backend reads as unknown, which stops \
                             and asks instead of moving down the list."
                        </p>
                    }
                    .into_any();
                }
                rules
                    .into_iter()
                    .enumerate()
                    .map(|(i, rule)| rule_row(form, i, &rule))
                    .collect::<Vec<_>>()
                    .into_any()
            }}
            <div class="adi-form adi-form--toolbar">
                <button class="adi-btn adi-btn--ghost" type="button"
                    on:click=move |_| form.rules.update(|rules| rules.push(default_rule()))>
                    "Add a rule"
                </button>
            </div>
        </div>
    }
    .into_any()
}

/// One rule's row. Every control writes straight back into the vector at `i`, so the order on
/// screen is the order they are tried — which is the whole of what a rule's position means.
fn rule_row(form: LlmBackendsForm, i: usize, rule: &LimitRuleDto) -> AnyView {
    let pattern = rule.pattern.clone();
    let fixed = rule.fixed.clone().unwrap_or_default();
    let (class, scope, resume) = (rule.class.clone(), rule.scope.clone(), rule.resume.clone());
    view! {
        <div class="adi-form adi-form--first">
            <div class="adi-field adi-field--grow">
                <label class="adi-field__label" for=format!("llmb-rule-{i}")>"Matches"</label>
                <input class="adi-input adi-input--wide adi-mono" id=format!("llmb-rule-{i}")
                    autocomplete="off" placeholder="usage limit reached"
                    prop:value=pattern
                    on:input=move |ev| {
                        let value = event_target_value(&ev);
                        form.rules.update(|rules| if let Some(r) = rules.get_mut(i) {
                            r.pattern = value;
                        });
                    } />
            </div>
            {rule_select(form, i, "Means", &class, &CLASSES, |r, v| r.class = v)}
            {rule_select(form, i, "Holds", &scope, &SCOPES, |r, v| r.scope = v)}
            {rule_select(form, i, "Waits", &resume, &RESUMES, |r, v| r.resume = v)}
            <div class="adi-field">
                <label class="adi-field__label" for=format!("llmb-fixed-{i}")>"Fallback wait"</label>
                <input class="adi-input adi-mono" id=format!("llmb-fixed-{i}") autocomplete="off"
                    placeholder="4h"
                    prop:value=fixed
                    on:input=move |ev| {
                        let value = event_target_value(&ev);
                        form.rules.update(|rules| if let Some(r) = rules.get_mut(i) {
                            r.fixed = Some(value);
                        });
                    } />
            </div>
            <button class="adi-btn adi-btn--ghost" type="button" title="Remove this rule"
                on:click=move |_| form.rules.update(|rules| { rules.remove(i); })>
                "Remove"
            </button>
        </div>
    }
    .into_any()
}

/// One of a rule's three enum controls. The values are the wire spellings the API round-trips
/// through serde, so what is written here is what the TOML reads.
fn rule_select(
    form: LlmBackendsForm,
    i: usize,
    label: &'static str,
    current: &str,
    options: &'static [(&'static str, &'static str)],
    set: fn(&mut LimitRuleDto, String),
) -> AnyView {
    let id = format!("llmb-{}-{i}", label.to_lowercase());
    let current = current.to_string();
    view! {
        <div class="adi-field">
            <label class="adi-field__label" for=id.clone()>{label}</label>
            <select class="adi-input" id=id
                prop:value=current
                on:change=move |ev| {
                    let value = event_target_value(&ev);
                    form.rules.update(|rules| if let Some(r) = rules.get_mut(i) {
                        set(r, value);
                    });
                }>
                {options.iter().map(|(value, text)| view! {
                    <option value=*value>{*text}</option>
                }).collect::<Vec<_>>()}
            </select>
        </div>
    }
    .into_any()
}

/// The probe: the tiny request the background sweep sends to ask whether a held backend is back.
/// Optional, because a backend that cannot be asked cheaply should not be asked at all — its holds
/// then simply expire on their own deadline.
fn probe_view(form: LlmBackendsForm) -> AnyView {
    view! {
        <div class="adi-field">
            <span class="adi-field__label">"Coming back"</span>
            {field_hint(
                "When a hold's time is up, a background sweep sends this one small request to find \
                 out whether the backend is really back. A chat turn never discovers it."
            )}
            <label class="adi-field adi-field--check">
                <input type="checkbox"
                    prop:checked=move || form.probe_on.get()
                    on:change=move |ev| form.probe_on.set(event_target_checked(&ev)) />
                <span class="adi-field__label">"Probe this backend when its hold is due"</span>
            </label>
            {move || form.probe_on.get().then(|| view! {
                <div class="adi-form adi-form--first">
                    <TextField id="llmb-probe-model" label="Probe with model" mono=true
                        placeholder="the backend's own model"
                        hint="Only when a cheaper model on the same login is worth asking with."
                        value=form.probe_model />
                    <TextField id="llmb-probe-prompt" label="Probe prompt" wide=true
                        placeholder="ping" field_class="adi-field--grow"
                        value=form.probe_prompt />
                </div>
            })}
        </div>
    }
    .into_any()
}

/// A blank rule, in the shape most rules turn out to have: read the provider's own words for the
/// wait, and hold only the model. Both are the cautious end of their choice.
fn default_rule() -> LimitRuleDto {
    LimitRuleDto {
        pattern: String::new(),
        class: "quota".to_string(),
        scope: "model".to_string(),
        resume: "from_message".to_string(),
        fixed: None,
    }
}

/// Validate what has been typed and save the whole object. The two refusals here are the ones the
/// server cannot phrase as well: a backend with no name has no file to live in, and half-typed JSON
/// is a form still being written rather than a backend to store.
fn submit(state: State, form: LlmBackendsForm) {
    let id = form.id.get().trim().to_string();
    if id.is_empty() {
        state
            .flash
            .set(Some(Flash::err("Give the backend a name.".to_string())));
        return;
    }
    let params = match parse_params(&form.params.get()) {
        Ok(params) => params,
        Err(e) => {
            state.flash.set(Some(Flash::err(e)));
            return;
        }
    };
    let editing = form.editing.get();
    let renaming = !editing.is_empty() && editing != id;
    let body = SaveLlmBackend {
        id: id.clone(),
        label: form.label.get().trim().to_string(),
        runtime: form.runtime.get().trim().to_string(),
        model: form.model.get().trim().to_string(),
        context_tokens: form.context_tokens.get().trim().parse().unwrap_or(0),
        settings: form.settings.get().trim().to_string(),
        provider: form.provider.get().trim().to_string(),
        base_url: form.base_url.get().trim().to_string(),
        api_key_env: form.api_key_env.get().trim().to_string(),
        params,
        // A rule that matches nothing is a rule that was started and abandoned; dropping it here
        // keeps an empty row from silently classifying every error on this backend.
        limit_rules: form
            .rules
            .get()
            .into_iter()
            .filter(|r| !r.pattern.trim().is_empty())
            .collect(),
        probe: form.probe_on.get().then(|| ProbeDto {
            model: Some(form.probe_model.get().trim().to_string()),
            prompt: form.probe_prompt.get().trim().to_string(),
        }),
        rename_from: renaming.then(|| editing.clone()),
    };
    let message = if renaming {
        format!("Renamed {editing} to {id}, and re-pointed every agent that listed it.")
    } else if editing.is_empty() {
        format!("Added the backend {id}.")
    } else {
        format!("Saved the backend {id}.")
    };
    // The form clears itself only on success, and only because the save landed — a failed save
    // must leave what was typed exactly where it was, so it can be fixed rather than retyped.
    let form_to_clear = form;
    apply_mutation(
        state,
        Some(form.busy),
        message,
        move |s, fresh: LlmBackendsDto| {
            s.llm_backends.set(Some(fresh));
            form_to_clear.clear();
        },
        fetch::save_llm_backend(body),
    );
}

/// Read the dials box as a JSON object. An empty box is no dials; anything that is not an object is
/// refused by name, because `[1, 2]` and `"temperature"` both parse and neither is a set of dials.
pub(crate) fn parse_params(
    raw: &str,
) -> Result<std::collections::BTreeMap<String, serde_json::Value>, String> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Ok(std::collections::BTreeMap::new());
    }
    match serde_json::from_str::<serde_json::Value>(raw) {
        Ok(serde_json::Value::Object(map)) => Ok(map.into_iter().collect()),
        Ok(_) => Err("The dials must be a JSON object, like { \"temperature\": 0.2 }.".to_string()),
        Err(e) => Err(format!("The dials are not valid JSON: {e}")),
    }
}

// ---------------------------------------------------------------- settings

/// The two global switches. Both save on change and read their current value straight from the
/// registry, so there is nothing here to seed and nothing a poll can overwrite mid-edit.
fn settings_view(state: State) -> AnyView {
    let settings = move || state.llm_backends.get().map(|b| b.settings);
    view! {
        <section class="adi-panel">
            <div class="adi-panel__head">
                <h2 class="adi-panel__title">"How switching behaves"</h2>
                <span class="adi-spacer"></span>
                <span class="adi-updated">"Applies to every run \u{2014} chats, triggers, workers"</span>
            </div>
            <div class="adi-panel__body">
                <label class="adi-field adi-field--check">
                    <input type="checkbox"
                        prop:checked=move || settings().is_some_and(|s| s.ask_on_switch)
                        prop:disabled=move || settings().is_none()
                        on:change=move |ev| {
                            let Some(current) = settings() else { return };
                            let ask = event_target_checked(&ev);
                            apply(state, None,
                                if ask {
                                    "Every switch will ask first.".to_string()
                                } else {
                                    "A quota limit will switch on its own again.".to_string()
                                },
                                fetch::save_llm_settings(SaveLlmSettings {
                                    ask_on_switch: ask,
                                    probe_every: current.probe_every,
                                }));
                        } />
                    <span class="adi-field__label">"Ask before every switch"</span>
                    {field_hint(
                        "Off by default: the point of an agent's list is that running out of quota \
                         does not interrupt the conversation. A broken login or an error the rules \
                         do not recognise stops and asks either way."
                    )}
                </label>
                <div class="adi-field">
                    <label class="adi-field__label" for="llmb-sweep">"Check held backends"</label>
                    {field_hint(
                        "How often the background sweep asks whether a held backend is back. Each \
                         check is a real request against that subscription, so this is a cost as \
                         well as a delay."
                    )}
                    <select class="adi-input" id="llmb-sweep"
                        prop:value=move || settings().map(|s| s.probe_every.to_string()).unwrap_or_default()
                        prop:disabled=move || settings().is_none()
                        on:change=move |ev| {
                            let Some(current) = settings() else { return };
                            let every = event_target_value(&ev).parse().unwrap_or(current.probe_every);
                            apply(state, None,
                                match every {
                                    0 => "Held backends will not be checked.".to_string(),
                                    n => format!("Checking held backends every {n}s."),
                                },
                                fetch::save_llm_settings(SaveLlmSettings {
                                    ask_on_switch: current.ask_on_switch,
                                    probe_every: every,
                                }));
                        }>
                        {move || sweep_options(settings().map_or(0, |s| s.probe_every))}
                    </select>
                </div>
            </div>
        </section>
    }
    .into_any()
}

/// The sweep intervals to offer, with whatever is stored included even when it is not one of them —
/// a value somebody set by hand in `llm/settings.toml` must not be silently changed by opening this
/// page and touching the other switch.
fn sweep_options(current: u64) -> Vec<AnyView> {
    let mut choices: Vec<(u64, String)> = SWEEP_CHOICES
        .iter()
        .map(|(secs, text)| (*secs, (*text).to_string()))
        .collect();
    if !choices.iter().any(|(secs, _)| *secs == current) {
        choices.push((current, format!("Every {current}s")));
        choices.sort_by_key(|(secs, _)| *secs);
    }
    choices
        .into_iter()
        .map(|(secs, text)| view! { <option value=secs.to_string()>{text}</option> }.into_any())
        .collect()
}

// ----------------------------------------------------------------- helpers

/// Run a registry mutation: store the fresh registry it answers with, and flash success or the
/// error. Every endpoint on this page answers with the whole registry, so an edit and the view of
/// it are one round trip.
fn apply<F>(state: State, busy: Option<RwSignal<bool>>, ok_msg: String, fut: F)
where
    F: std::future::Future<Output = Result<LlmBackendsDto, String>> + 'static,
{
    apply_mutation(state, busy, ok_msg, |s, b| s.llm_backends.set(Some(b)), fut);
}

/// Bring the editor into view after **Edit** fills it. The table can be long enough that the form
/// filling below the fold reads as nothing having happened. Best-effort: a browser that will not
/// scroll simply leaves the page where it is, and the form is still filled.
fn scroll_to_editor() {
    if let Some(editor) = web_sys::window()
        .and_then(|w| w.document())
        .and_then(|d| d.get_element_by_id("llm-backend-editor"))
    {
        editor.scroll_into_view();
    }
}

/// A token count as people say one: `200k`, `1.0M`, or the number itself when it is small.
pub(crate) fn tokens(n: u64) -> String {
    if n >= 1_000_000 {
        #[allow(clippy::cast_precision_loss)]
        return format!("{:.1}M", n as f64 / 1_000_000.0);
    }
    if n >= 1_000 {
        return format!("{}k", n / 1_000);
    }
    n.to_string()
}

/// A credential shortened for the table. It is a path or a provider/URL pair and both run long;
/// the full text is on the cell's title.
fn short(credential: &str) -> String {
    if credential.len() <= 28 {
        return credential.to_string();
    }
    format!("\u{2026}{}", &credential[credential.len() - 27..])
}

#[cfg(test)]
mod tests {
    use super::{SWEEP_CHOICES, parse_params, short, tokens};

    #[test]
    fn a_token_count_reads_the_way_people_say_one() {
        assert_eq!(tokens(0), "0");
        assert_eq!(tokens(999), "999");
        assert_eq!(tokens(200_000), "200k");
        assert_eq!(tokens(1_000_000), "1.0M");
        assert_eq!(tokens(2_500_000), "2.5M");
    }

    /// The shortening cuts from the *front*: the tail of a credential is what distinguishes two
    /// logins under the same home directory, and cutting there would make them read alike.
    #[test]
    fn a_long_credential_keeps_its_tail() {
        let long = "/Users/somebody/.config/some-provider/profiles/work/settings.json";
        let short = short(long);
        assert!(short.starts_with('\u{2026}'), "{short}");
        assert!(long.ends_with(short.trim_start_matches('\u{2026}')), "{short}");
        assert_eq!(short.chars().count(), 28);
    }

    #[test]
    fn a_credential_that_fits_is_left_alone() {
        assert_eq!(short("anthropic|claude-opus-5"), "anthropic|claude-opus-5");
    }

    /// An empty dials box is no dials, not a parse error — a backend with nothing to tune is the
    /// ordinary case, and it must be savable.
    #[test]
    fn an_empty_dials_box_is_no_dials() {
        assert!(parse_params("   ").expect("empty parses").is_empty());
    }

    #[test]
    fn dials_must_be_an_object() {
        assert!(parse_params(r#"{"temperature": 0.2}"#).is_ok());
        assert!(parse_params("[1, 2]").is_err());
        assert!(parse_params("\"temperature\"").is_err());
        assert!(parse_params("{oops").is_err());
    }

    /// `0` is a setting — "never sweep" — and the select has to be able to express it, or turning
    /// probing off would be impossible from the page that owns the switch.
    #[test]
    fn the_sweep_choices_offer_switching_it_off() {
        assert!(SWEEP_CHOICES.iter().any(|(secs, _)| *secs == 0));
    }
}
