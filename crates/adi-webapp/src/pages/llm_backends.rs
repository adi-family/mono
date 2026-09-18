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

use std::collections::{BTreeMap, BTreeSet};

use adi_webapp_api::types::{
    AgentFieldOwner, AgentFormField, AgentFormFieldKind, AgentFormSpec, ContextWarningDto,
    LimitRuleDto, LlmBackendDto, LlmBackendsDto, ProbeDto, SaveLlmBackend, SaveLlmSettings,
};
use adi_ui::{Row as TableRow, Table};
use leptos::prelude::*;

use wasm_bindgen_futures::spawn_local;

use crate::fetch;
use crate::routing::{Route, go_global};
use crate::state::{Flash, LlmBackendsForm, State, read_error};
use crate::ui::{
    Key, TextField, apply_mutation, confirm, field_hint, flash_view, menu_item, row_actions,
    rows_or_status, sort_rows, test_verdict_view,
};

use super::agents::field_applies;

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
                form.edit(&row, &declared_dials(spec(state).as_ref()));
                scroll_to_editor();
            });
            let held = b.hold.is_some();
            let release_id = b.id.clone();
            let release = held.then(|| {
                menu_item(state, "Release the hold", false, move || {
                    let id = release_id.clone();
                    apply(state, None, fetch::release_llm_hold(id.clone()));
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
                apply(state, None, fetch::delete_llm_backend(id.clone()));
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
            // The id under the label, not beside it: inline, the two run together into one word —
            // `Anthropic · Opusanthropic` — because neither carries a separator of its own.
            let sub = (!b.label.is_empty()).then(|| {
                view! { <span class="adi-cell__sub adi-mono">{b.id.clone()}</span> }
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
///
/// The runtime is asked first, and everything under it is that runtime's own questions: the login
/// it can be pointed at, the models it offers, the dials it understands. A form that showed all of
/// them at once would be asking for a provider and an API key variable to configure a logged-in
/// CLI, which is four fields that do nothing and one wrong answer waiting to be typed.
fn editor_view(state: State, form: LlmBackendsForm) -> AnyView {
    // The schema through a memo, not read straight from the agents state: that state is polled
    // every second and carries the live runs, so a section that subscribed to it directly would be
    // rebuilt — mid-word, under the cursor — every time an agent somewhere started a turn. The
    // spec itself changes only when the server is replaced.
    let schema = Memo::new(move |_| spec(state));
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
                    {runtime_picker(schema, form)}
                </div>

                {move || runtime_sections(form, schema)}

                <div class="adi-form">
                    <button class="adi-btn adi-btn--primary" type="submit"
                        prop:disabled=move || form.busy.get() || form.runtime.get().is_empty()>
                        {move || if form.editing.get().is_empty() { "Add backend" } else { "Save backend" }}
                    </button>
                    <button class="adi-btn adi-btn--ghost" type="button"
                        title="Sends a real request through this backend, on your own account — \
                               billed like any other turn."
                        prop:disabled=move || form.testing.get() || form.runtime.get().is_empty()
                        on:click=move |_| run_test(state, form)>
                        {move || if form.testing.get() { "Testing\u{2026}" } else { "Test" }}
                    </button>
                    {move || (!form.editing.get().is_empty()).then(|| view! {
                        <button class="adi-btn adi-btn--ghost" type="button"
                            on:click=move |_| form.clear()>"Cancel"</button>
                    })}
                    {move || (!form.testing.get()).then(|| test_verdict_view(form.test_result.get()))}
                </div>
            </form>
            {flash_view(state.flash)}
        </section>
    }
    .into_any()
}

/// The runtime select — the first question, and the one every question after it depends on.
fn runtime_picker(schema: Memo<Option<AgentFormSpec>>, form: LlmBackendsForm) -> AnyView {
    view! {
        <div class="adi-field adi-field--grow">
            <label class="adi-field__label" for="llmb-runtime">"Runtime"</label>
            {field_hint(
                "What actually answers a turn. It decides the rest of this form \u{2014} a \
                 vendor CLI signs in on its own, an API needs a key."
            )}
            <select class="adi-input adi-mono" id="llmb-runtime"
                prop:value=move || form.runtime.get()
                on:change=move |ev| form.runtime.set(event_target_value(&ev))>
                {move || {
                    let runtimes = schema.get().map(|s| s.backends).unwrap_or_default();
                    let first = if runtimes.is_empty() {
                        "\u{2014} loading runtimes \u{2014}"
                    } else {
                        "\u{2014} pick a runtime \u{2014}"
                    };
                    view! {
                        <option value="">{first}</option>
                        {runtimes.into_iter().map(|b| {
                            view! { <option value=b.id>{b.label}</option> }
                        }).collect::<Vec<_>>()}
                    }
                }}
            </select>
        </div>
    }
    .into_any()
}

/// Everything below the runtime: the login it takes, the model it runs, its dials, and the two
/// blocks about running out. Nothing until a runtime is chosen, because until then there is no
/// honest answer to what any of them should say.
fn runtime_sections(form: LlmBackendsForm, schema: Memo<Option<AgentFormSpec>>) -> AnyView {
    let runtime = form.runtime.get();
    if runtime.is_empty() {
        return view! {
            <p class="adi-hint">
                "Pick a runtime and the rest of the form becomes the questions it actually takes \
                 \u{2014} the login it is pointed at, the models it offers, and the dials it \
                 understands."
            </p>
        }
        .into_any();
    }
    let Some(spec) = schema.get() else {
        return view! { <p class="adi-hint">"Loading what this runtime takes\u{2026}"</p> }.into_any();
    };
    view! {
        {login_view(form, &spec, &runtime)}
        {model_view(form, &spec, &runtime)}
        {dials_view(form, &spec, &runtime)}
        {rules_view(form)}
        {probe_view(form)}
    }
    .into_any()
}

// ------------------------------------------------------------------- login

/// The login block: only the ways *this* runtime can be pointed at a credential.
///
/// A vendor CLI has none — it answers on whatever it is logged into — and saying so is the point of
/// the block, because the alternative is four empty boxes that read as configuration somebody
/// forgot to do. Anything already typed that this runtime cannot use is named rather than hidden:
/// it is dropped on save, and a login that changes silently is a hold recorded against a
/// subscription nobody is using.
fn login_view(form: LlmBackendsForm, spec: &AgentFormSpec, runtime: &str) -> AnyView {
    let provider = form.provider.get();
    let fields = fields_for(spec, AgentFieldOwner::Credential, runtime, &provider);
    // Which of the four this runtime cannot read — a fact about the schema, so it is settled here.
    // *Whether* one of them is filled in is a fact about what is being typed, and is read inside
    // the closure below: read here it would subscribe this whole section to every keystroke in a
    // login box, and rebuild the box being typed into.
    let unread: Vec<LoginField> = CREDENTIALS
        .iter()
        .filter(|(name, _)| !fields.iter().any(|f| f.name == *name))
        .copied()
        .collect();
    let body = if fields.is_empty() {
        view! {
            <p class="adi-hint">
                "This runtime signs in by itself \u{2014} it answers on whatever its CLI is logged \
                 into, and there is nothing to point it at here. Every backend on it therefore \
                 shares one hold: when the subscription behind it runs out, they all do."
            </p>
        }
        .into_any()
    } else {
        view! {
            <div class="adi-form adi-form--first">
                {fields.into_iter()
                    .map(|field| credential_field(form, &field))
                    .collect::<Vec<_>>()}
            </div>
        }
        .into_any()
    };
    view! {
        <div class="adi-field">
            <span class="adi-field__label">"Login"</span>
            {field_hint(
                "Which credential answers. Two backends naming the same one share a hold, so \
                 moving from one to the other when it runs out buys nothing. An agent row may \
                 respell the model and the dials, never this."
            )}
            {body}
            {move || stale_login_view(form, &unread)}
        </div>
    }
    .into_any()
}

/// The line that names login fields carrying a value this runtime will not read. Said before the
/// click rather than discovered after it: the save drops them, and a login that changed without
/// being mentioned is a hold recorded against the wrong subscription.
fn stale_login_view(
    form: LlmBackendsForm,
    unread: &[LoginField],
) -> AnyView {
    let stale: Vec<&str> = unread
        .iter()
        .filter(|(_, signal)| !signal(form).get().trim().is_empty())
        .map(|(name, _)| *name)
        .collect();
    if stale.is_empty() {
        return ().into_any();
    }
    let (is_are, it_them) = if stale.len() == 1 {
        ("is", "it")
    } else {
        ("are", "them")
    };
    // The names are the keys as the manifest spells them, so they are mono; the sentence around
    // them is not.
    let names: Vec<AnyView> = stale
        .iter()
        .enumerate()
        .map(|(i, name)| {
            let separator = (i > 0).then_some(", ");
            view! { {separator} <span class="adi-mono">{(*name).to_string()}</span> }.into_any()
        })
        .collect();
    view! {
        <p class="adi-hint">
            {names}
            {format!(
                " {is_are} set from another runtime, which this one does not read \u{2014} saving \
                 drops {it_them}."
            )}
        </p>
    }
    .into_any()
}

/// One login field: the schema's name for it, and the signal on this form that holds it.
type LoginField = (&'static str, fn(LlmBackendsForm) -> RwSignal<String>);

/// The four login fields and the signal each one is typed into. The names are the schema's, so a
/// field named here that the schema stops declaring simply stops being offered.
const CREDENTIALS: [LoginField; 4] = [
    ("settings", |f| f.settings),
    ("provider", |f| f.provider),
    ("base_url", |f| f.base_url),
    ("api_key_env", |f| f.api_key_env),
];

/// One login field, bound to its own signal rather than to the dials map — a credential is a field
/// of the backend, not one of its params, and the store refuses it as a param.
fn credential_field(form: LlmBackendsForm, field: &AgentFormField) -> AnyView {
    let Some((_, signal)) = CREDENTIALS.iter().find(|(name, _)| *name == field.name) else {
        // A credential the schema declares and this page has no signal for. Silent rather than
        // broken: the field simply is not offered until somebody adds the signal.
        return ().into_any();
    };
    let value = signal(form);
    control(
        field,
        move || value.get(),
        move |text| value.set(text),
        "llmb-cred",
    )
}

// ------------------------------------------------------------------- model

/// The model this backend runs, with the chosen runtime's own suggestions as chips, and how much
/// history it can hold.
///
/// The suggestions are the runtime's, not this page's: which aliases a Claude CLI takes and which
/// model ids the adi loop takes is server knowledge, and the same list is what the agent form
/// offers.
fn model_view(form: LlmBackendsForm, spec: &AgentFormSpec, runtime: &str) -> AnyView {
    let option = spec.backends.iter().find(|b| b.id == runtime);
    let placeholder = option
        .map(|b| b.model_placeholder.clone())
        .filter(|p| !p.is_empty())
        .unwrap_or_else(|| "model alias".to_string());
    let suggestions = option.map(|b| b.model_suggestions.clone()).unwrap_or_default();
    view! {
        <div class="adi-form">
            <div class="adi-field adi-field--grow">
                <label class="adi-field__label" for="llmb-model">"Model"</label>
                {field_hint(
                    "What this backend answers on. Two models on one subscription are two \
                     backends, not one \u{2014} that is what lets a limit on one leave the other \
                     running."
                )}
                {(!suggestions.is_empty()).then(|| view! {
                    <div class="adi-toolpick">
                        {suggestions.into_iter().map(|model| {
                            let pressed = model.clone();
                            let clicked = model.clone();
                            view! {
                                <button type="button" class="adi-toolpick__chip"
                                    aria-pressed=move || (form.model.get() == pressed).to_string()
                                    on:click=move |_| {
                                        if form.model.get() == clicked {
                                            form.model.set(String::new());
                                        } else {
                                            form.model.set(clicked.clone());
                                        }
                                    }>
                                    {model}
                                </button>
                            }
                        }).collect::<Vec<_>>()}
                    </div>
                })}
                <input class="adi-input adi-input--wide adi-mono" id="llmb-model" autocomplete="off"
                    placeholder=placeholder
                    prop:value=move || form.model.get()
                    on:input=move |ev| form.model.set(event_target_value(&ev)) />
            </div>
            <TextField id="llmb-context" label="Context (tokens)" numeric=true
                placeholder="200000"
                hint="How much history this backend can hold. Leave it empty when you do not \
                      know — an unstated window warns about nothing."
                value=form.context_tokens />
        </div>
    }
    .into_any()
}

// ------------------------------------------------------------------- dials

/// The dials this runtime understands, as the controls it declares for them — a select of the
/// efforts it takes, a number box for a budget — instead of a JSON object to be spelled correctly
/// from memory.
///
/// Two things are deliberately still text. A dial the runtime does not declare is kept and shown
/// rather than quietly dropped, because a hand-written backend may carry one; and anything whose
/// value is not a single word or number stays JSON, because that is the only form it has.
fn dials_view(form: LlmBackendsForm, spec: &AgentFormSpec, runtime: &str) -> AnyView {
    let provider = form.provider.get();
    let fields = fields_for(spec, AgentFieldOwner::Dial, runtime, &provider);
    let declared: BTreeSet<String> = fields.iter().map(|f| f.name.clone()).collect();
    let empty = fields.is_empty();
    let waiting = empty && runtime == ADI_HARNESS && provider.trim().is_empty();
    view! {
        <div class="adi-field">
            <span class="adi-field__label">"Dials"</span>
            {field_hint(
                "How hard the model runs. An agent row may respell any of these for itself; \
                 what the agent decides \u{2014} its prompt, its tools, how freely it may act \
                 \u{2014} is on the agent, not here."
            )}
            {(!empty).then(|| view! {
                <div class="adi-form adi-form--first">
                    {fields.into_iter().map(|field| dial_field(form, &field)).collect::<Vec<_>>()}
                </div>
            })}
            {waiting.then(|| view! {
                <p class="adi-hint">
                    "Pick the provider above and its own dials appear here \u{2014} each API takes \
                     a different set."
                </p>
            })}
            {(empty && !waiting).then(|| view! {
                <p class="adi-hint">
                    "This runtime declares no dials of its own. Anything it does understand can \
                     still be written below."
                </p>
            })}
            {move || undeclared_view(form, &declared)}
        </div>
        // Its own field, not a second label inside the one above: `.adi-field` puts every direct
        // label in grid row 1, so two of them land on top of each other.
        <div class="adi-field">
            <label class="adi-field__label" for="llmb-extra-dials">"Anything else"</label>
            {field_hint(
                "Dials with no control above, as a JSON object \u{2014} a knob this runtime has \
                 gained, or a value that is a list rather than a word. Written onto the run \
                 exactly as it stands. A name that also has a control above is read from that \
                 control, unless the control is empty."
            )}
            <textarea class="adi-textarea adi-llmb__short adi-mono" id="llmb-extra-dials" rows="4"
                placeholder="{ \"stop\": [\"\\n\\n\"] }"
                prop:value=move || form.extra_dials.get()
                on:input=move |ev| form.extra_dials.set(event_target_value(&ev))></textarea>
        </div>
    }
    .into_any()
}

/// One dial, bound to its name in the dials map.
fn dial_field(form: LlmBackendsForm, field: &AgentFormField) -> AnyView {
    let get = field.name.clone();
    let set = field.name.clone();
    control(
        field,
        move || dial_text(form, &get),
        move |text| set_dial(form, &set, text),
        "llmb-dial",
    )
}

/// The dials this backend carries that the chosen runtime does not declare — after a runtime change
/// most often, or from a manifest written by hand. Editable, so they can be corrected or emptied,
/// and named as odd, so nothing about the backend is invisible on the page that edits it.
///
/// They write on **change** rather than on every keystroke: this list is rebuilt whenever the dials
/// map is written, and a box rebuilt under a cursor is a box you cannot type into.
fn undeclared_view(form: LlmBackendsForm, declared: &BTreeSet<String>) -> AnyView {
    let strays: Vec<(String, String)> = form
        .dials
        .get()
        .into_iter()
        .filter(|(name, _)| !declared.contains(name))
        .collect();
    if strays.is_empty() {
        return ().into_any();
    }
    view! {
        <p class="adi-hint">
            "Set on this backend, but not something the chosen runtime declares \u{2014} kept as \
             it is, and saved as it is."
        </p>
        <div class="adi-form adi-form--first">
            {strays.into_iter().map(|(name, value)| {
                let id = format!("llmb-stray-{name}");
                let label = name.clone();
                let edited = name.clone();
                let removed = name.clone();
                view! {
                    <div class="adi-field">
                        <label class="adi-field__label adi-mono" for=id.clone()>{label}</label>
                        <input class="adi-input adi-mono" id=id autocomplete="off"
                            prop:value=value
                            on:change=move |ev| set_dial(form, &edited, event_target_value(&ev)) />
                    </div>
                    <button class="adi-btn adi-btn--ghost" type="button"
                        title=format!("Remove the dial {removed}")
                        on:click=move |_| set_dial(form, &removed, String::new())>
                        "Remove"
                    </button>
                }
            }).collect::<Vec<_>>()}
        </div>
    }
    .into_any()
}

// ---------------------------------------------------------------- controls

/// Render one schema field as the control it declares, bound to whatever holds its value here.
///
/// The agent form has renderers of its own for the same kinds, and they are not shared: those bind
/// to that form's named signals and carry its layout, and a single renderer parameterised over both
/// would be longer than the two. What *is* shared is the schema — the labels, hints, options and
/// filters all arrive from the server — so the two forms cannot disagree about what a runtime takes.
fn control(
    field: &AgentFormField,
    get: impl Fn() -> String + Send + Sync + 'static,
    set: impl Fn(String) + Send + Sync + 'static,
    prefix: &str,
) -> AnyView {
    let id = format!("{prefix}-{}", field.name.replace('_', "-"));
    let label = field.label.clone();
    let hint = field.hint.clone();
    let placeholder = field.placeholder.clone();
    let options = field.options.clone();
    let grow = if field.wide { "adi-field adi-field--grow" } else { "adi-field" };
    let mut class = String::from("adi-input");
    if field.wide {
        class.push_str(" adi-input--wide");
    }
    if field.mono {
        class.push_str(" adi-mono");
    }
    let numeric = field.numeric || matches!(field.kind, AgentFormFieldKind::Number);
    let hint_view = (!hint.is_empty()).then(|| field_hint(hint));
    let label_view = view! { <label class="adi-field__label" for=id.clone()>{label}</label> };
    match field.kind {
        AgentFormFieldKind::Checkbox => view! {
            <label class="adi-field adi-field--check">
                <input type="checkbox"
                    prop:checked=move || get() == "true"
                    on:change=move |ev| set(if event_target_checked(&ev) {
                        "true".to_string()
                    } else {
                        String::new()
                    }) />
                <span class="adi-field__label">{field.label.clone()}</span>
                {hint_view}
            </label>
        }
        .into_any(),
        AgentFormFieldKind::Select => view! {
            <div class=grow>
                {label_view}
                {hint_view}
                <select class="adi-input" id=id
                    prop:value=get
                    on:change=move |ev| set(event_target_value(&ev))>
                    {options.into_iter().map(|opt| {
                        view! { <option value=opt.value>{opt.label}</option> }
                    }).collect::<Vec<_>>()}
                </select>
            </div>
        }
        .into_any(),
        AgentFormFieldKind::Textarea => view! {
            <div class=grow>
                {label_view}
                {hint_view}
                <textarea class="adi-textarea" id=id rows="3" placeholder=placeholder
                    prop:value=get
                    on:input=move |ev| set(event_target_value(&ev))></textarea>
            </div>
        }
        .into_any(),
        // Text, Number, and the two pickers the agent form draws specially — a backend's model has
        // its own control on this page, and its tools are the agent's, so neither can reach here.
        _ => view! {
            <div class=grow>
                {label_view}
                {hint_view}
                <input class=class id=id autocomplete="off" placeholder=placeholder
                    inputmode=if numeric { "numeric" } else { "text" }
                    prop:value=get
                    on:input=move |ev| set(event_target_value(&ev)) />
            </div>
        }
        .into_any(),
    }
}

// ------------------------------------------------------------ the schema

/// The `harness:adi` runtime, whose dials depend on a second choice — the provider it calls.
/// Must match the id the API's form spec serves.
const ADI_HARNESS: &str = "harness:adi";

/// The form schema, which the API owns and this page borrows: `None` until `/api/agents` lands.
fn spec(state: State) -> Option<AgentFormSpec> {
    state.agents.get().map(|a| a.form)
}

/// The fields of one owner that `runtime` takes — its login fields, or its dials.
fn fields_for(
    spec: &AgentFormSpec,
    owner: AgentFieldOwner,
    runtime: &str,
    provider: &str,
) -> Vec<AgentFormField> {
    spec.fields
        .iter()
        .filter(|f| f.owner == owner)
        .filter(|f| field_applies(f, runtime, provider))
        .cloned()
        .collect()
}

/// Every dial name any runtime declares — what a loaded backend's params are split on, so a dial
/// with a control somewhere reaches that control and everything else keeps its JSON type.
fn declared_dials(spec: Option<&AgentFormSpec>) -> BTreeSet<String> {
    spec.map(|spec| {
        spec.fields
            .iter()
            .filter(|f| f.owner == AgentFieldOwner::Dial)
            .map(|f| f.name.clone())
            .collect()
    })
    .unwrap_or_default()
}

/// One dial as typed, or empty when it is not set.
fn dial_text(form: LlmBackendsForm, name: &str) -> String {
    form.dials.get().get(name).cloned().unwrap_or_default()
}

/// Write a dial. Empty removes it: a dial set to nothing is a dial that is not set, and writing
/// `""` into the manifest would be a value the runtime then has to interpret.
fn set_dial(form: LlmBackendsForm, name: &str, value: String) {
    form.dials.update(|dials| {
        if value.trim().is_empty() {
            dials.remove(name);
        } else {
            dials.insert(name.to_string(), value);
        }
    });
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

/// The form as it currently stands, as the wire body a save or a test both send — shared so a
/// **Test** asks exactly the backend a **Save** would write, never a slightly different reading of
/// the same boxes.
///
/// The two refusals here are the ones the server cannot phrase as well: a backend with no name has
/// no file to live in, and half-typed JSON is a form still being written rather than a backend to
/// send anywhere.
fn body_from_form(state: State, form: LlmBackendsForm) -> Result<SaveLlmBackend, String> {
    let id = form.id.get().trim().to_string();
    if id.is_empty() {
        return Err("Give the backend a name.".to_string());
    }
    let runtime = form.runtime.get().trim().to_string();
    let spec = spec(state);
    let params = dial_params(form, spec.as_ref())?;
    // Only the login fields this runtime reads are sent. The others are shown as dropped before the
    // click (see `login_view`), and dropping them is the point: a `base_url` left over from an
    // `harness:adi` backend would key this one's holds against a subscription it never calls.
    let login = |name: &str, value: String| -> String {
        let applies = spec.as_ref().is_some_and(|spec| {
            spec.fields.iter().any(|f| {
                f.name == name
                    && f.owner == AgentFieldOwner::Credential
                    && field_applies(f, &runtime, &form.provider.get())
            })
        });
        if applies { value.trim().to_string() } else { String::new() }
    };
    let (settings, provider, base_url, api_key_env) = (
        login("settings", form.settings.get()),
        login("provider", form.provider.get()),
        login("base_url", form.base_url.get()),
        login("api_key_env", form.api_key_env.get()),
    );
    let editing = form.editing.get();
    let renaming = !editing.is_empty() && editing != id;
    Ok(SaveLlmBackend {
        id: id.clone(),
        label: form.label.get().trim().to_string(),
        runtime,
        model: form.model.get().trim().to_string(),
        context_tokens: form.context_tokens.get().trim().parse().unwrap_or(0),
        settings,
        provider,
        base_url,
        api_key_env,
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
    })
}

/// Validate what has been typed and save the whole object.
fn submit(state: State, form: LlmBackendsForm) {
    let body = match body_from_form(state, form) {
        Ok(body) => body,
        Err(e) => {
            state.flash.set(Some(Flash::err(e)));
            return;
        }
    };
    // The form clears itself only on success, and only because the save landed — a failed save
    // must leave what was typed exactly where it was, so it can be fixed rather than retyped.
    let form_to_clear = form;
    apply_mutation(
        state,
        Some(form.busy),
        move |s, fresh: LlmBackendsDto| {
            s.llm_backends.set(Some(fresh));
            form_to_clear.clear();
        },
        fetch::save_llm_backend(body),
    );
}

/// Ask the form as it currently stands, right now — a real, billed request through whichever
/// runtime is chosen. Distinct from [`submit`]: nothing here is written, so the button beside it
/// says so, and the id doesn't have to be filled in yet the way a save requires it to be a filename.
fn run_test(state: State, form: LlmBackendsForm) {
    let mut body = match body_from_form(state, form) {
        Ok(body) => body,
        Err(e) => {
            state.flash.set(Some(Flash::err(e)));
            return;
        }
    };
    // A test needs no name — it is never written anywhere — so a blank id (the ordinary state of a
    // form nobody has named yet) must not be refused the way a save refuses it.
    if body.id.trim().is_empty() {
        body.id = "test".to_string();
    }
    form.test_result.set(None);
    form.testing.set(true);
    spawn_local(async move {
        match fetch::test_llm_backend(body).await {
            Ok(result) => form.test_result.set(Some(result)),
            Err(e) => state.flash.set(Some(Flash::err(e))),
        }
        form.testing.set(false);
    });
}


/// The dials as they go on the wire: the "anything else" box, with every typed control laid over
/// it.
///
/// Each control's value is converted by the **kind its field declares** — a number box writes a
/// number, a checkbox writes a boolean — so what the runtime receives is the type it expects rather
/// than the string a form holds. A dial whose field the schema no longer declares is written as
/// text, which is what it was read as.
fn dial_params(
    form: LlmBackendsForm,
    spec: Option<&AgentFormSpec>,
) -> Result<BTreeMap<String, serde_json::Value>, String> {
    let mut params = parse_params(&form.extra_dials.get())?;
    for (name, text) in form.dials.get() {
        let text = text.trim().to_string();
        if text.is_empty() {
            continue;
        }
        let kind = spec.and_then(|spec| {
            spec.fields
                .iter()
                .find(|f| f.name == name)
                .map(|f| f.kind)
        });
        params.insert(name.clone(), dial_value(kind, &text).ok_or_else(|| {
            format!("{name} takes a number, and {text} is not one.")
        })?);
    }
    Ok(params)
}

/// One dial's text as the value its field's kind calls for, or `None` when the text does not hold
/// one — a number box mid-word, which is a form still being filled in rather than a backend to
/// save.
///
/// A whole number is written as an integer and not as a float. `8192.0` does decode into the
/// runner's `u64` knobs (`arguments.rs` has a deserializer for exactly that, because this form used
/// to send floats), but `llm/backends/*.toml` is edited by hand as well as here, and opening a file
/// to read it should not be what rewrites `8192` in it.
fn dial_value(kind: Option<AgentFormFieldKind>, text: &str) -> Option<serde_json::Value> {
    match kind {
        Some(AgentFormFieldKind::Checkbox) => Some(serde_json::Value::Bool(text == "true")),
        Some(AgentFormFieldKind::Number) => text
            .parse::<i64>()
            .map(serde_json::Value::from)
            .or_else(|_| text.parse::<f64>().map(serde_json::Value::from))
            .ok(),
        _ => Some(serde_json::Value::String(text.to_string())),
    }
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

/// Run a registry mutation: store the fresh registry it answers with, and flash the error if
/// there was one. Every endpoint on this page answers with the whole registry, so an edit and the
/// view of it are one round trip — and the redrawn table is what reports the edit landed.
fn apply<F>(state: State, busy: Option<RwSignal<bool>>, fut: F)
where
    F: std::future::Future<Output = Result<LlmBackendsDto, String>> + 'static,
{
    apply_mutation(state, busy, |s, b| s.llm_backends.set(Some(b)), fut);
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
    use super::{
        AgentFieldOwner, AgentFormField, AgentFormFieldKind, AgentFormSpec, SWEEP_CHOICES,
        declared_dials, dial_value, fields_for, parse_params, short, tokens,
    };

    fn field(name: &str, owner: AgentFieldOwner, ids: &[&str], providers: &[&str]) -> AgentFormField {
        AgentFormField {
            name: name.to_string(),
            label: name.to_string(),
            kind: AgentFormFieldKind::Text,
            placeholder: String::new(),
            hint: String::new(),
            options: Vec::new(),
            backend_ids: ids.iter().map(|i| (*i).to_string()).collect(),
            executors: Vec::new(),
            providers: providers.iter().map(|p| (*p).to_string()).collect(),
            mono: false,
            wide: false,
            numeric: false,
            required: false,
            run_override: false,
            owner,
        }
    }

    fn spec() -> AgentFormSpec {
        AgentFormSpec {
            backends: Vec::new(),
            presets: Vec::new(),
            fields: vec![
                field("settings", AgentFieldOwner::Credential, &["harness:claude-sdk"], &[]),
                field("base_url", AgentFieldOwner::Credential, &["harness:adi"], &[]),
                field("effort", AgentFieldOwner::Dial, &["harness:claude-sdk"], &[]),
                field("top_p", AgentFieldOwner::Dial, &[], &["ollama"]),
                field("system_prompt", AgentFieldOwner::Agent, &[], &[]),
            ],
        }
    }

    /// The whole complaint this form was rebuilt around: a runtime that signs in through its own
    /// CLI must not be asked for a base URL, and one that calls an API must be.
    #[test]
    fn a_runtime_is_only_asked_for_the_login_it_reads() {
        let spec = spec();
        let sdk = fields_for(&spec, AgentFieldOwner::Credential, "harness:claude-sdk", "");
        assert_eq!(names(&sdk), ["settings"]);
        let adi = fields_for(&spec, AgentFieldOwner::Credential, "harness:adi", "");
        assert_eq!(names(&adi), ["base_url"]);
        let cli = fields_for(&spec, AgentFieldOwner::Credential, "pty:codex", "");
        assert!(cli.is_empty(), "{:?}", names(&cli));
    }

    /// The second half of it: a dial scoped to a provider appears when that provider is chosen,
    /// and not before — which is what makes picking one worth doing.
    #[test]
    fn a_provider_scoped_dial_waits_for_its_provider() {
        let spec = spec();
        assert!(fields_for(&spec, AgentFieldOwner::Dial, "harness:adi", "").is_empty());
        let ollama = fields_for(&spec, AgentFieldOwner::Dial, "harness:adi", "ollama");
        assert_eq!(names(&ollama), ["top_p"]);
    }

    /// What a loaded backend's params are split on: every dial name, whichever runtime declares it,
    /// and nothing that belongs to the agent or names a login.
    #[test]
    fn only_dials_are_declared_dials() {
        let spec = spec();
        let declared = declared_dials(Some(&spec));
        assert!(declared.contains("effort") && declared.contains("top_p"));
        assert!(!declared.contains("settings") && !declared.contains("system_prompt"));
        assert!(declared_dials(None).is_empty(), "no schema declares nothing");
    }

    /// A whole number stays whole. `8192.0` decodes fine, but these files are edited by hand too,
    /// and opening one to read it should not rewrite the numbers in it.
    #[test]
    fn a_whole_number_dial_is_saved_as_an_integer() {
        let number = Some(AgentFormFieldKind::Number);
        assert_eq!(dial_value(number, "8192"), Some(serde_json::json!(8192)));
        assert_eq!(dial_value(number, "0.9"), Some(serde_json::json!(0.9)));
        assert_eq!(dial_value(number, "high"), None, "a number box holding a word");
    }

    /// Everything else is text, and a checkbox is a boolean — the field's kind decides, not the
    /// shape of what was typed, so a model called `2` is not saved as a number.
    #[test]
    fn a_dial_takes_the_type_its_field_declares() {
        assert_eq!(
            dial_value(Some(AgentFormFieldKind::Checkbox), "true"),
            Some(serde_json::json!(true))
        );
        assert_eq!(
            dial_value(Some(AgentFormFieldKind::Select), "high"),
            Some(serde_json::json!("high"))
        );
        assert_eq!(dial_value(None, "2"), Some(serde_json::json!("2")));
    }

    fn names(fields: &[AgentFormField]) -> Vec<&str> {
        fields.iter().map(|f| f.name.as_str()).collect()
    }

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
