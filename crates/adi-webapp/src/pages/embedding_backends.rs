//! The Embedding backends page: the registry of ways to turn text into a vector, and which of
//! `indexer`/`knowledge`/`facts` resolves through each.
//!
//! Modelled on [`super::llm_backends`], much smaller. A **backend** is one complete way to embed —
//! a runtime, a model, the dimensions it produces, and the fallbacks it may fail over to — set up
//! once here and named by exactly one consumer's assignment, never built on another backend. There
//! is no hold, no prober, and no context-window shape to show: nothing here is rate-limited the way
//! a chat subscription is, so the one thing worth keeping from the LLM design is its flat,
//! whole-object editor. See `docs/embedding-backends.md`.
//!
//! Page-local, like the Knowledge page: nothing here is watched over the live channel (see
//! `adi-app/src/live.rs`'s own note on this route — no prober, no hold, nothing outside the panel
//! moves it), so it is fetched once when the page opens and every mutation answers with the fresh
//! registry in the same round trip.

use adi_ui::{Icon, Lucide, Row as TableRow, Table};
use leptos::prelude::*;
use wasm_bindgen_futures::spawn_local;

use adi_webapp_api::types::{
    ConsumerAssignmentDto, EmbeddingBackendDto, EmbeddingBackendsDto, SaveEmbeddingBackend,
};

use crate::fetch;
use crate::state::{EmbeddingsConsole, Flash, State};
use crate::ui::{
    Key, TextField, apply_mutation, confirm, field_hint, flash_view, menu_item, row_actions,
    rows_or_placeholder, sort_rows,
};

/// The registry table.
pub(crate) const COLS: &[&str] = &["Backend", "Runtime", "Model", "Dimensions", "Status", ""];

/// The four runtimes this build knows, and what to call each in the picker.
const RUNTIMES: [(&str, &str); 4] = [
    ("candle", "candle — in-process, jina-embeddings-v2-base-code"),
    ("ollama", "ollama — a local model server, one request per text"),
    ("openai", "openai — any OpenAI-compatible /v1/embeddings endpoint"),
    ("hash", "hash — deterministic word-overlap, no model, no network"),
];

/// Which consumers this build knows to ask, in the fixed order the page shows them — matching
/// `adi_embeddings::{CONSUMER_INDEXER, CONSUMER_KNOWLEDGE, CONSUMER_FACTS}` and the API's own copy
/// of this order.
const CONSUMERS: [(&str, &str); 3] = [
    ("indexer", "The code index — symbols and semantic search"),
    ("knowledge", "Knowledge bases — notes, searched by meaning"),
    ("facts", "The facts base — sentences, and the pairs still to decide"),
];

/// The Embedding backends page: the registry, the whole-object editor under it, and how each
/// consumer resolves.
pub(crate) fn embedding_backends_view(state: State, console: EmbeddingsConsole) -> AnyView {
    // Load once when the page opens — this page's data is page-local, so nothing here rides the
    // shell's 4s poll, the same reasoning the Knowledge page's bases table follows.
    Effect::new(move |loaded: Option<()>| {
        if loaded.is_none() {
            spawn_local(refresh(console));
        }
    });

    view! {
        <section class="adi-panel">
            <div class="adi-panel__head">
                <h2 class="adi-panel__title">"Backends"</h2>
                <span class="adi-updated">{move || summary(console.backends)}</span>
                <span class="adi-spacer"></span>
                <button class="adi-btn adi-btn--ghost" type="button"
                    on:click=move |_| spawn_local(refresh(console))>
                    <Icon icon=Lucide::RefreshCw/>"Reload"
                </button>
            </div>
            <Table state=state.tables.embedding_backends>{move || rows_view(state, console)}</Table>
            {move || error_view(console)}
            <p class="adi-hint">
                "A backend is one whole way to turn text into a vector. `indexer`, `knowledge` and \
                 `facts` each resolve through exactly one, set below \u{2014} never built on \
                 another backend, and never mixed: two backends naming different models never rank \
                 against each other."
            </p>
        </section>

        {assignments_view(state, console)}
        {editor_view(state, console)}
    }
    .into_any()
}

/// "4 backends · 1 unavailable", or nothing until the first load lands.
fn summary(backends: RwSignal<Option<EmbeddingBackendsDto>>) -> String {
    let Some(state) = backends.get() else {
        return String::new();
    };
    let mut text = match state.backends.len() {
        1 => "1 backend".to_string(),
        n => format!("{n} backends"),
    };
    let unavailable = state.backends.iter().filter(|b| !b.available).count();
    if unavailable > 0 {
        text.push_str(&format!(" \u{b7} {unavailable} unavailable"));
    }
    text
}

fn error_view(console: EmbeddingsConsole) -> AnyView {
    match console.error.get() {
        Some(e) => view! { <p class="adi-error" role="alert">{e}</p> }.into_any(),
        None => ().into_any(),
    }
}

// ------------------------------------------------------------------- table

fn rows_view(state: State, console: EmbeddingsConsole) -> AnyView {
    let table = state.tables.embedding_backends;
    let mut backends = match rows_or_placeholder(
        table,
        console.backends.get().map(|v| v.backends),
        "No embedding backends yet \u{2014} describe one below, or run `adi-mono embeddings seed` \
         to materialize the defaults.",
    ) {
        Ok(rows) => rows,
        Err(placeholder) => return placeholder,
    };
    // An unavailable backend sorts to the top: it is the row an operator most needs to notice.
    sort_rows(
        &mut backends,
        table.sort.get(),
        |b, col| match col {
            "Runtime" => Key::text(&b.runtime),
            "Model" => Key::text(&b.model),
            "Dimensions" => Key::num(u64::from(b.dimensions)),
            "Status" => Key::num(u64::from(b.available)),
            _ => Key::text(name_of(b)),
        },
        |b| Key::text(format!("{}{}", u8::from(b.available), name_of(b))),
    );
    backends
        .into_iter()
        .map(|b| {
            let row = b.clone();
            let edit = menu_item(state, "Edit", false, move || {
                console.edit(&row);
                scroll_to_editor();
            });
            let delete_id = b.id.clone();
            let used_by = b.used_by.clone();
            let delete = menu_item(state, "Delete", true, move || {
                let id = delete_id.clone();
                // The server refuses this while a consumer still names it; saying so here saves
                // the round trip and names the consequence before the click.
                let question = if used_by.is_empty() {
                    format!("Delete the backend {id}?")
                } else {
                    format!(
                        "{id} is still assigned to {}, which will refuse the delete. Try anyway?",
                        used_by.join(", ")
                    )
                };
                if !confirm(&question) {
                    return;
                }
                apply(
                    state,
                    console,
                    None,
                    format!("Deleted the backend {id}."),
                    fetch::delete_embedding_backend(id.clone()),
                );
            });
            let menu_key = format!("embedding-backend:{}", b.id);
            view! {
                <TableRow state=table cell=move |col| cell(col, &b)
                    actions=row_actions(state, menu_key.clone(), (), vec![edit, delete])/>
            }
            .into_any()
        })
        .collect::<Vec<_>>()
        .into_any()
}

fn name_of(b: &EmbeddingBackendDto) -> String {
    if b.label.is_empty() { b.id.clone() } else { b.label.clone() }
}

fn cell(col: &str, b: &EmbeddingBackendDto) -> AnyView {
    match col {
        "Runtime" => view! { <span class="adi-mono adi-muted">{b.runtime.clone()}</span> }.into_any(),
        "Model" => {
            if b.model.is_empty() {
                view! { <span class="adi-muted">"\u{2014}"</span> }.into_any()
            } else {
                view! { <span class="adi-mono">{b.model.clone()}</span> }.into_any()
            }
        }
        "Dimensions" => {
            if b.dimensions == 0 {
                view! { <span class="adi-muted">"\u{2014}"</span> }.into_any()
            } else {
                view! { <span class="adi-tabnums">{b.dimensions.to_string()}</span> }.into_any()
            }
        }
        "Status" => status_cell(b),
        _ => {
            let sub = (!b.label.is_empty())
                .then(|| view! { <span class="adi-cell__sub adi-mono">{b.id.clone()}</span> });
            view! { <span>{name_of(b)}</span> {sub} }.into_any()
        }
    }
}

/// Whether this binary can actually build the backend — the one piece of live state this page
/// carries, in place of the LLM page's hold.
fn status_cell(b: &EmbeddingBackendDto) -> AnyView {
    if b.available {
        view! {
            <span class="adi-status" data-state="online">
                <span class="adi-status__led"></span>
                <span>"available"</span>
            </span>
        }
        .into_any()
    } else {
        view! {
            <span class="adi-status" data-state="down"
                title="Rebuild this binary with the `candle` feature to build this runtime.">
                <span class="adi-status__led"></span>
                <span>"unavailable in this binary"</span>
            </span>
        }
        .into_any()
    }
}

// ---------------------------------------------------------- consumer assignments

/// How each consumer currently resolves, and whether that assignment would actually work — the
/// page's answer to the question an operator comes here with, without building an embedder to find
/// out (see this module's own doc on why nothing here calls `adi_embeddings::resolve`).
fn assignments_view(state: State, console: EmbeddingsConsole) -> AnyView {
    view! {
        <section class="adi-panel">
            <div class="adi-panel__head">
                <h2 class="adi-panel__title">"What each consumer resolves to"</h2>
            </div>
            <div class="adi-panel__body">
                {move || {
                    let Some(dto) = console.backends.get() else {
                        return view! { <p class="adi-hint">"Loading\u{2026}"</p> }.into_any();
                    };
                    CONSUMERS
                        .iter()
                        .map(|(consumer, blurb)| {
                            let row = dto
                                .assignments
                                .iter()
                                .find(|a| a.consumer == *consumer)
                                .cloned();
                            assignment_row(state, console, &dto, consumer, blurb, row)
                        })
                        .collect::<Vec<_>>()
                        .into_any()
                }}
            </div>
            <p class="adi-hint">
                "Two backends naming the same model may fail over between each other; nothing else \
                 does \u{2014} wanting a different model for a consumer means pointing it at a \
                 different backend here, not adding one as a fallback."
            </p>
        </section>
    }
    .into_any()
}

fn assignment_row(
    state: State,
    console: EmbeddingsConsole,
    dto: &EmbeddingBackendsDto,
    consumer: &'static str,
    blurb: &'static str,
    row: Option<ConsumerAssignmentDto>,
) -> AnyView {
    let current = row.as_ref().map(|r| r.backend.clone()).unwrap_or_default();
    let resolvable = row.as_ref().is_some_and(|r| r.resolvable);
    let has_assignment = !current.is_empty();
    let options = dto.backends.clone();
    let assignments = dto.assignments.clone();
    view! {
        <div class="adi-form adi-form--first">
            <div class="adi-field adi-field--grow">
                <label class="adi-field__label">{consumer}</label>
                {field_hint(blurb)}
                <select class="adi-input adi-mono"
                    prop:value=current.clone()
                    on:change=move |ev| {
                        let backend = event_target_value(&ev);
                        reassign(state, console, assignments.clone(), consumer, backend);
                    }>
                    <option value="">"\u{2014} unassigned \u{2014}"</option>
                    {options.iter().map(|b| {
                        view! { <option value=b.id.clone()>{name_of(b)}</option> }
                    }).collect::<Vec<_>>()}
                </select>
            </div>
            <div class="adi-field">
                <span class="adi-field__label">"Status"</span>
                {if !has_assignment {
                    view! { <span class="adi-muted">"unassigned"</span> }.into_any()
                } else if resolvable {
                    view! {
                        <span class="adi-status" data-state="online">
                            <span class="adi-status__led"></span><span>"resolves"</span>
                        </span>
                    }
                    .into_any()
                } else {
                    view! {
                        <span class="adi-status" data-state="down"
                            title="This backend is either gone or unavailable in this binary.">
                            <span class="adi-status__led"></span><span>"will not resolve"</span>
                        </span>
                    }
                    .into_any()
                }}
            </div>
        </div>
    }
    .into_any()
}

/// Save the whole assignments map with one consumer's row changed. The API replaces the map
/// wholesale — there are only three rows, ever, and a whole-object save is simpler than a per-row
/// patch endpoint would be — so every other consumer's current assignment is carried along
/// unchanged.
fn reassign(
    state: State,
    console: EmbeddingsConsole,
    current: Vec<ConsumerAssignmentDto>,
    consumer: &'static str,
    backend: String,
) {
    let mut assignments: std::collections::BTreeMap<String, String> = current
        .into_iter()
        .filter(|a| !a.backend.is_empty())
        .map(|a| (a.consumer, a.backend))
        .collect();
    if backend.is_empty() {
        assignments.remove(consumer);
    } else {
        assignments.insert(consumer.to_string(), backend.clone());
    }
    let message = if backend.is_empty() {
        format!("{consumer} is now unassigned.")
    } else {
        format!("{consumer} now resolves through {backend}.")
    };
    apply(state, console, None, message, fetch::save_embedding_settings(assignments));
}

// ------------------------------------------------------------------ editor

/// The whole-object editor: every field of one backend, on one form. `candle` and `hash` offer
/// nothing to fill in beyond the runtime itself — both take their model and width from nowhere but
/// the one fixed answer each always produces (`adi_embeddings::Runtime::fixed_model`) — while
/// `ollama` asks for a host and `openai` for a base URL and the environment variable its key comes
/// from, both alongside the model and width every non-fixed runtime must state.
fn editor_view(state: State, console: EmbeddingsConsole) -> AnyView {
    view! {
        <section class="adi-panel" id="embedding-backend-editor">
            <div class="adi-panel__head">
                <h2 class="adi-panel__title">
                    {move || match console.editing.get() {
                        id if id.is_empty() => "Add a backend".to_string(),
                        id => format!("Edit {id}"),
                    }}
                </h2>
                <span class="adi-spacer"></span>
                {move || (!console.editing.get().is_empty()).then(|| view! {
                    <button class="adi-btn adi-btn--ghost" type="button"
                        on:click=move |_| console.clear()>"New backend"</button>
                })}
            </div>

            <form class="adi-panel__body" on:submit=move |ev| {
                ev.prevent_default();
                submit(state, console);
            }>
                <div class="adi-form adi-form--first">
                    <TextField id="embb-id" label="Name" mono=true placeholder="ollama"
                        hint="What a consumer's assignment names, and its filename in \
                              embeddings/backends/."
                        value=console.id />
                    <TextField id="embb-label" label="Shown as" placeholder="Local ollama"
                        field_class="adi-field--grow" value=console.label />
                    {runtime_picker(console)}
                </div>

                {move || runtime_fields(console)}

                <div class="adi-field">
                    <TextField id="embb-fallbacks" label="Fallbacks" mono=true wide=true
                        placeholder="ollama-hosted"
                        hint="Other backend ids, comma-separated, tried in order if this one \
                              fails to build or answer \u{2014} accepted only between backends \
                              declaring the same model and width."
                        value=console.fallbacks />
                </div>

                <div class="adi-form">
                    <button class="adi-btn adi-btn--primary" type="submit"
                        prop:disabled=move || console.busy.get() || console.runtime.get().is_empty()>
                        {move || if console.editing.get().is_empty() { "Add backend" } else { "Save backend" }}
                    </button>
                    {move || (!console.editing.get().is_empty()).then(|| view! {
                        <button class="adi-btn adi-btn--ghost" type="button"
                            on:click=move |_| console.clear()>"Cancel"</button>
                    })}
                </div>
            </form>
            {flash_view(state.flash)}
        </section>
    }
    .into_any()
}

fn runtime_picker(console: EmbeddingsConsole) -> AnyView {
    view! {
        <div class="adi-field adi-field--grow">
            <label class="adi-field__label" for="embb-runtime">"Runtime"</label>
            {field_hint(
                "Which of the four ways this build can turn text into a vector. It decides the \
                 rest of this form."
            )}
            <select class="adi-input adi-mono" id="embb-runtime"
                prop:value=move || console.runtime.get()
                on:change=move |ev| console.runtime.set(event_target_value(&ev))>
                <option value="">"\u{2014} pick a runtime \u{2014}"</option>
                {RUNTIMES.iter().map(|(id, label)| {
                    view! { <option value=*id>{*label}</option> }
                }).collect::<Vec<_>>()}
            </select>
        </div>
    }
    .into_any()
}

/// Everything below the runtime picker. Nothing until a runtime is chosen — until then there is no
/// honest answer to what any of these fields should say.
fn runtime_fields(console: EmbeddingsConsole) -> AnyView {
    match console.runtime.get().as_str() {
        "" => view! {
            <p class="adi-hint">
                "Pick a runtime and the rest of the form becomes the questions it actually takes."
            </p>
        }
        .into_any(),
        "candle" | "hash" => view! {
            <p class="adi-hint">
                "This runtime always embeds with one fixed model at one fixed width \u{2014} \
                 nothing to configure here."
            </p>
        }
        .into_any(),
        "ollama" => view! {
            <div class="adi-form adi-form--first">
                <TextField id="embb-host" label="Host" mono=true
                    placeholder="http://127.0.0.1:11434"
                    hint="The local model server's address."
                    value=console.base_url />
                <TextField id="embb-model" label="Model" mono=true
                    placeholder="nomic-embed-text"
                    hint="Every stored vector is recorded against this name."
                    value=console.model />
                <TextField id="embb-dimensions" label="Dimensions" numeric=true
                    placeholder="768"
                    hint="The width of the vectors this model produces."
                    value=console.dimensions />
            </div>
        }
        .into_any(),
        "openai" => view! {
            <div class="adi-form adi-form--first">
                <TextField id="embb-base-url" label="Base URL" mono=true wide=true
                    placeholder="https://api.openai.com/v1"
                    hint="The OpenAI-compatible endpoint's base."
                    field_class="adi-field--grow"
                    value=console.base_url />
                <TextField id="embb-model" label="Model" mono=true
                    placeholder="text-embedding-3-small"
                    hint="Every stored vector is recorded against this name."
                    value=console.model />
                <TextField id="embb-dimensions" label="Dimensions" numeric=true
                    placeholder="1536"
                    hint="The width of the vectors this model produces."
                    value=console.dimensions />
                <TextField id="embb-api-key-env" label="API key env" mono=true
                    placeholder="OPENAI_API_KEY"
                    hint="The environment variable the key is read from \u{2014} never the key \
                          itself; this page never displays or accepts one."
                    value=console.api_key_env />
            </div>
        }
        .into_any(),
        _ => ().into_any(),
    }
}

/// Validate what has been typed and save the whole object. The two refusals here are the ones the
/// server cannot phrase as well: a backend with no name has no file to live in, and a form with no
/// runtime chosen has not decided what the rest of it means yet.
fn submit(state: State, console: EmbeddingsConsole) {
    let id = console.id.get().trim().to_string();
    if id.is_empty() {
        state.flash.set(Some(Flash::err("Give the backend a name.".to_string())));
        return;
    }
    let runtime = console.runtime.get().trim().to_string();
    if runtime.is_empty() {
        state.flash.set(Some(Flash::err("Pick a runtime.".to_string())));
        return;
    }
    // `candle`/`hash` never take a model or width from configuration (`Runtime::fixed_model`), so
    // this form asks for neither and sends blank/zero — the server fills in the one fixed answer.
    let fixed = matches!(runtime.as_str(), "candle" | "hash");
    let body = SaveEmbeddingBackend {
        id: id.clone(),
        label: console.label.get().trim().to_string(),
        runtime,
        model: if fixed { String::new() } else { console.model.get().trim().to_string() },
        dimensions: if fixed {
            0
        } else {
            console.dimensions.get().trim().parse().unwrap_or(0)
        },
        base_url: console.base_url.get().trim().to_string(),
        api_key_env: console.api_key_env.get().trim().to_string(),
        fallbacks: console
            .fallbacks
            .get()
            .split(',')
            .map(|f| f.trim().to_string())
            .filter(|f| !f.is_empty())
            .collect(),
    };
    let editing = console.editing.get();
    let message = if editing.is_empty() {
        format!("Added the backend {id}.")
    } else {
        format!("Saved the backend {id}.")
    };
    // The form clears itself only on success — a failed save must leave what was typed exactly
    // where it was, so it can be fixed rather than retyped.
    apply_mutation(
        state,
        Some(console.busy),
        message,
        move |_s, fresh: EmbeddingBackendsDto| {
            console.backends.set(Some(fresh));
            console.clear();
        },
        fetch::save_embedding_backend(body),
    );
}

/// Run a registry mutation: store the fresh registry, and flash success or the error. Every
/// endpoint answers with the whole registry, so an edit and the view of it are one round trip.
fn apply<F>(state: State, console: EmbeddingsConsole, busy: Option<RwSignal<bool>>, ok_msg: String, fut: F)
where
    F: std::future::Future<Output = Result<EmbeddingBackendsDto, String>> + 'static,
{
    apply_mutation(state, busy, ok_msg, move |_s, fresh| console.backends.set(Some(fresh)), fut);
}

async fn refresh(console: EmbeddingsConsole) {
    match fetch::embedding_backends().await {
        Ok(s) => {
            console.backends.set(Some(s));
            console.error.set(None);
        }
        Err(e) => console.error.set(Some(e)),
    }
}

/// Bring the editor into view after **Edit** fills it — the table can be long enough that the form
/// filling below the fold reads as nothing having happened.
fn scroll_to_editor() {
    if let Some(editor) = web_sys::window()
        .and_then(|w| w.document())
        .and_then(|d| d.get_element_by_id("embedding-backend-editor"))
    {
        editor.scroll_into_view();
    }
}
