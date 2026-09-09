//! The agent's model list: the ordered backends it may answer on, best first.
//!
//! This is the whole of an agent's model configuration. The agent itself has no model, no provider
//! and no login — it names backends, which carry all three, and row 1 is what a new conversation
//! starts on. When that backend runs out the run moves down the list on its own, keeping the same
//! prompt, tools, directory and history.
//!
//! Two rules the editor enforces by construction rather than by validating afterwards. A backend is
//! offered **at most once**: the picker only lists what this agent does not already name, so a chain
//! that names one backend twice cannot be built here (a hand-written manifest may still do it, and
//! everything downstream tolerates it — it is simply not a way of working the interface teaches).
//! And an override may respell the **model and the dials only**: there is nowhere in this editor to
//! type a login, because a different credential is a different backend, and a row that quietly
//! repointed one would make the shared hold describe a subscription nobody was using.

use adi_webapp_api::types::{AgentBackendRowDto, LlmBackendDto};
use adi_ui::{Icon, IconSize, Lucide};
use leptos::prelude::*;

use crate::routing::Route;
use crate::state::{AgentsForm, Flash, State};
use crate::ui::field_hint;

use super::super::llm_backends::{parse_params, tokens};

/// The one override key that is not a dial. Kept as a name here because the page cannot link
/// `adi-agents`, where the runner reads the same key.
const MODEL: &str = "model";

/// The Models section of the agent editor: the ordered list, its warnings, and the picker that adds
/// to it.
pub(crate) fn agent_backends_section(state: State, form: AgentsForm) -> AnyView {
    view! {
        <section class="adi-agents__section">
            <h2 class="adi-agents__h2">"Models"</h2>
            <p class="adi-agents__intro">
                "The backends this agent may answer on, best first. A conversation starts on the \
                 first one and moves down when a backend runs out of quota \u{2014} same prompt, \
                 same tools, same history, no interruption. Set the backends themselves up on "
                <a class="adi-link" href=Route::LlmBackends.path()>"LLM backends"</a>
                "; here you only put them in order."
            </p>
            {move || rows_view(state, form)}
            {move || warnings_view(state, form)}
            {move || add_view(state, form)}
        </section>
    }
    .into_any()
}

/// The list itself, or the line that says there is none.
fn rows_view(state: State, form: AgentsForm) -> AnyView {
    let rows = form.llm_rows.get();
    if rows.is_empty() {
        return view! {
            <p class="adi-agents__chain-empty">
                "This agent lists no backends yet, so it still runs on whatever its runtime is \
                 configured with. Add one below and that becomes what answers it."
            </p>
        }
        .into_any();
    }
    let registry = registry(state);
    view! {
        <ol class="adi-agents__chain">
            {rows.iter().enumerate()
                .map(|(i, row)| row_view(state, form, i, row, rows.len(), &registry))
                .collect::<Vec<_>>()}
        </ol>
    }
    .into_any()
}

/// One row: which backend, what it is, and the four things you can do to it.
///
/// The row is draggable *and* carries up/down buttons. The buttons are not a fallback nobody uses —
/// they are how this is done on a touch screen and by keyboard, and reordering three rows is the
/// most common edit on this page.
fn row_view(
    state: State,
    form: AgentsForm,
    i: usize,
    row: &AgentBackendRowDto,
    total: usize,
    registry: &[LlmBackendDto],
) -> AnyView {
    let id = row.backend.clone();
    let known = registry.iter().find(|b| b.id == id);
    let open = form.llm_row_open.get() == Some(i);
    let overrides = row.overrides.clone();
    // Computed here rather than inside the attribute: `i + 1 >= total` written in the view macro
    // parses as an attribute value that never reads `total`.
    let (first, last) = (i == 0, i + 1 >= total);
    view! {
        <li class="adi-agents__chain-row"
            class=("is-dragging", move || form.llm_drag_from.get() == Some(i))
            draggable="true"
            on:dragstart=move |_| form.llm_drag_from.set(Some(i))
            on:dragend=move |_| form.llm_drag_from.set(None)
            on:dragover=move |ev: leptos::ev::DragEvent| {
                // Without this the drop never fires: the browser's default for a dragover is to
                // refuse the drop.
                if form.llm_drag_from.get_untracked().is_some() {
                    ev.prevent_default();
                }
            }
            on:drop=move |ev: leptos::ev::DragEvent| {
                ev.prevent_default();
                if let Some(from) = form.llm_drag_from.get_untracked() {
                    form.llm_rows.update(|rows| move_row(rows, from, i));
                    form.llm_row_open.set(None);
                }
                form.llm_drag_from.set(None);
            }>
            <span class="adi-agents__chain-grip" title="drag to reorder">
                <Icon icon=Lucide::GripVertical size=IconSize::Sm/>
            </span>
            <span class="adi-agents__chain-pos adi-tabnums">{(i + 1).to_string()}</span>
            <span class="adi-agents__chain-name adi-mono">{id.clone()}</span>
            {describe(known, &overrides)}
            <span class="adi-spacer"></span>
            <button class="adi-btn adi-btn--icon-sm" type="button" title="move up"
                prop:disabled=first
                on:click=move |_| {
                    form.llm_rows.update(|rows| move_row(rows, i, i.saturating_sub(1)));
                    form.llm_row_open.set(None);
                }>
                <Icon icon=Lucide::ArrowUp label="Move up"/>
            </button>
            <button class="adi-btn adi-btn--icon-sm" type="button" title="move down"
                prop:disabled=last
                on:click=move |_| {
                    form.llm_rows.update(|rows| move_row(rows, i, i + 1));
                    form.llm_row_open.set(None);
                }>
                <Icon icon=Lucide::ArrowDown label="Move down"/>
            </button>
            <button class="adi-btn adi-btn--link" type="button"
                title="change the model or the dials for this row only"
                on:click=move |_| form.llm_row_open.set(if open { None } else { Some(i) })>
                {if overrides.is_empty() { "Overrides" } else { "Overrides \u{2022}" }}
            </button>
            <button class="adi-btn adi-btn--icon-sm" type="button" title="remove this backend"
                on:click=move |_| {
                    form.llm_rows.update(|rows| { rows.remove(i); });
                    form.llm_row_open.set(None);
                }>
                <Icon icon=Lucide::X label="Remove"/>
            </button>
        </li>
        {open.then(|| overrides_view(state, form, i, row))}
    }
    .into_any()
}

/// What this row actually runs, in one line: the model (the row's own, if it overrides one), the
/// runtime, and how much history the backend can hold. A backend the registry does not have is said
/// so here rather than left to look like an ordinary row.
fn describe(
    known: Option<&LlmBackendDto>,
    overrides: &std::collections::BTreeMap<String, serde_json::Value>,
) -> AnyView {
    let Some(b) = known else {
        return view! {
            <span class="adi-agents__chain-gone" title="no backend of this name is registered">
                "not found \u{2014} this row is skipped"
            </span>
        }
        .into_any();
    };
    let model = overrides
        .get(MODEL)
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_else(|| b.model.clone());
    let mut parts = vec![b.runtime.clone()];
    if !model.is_empty() {
        parts.push(model);
    }
    if b.context_tokens > 0 {
        parts.push(format!("{} ctx", tokens(b.context_tokens)));
    }
    let held = b.hold.as_ref().map(|h| h.describe.clone());
    view! {
        <span class="adi-agents__chain-meta">{parts.join(" \u{00b7} ")}</span>
        {held.map(|text| {
            let title = text.clone();
            view! { <span class="adi-agents__chain-held" title=title>{text}</span> }
        })}
    }
    .into_any()
}

/// One row's overrides: the model it runs instead of the backend's, and the dials it changes.
///
/// Both write on **change** rather than on every keystroke: the list above is rebuilt whenever the
/// rows signal is written, and a box rebuilt under a cursor is a box you cannot type into.
fn overrides_view(state: State, form: AgentsForm, i: usize, row: &AgentBackendRowDto) -> AnyView {
    let model = row
        .overrides
        .get(MODEL)
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_default();
    let dials = dials_text(&row.overrides);
    view! {
        <li class="adi-agents__chain-over">
            <div class="adi-field">
                <label class="adi-field__label" for="agent-chain-model">"Model"</label>
                <input class="adi-input adi-mono" id="agent-chain-model" type="text"
                    placeholder="the backend's own model"
                    prop:value=model
                    on:change=move |ev| {
                        let value = event_target_value(&ev).trim().to_string();
                        form.llm_rows.update(|rows| {
                            if let Some(row) = rows.get_mut(i) {
                                if value.is_empty() {
                                    row.overrides.remove(MODEL);
                                } else {
                                    row.overrides.insert(MODEL.to_string(), value.into());
                                }
                            }
                        });
                    }/>
                {field_hint(
                    "A different model on the same login. Leave it empty to run the backend's own.",
                )}
            </div>
            <div class="adi-field">
                <label class="adi-field__label" for="agent-chain-dials">"Dials"</label>
                <textarea class="adi-textarea adi-agents__short adi-mono" id="agent-chain-dials"
                    rows="3" placeholder="{ \"effort\": \"high\" }"
                    prop:value=dials
                    on:change=move |ev| {
                        match parse_params(&event_target_value(&ev)) {
                            Ok(dials) => form.llm_rows.update(|rows| {
                                if let Some(row) = rows.get_mut(i) {
                                    let model = row.overrides.remove(MODEL);
                                    row.overrides = dials;
                                    if let Some(model) = model {
                                        row.overrides.insert(MODEL.to_string(), model);
                                    }
                                }
                            }),
                            // Left as it was rather than half-applied: the box still shows what was
                            // typed until the next render, and the message says what is wrong with it.
                            Err(e) => state.flash.set(Some(Flash::err(e))),
                        }
                    }></textarea>
                {field_hint(
                    "JSON, laid over the backend's own dials for this agent only. The login is \
                     never overridable \u{2014} a different credential is a different backend.",
                )}
            </div>
        </li>
    }
    .into_any()
}

/// The picker that adds a row, offering every backend this agent does not already name.
///
/// When it names them all there is nothing to add, and the control says so instead of standing there
/// empty — the same reason it points at the backends page when the registry itself is empty.
fn add_view(state: State, form: AgentsForm) -> AnyView {
    let registry = registry(state);
    if registry.is_empty() {
        return view! {
            <p class="adi-agents__chain-empty">
                "No backends are registered yet. Set one up on "
                <a class="adi-link" href=Route::LlmBackends.path()>"LLM backends"</a>
                " and it can be listed here."
            </p>
        }
        .into_any();
    }
    let free = unused(&registry, &form.llm_rows.get());
    if free.is_empty() {
        return view! {
            <p class="adi-agents__chain-empty">
                "This agent already lists every registered backend."
            </p>
        }
        .into_any();
    }
    let first = free[0].id.clone();
    let picked = RwSignal::new(first);
    view! {
        <div class="adi-agents__chain-add">
            <select class="adi-input" aria-label="Backend to add"
                prop:value=move || picked.get()
                on:change=move |ev| picked.set(event_target_value(&ev))>
                {free.into_iter().map(|b| {
                    let label = if b.label.is_empty() { b.id.clone() } else {
                        format!("{} \u{2014} {}", b.id, b.label)
                    };
                    view! { <option value=b.id.clone()>{label}</option> }
                }).collect::<Vec<_>>()}
            </select>
            <button class="adi-btn" type="button"
                on:click=move |_| {
                    let id = picked.get_untracked();
                    if id.is_empty() {
                        return;
                    }
                    form.llm_rows.update(|rows| {
                        if !rows.iter().any(|r| r.backend == id) {
                            rows.push(AgentBackendRowDto {
                                backend: id,
                                overrides: std::collections::BTreeMap::new(),
                            });
                        }
                    });
                }>"Add backend"</button>
        </div>
    }
    .into_any()
}

/// The warnings this list earns as it is edited — shown, never enforced.
///
/// A chain that steps down in context is legitimate and is also a trap: the switch that has to
/// replay a long conversation into a smaller window is the one that fails, and it fails at the
/// moment somebody is mid-chat. Saying so here is the only place it can be said early. The pty
/// warning is the harder one: a runtime that keeps no transcript cannot be handed a conversation at
/// all, so a row like that below the first is a row nothing will ever move onto.
fn warnings_view(state: State, form: AgentsForm) -> AnyView {
    let registry = registry(state);
    let rows = form.llm_rows.get();
    let mut lines = shrink_warnings(&rows, &registry);
    lines.extend(unreplayable_warnings(&rows, &registry));
    if lines.is_empty() {
        return ().into_any();
    }
    view! {
        <ul class="adi-agents__chain-warn">
            {lines.into_iter().map(|line| view! {
                <li>
                    <Icon icon=Lucide::TriangleAlert size=IconSize::Sm/>
                    <span>{line}</span>
                </li>
            }).collect::<Vec<_>>()}
        </ul>
    }
    .into_any()
}

/// The registry as this page reads it — empty until `/api/llm/backends` has landed.
fn registry(state: State) -> Vec<LlmBackendDto> {
    state
        .llm_backends
        .get()
        .map(|b| b.backends)
        .unwrap_or_default()
}

// ----------------------------------------------------------------- the rules

/// Move the row at `from` to `to`, keeping every other row's order. Out-of-range indices are left
/// alone rather than clamped: a drop that landed nowhere should change nothing.
fn move_row(rows: &mut Vec<AgentBackendRowDto>, from: usize, to: usize) {
    if from >= rows.len() || to >= rows.len() || from == to {
        return;
    }
    let row = rows.remove(from);
    rows.insert(to, row);
}

/// The backends this agent does not already name, in registry order — what the add picker offers,
/// and the whole of "each backend at most once".
fn unused(registry: &[LlmBackendDto], rows: &[AgentBackendRowDto]) -> Vec<LlmBackendDto> {
    registry
        .iter()
        .filter(|b| !rows.iter().any(|r| r.backend == b.id))
        .cloned()
        .collect()
}

/// The row's dials — its overrides without the model, as pretty JSON. An empty object is written as
/// an empty box rather than `{}`, so a row with nothing changed reads as nothing changed.
fn dials_text(overrides: &std::collections::BTreeMap<String, serde_json::Value>) -> String {
    let dials: serde_json::Map<String, serde_json::Value> = overrides
        .iter()
        .filter(|(key, _)| key.as_str() != MODEL)
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect();
    if dials.is_empty() {
        return String::new();
    }
    serde_json::to_string_pretty(&dials).unwrap_or_default()
}

/// One line per row that can hold less than something above it — the same rule the server applies to
/// a saved agent (`ResolvedChain::context_shrink_warnings`), applied live as the list is edited.
///
/// A backend that declares no context window is never warned about: unknown is not evidence of
/// small, and a warning nobody can act on is noise.
fn shrink_warnings(rows: &[AgentBackendRowDto], registry: &[LlmBackendDto]) -> Vec<String> {
    let mut warnings = Vec::new();
    let mut largest = 0_u64;
    let mut largest_name = String::new();
    for row in rows {
        let Some(b) = registry.iter().find(|b| b.id == row.backend) else {
            continue;
        };
        if b.context_tokens == 0 {
            continue;
        }
        if largest > 0 && b.context_tokens < largest {
            warnings.push(format!(
                "{} holds {} tokens, less than {} above it ({}) — a switch may not fit. A \
                 conversation that does not fit stops and asks; it is never cut down to size.",
                b.id,
                tokens(b.context_tokens),
                largest_name,
                tokens(largest),
            ));
        }
        if b.context_tokens > largest {
            largest = b.context_tokens;
            largest_name.clone_from(&b.id);
        }
    }
    warnings
}

/// One line per row a conversation could never be moved onto. Only rows *after* the first: a pty
/// backend at the head is a perfectly good way to start a chat — it just cannot be fallen back to,
/// or fallen away from.
fn unreplayable_warnings(rows: &[AgentBackendRowDto], registry: &[LlmBackendDto]) -> Vec<String> {
    rows.iter()
        .skip(1)
        .filter_map(|row| registry.iter().find(|b| b.id == row.backend))
        .filter(|b| !b.replayable)
        .map(|b| {
            format!(
                "{} keeps no transcript, so a conversation cannot be moved onto it. Listed below \
                 the first row it will never be reached; it can only be where a chat starts.",
                b.id
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{
        AgentBackendRowDto, LlmBackendDto, dials_text, move_row, shrink_warnings, unused,
        unreplayable_warnings,
    };

    fn backend(id: &str, context: u64) -> LlmBackendDto {
        LlmBackendDto {
            id: id.to_string(),
            context_tokens: context,
            replayable: true,
            ..LlmBackendDto::default()
        }
    }

    fn rows(ids: &[&str]) -> Vec<AgentBackendRowDto> {
        ids.iter()
            .map(|id| AgentBackendRowDto {
                backend: (*id).to_string(),
                overrides: std::collections::BTreeMap::new(),
            })
            .collect()
    }

    #[test]
    fn moving_a_row_down_keeps_everything_else_in_order() {
        let mut list = rows(&["a", "b", "c"]);
        move_row(&mut list, 0, 2);
        let ids: Vec<&str> = list.iter().map(|r| r.backend.as_str()).collect();
        assert_eq!(ids, ["b", "c", "a"]);
    }

    #[test]
    fn moving_a_row_up_keeps_everything_else_in_order() {
        let mut list = rows(&["a", "b", "c"]);
        move_row(&mut list, 2, 0);
        let ids: Vec<&str> = list.iter().map(|r| r.backend.as_str()).collect();
        assert_eq!(ids, ["c", "a", "b"]);
    }

    /// A drop outside the list, or onto the row it started from, is not an edit.
    #[test]
    fn a_move_that_lands_nowhere_changes_nothing() {
        let mut list = rows(&["a", "b"]);
        move_row(&mut list, 1, 1);
        move_row(&mut list, 0, 9);
        move_row(&mut list, 9, 0);
        let ids: Vec<&str> = list.iter().map(|r| r.backend.as_str()).collect();
        assert_eq!(ids, ["a", "b"]);
    }

    /// The picker is the whole of "each backend at most once" — what it does not offer cannot be
    /// added, so a listed backend must not appear in it.
    #[test]
    fn the_picker_never_offers_a_backend_already_listed() {
        let registry = vec![backend("a", 0), backend("b", 0), backend("c", 0)];
        let free = unused(&registry, &rows(&["b"]));
        let ids: Vec<&str> = free.iter().map(|b| b.id.as_str()).collect();
        assert_eq!(ids, ["a", "c"]);
    }

    #[test]
    fn a_list_that_steps_down_in_context_is_warned_about() {
        let registry = vec![backend("big", 200_000), backend("small", 32_000)];
        assert_eq!(shrink_warnings(&rows(&["big", "small"]), &registry).len(), 1);
        assert!(shrink_warnings(&rows(&["small", "big"]), &registry).is_empty());
    }

    /// Unknown is not evidence of small, so a backend that declares no window warns about nothing —
    /// neither for itself nor for the row after it.
    #[test]
    fn an_unstated_context_window_warns_about_nothing() {
        let registry = vec![backend("big", 200_000), backend("quiet", 0)];
        assert!(shrink_warnings(&rows(&["big", "quiet"]), &registry).is_empty());
        assert!(shrink_warnings(&rows(&["quiet", "big"]), &registry).is_empty());
    }

    /// A pty backend can be where a chat starts; it cannot be where one lands.
    #[test]
    fn a_transcriptless_backend_is_only_warned_about_below_the_first_row() {
        let mut pty = backend("pty", 0);
        pty.replayable = false;
        let registry = vec![backend("api", 0), pty];
        assert!(unreplayable_warnings(&rows(&["pty", "api"]), &registry).is_empty());
        assert_eq!(unreplayable_warnings(&rows(&["api", "pty"]), &registry).len(), 1);
    }

    #[test]
    fn the_model_override_is_not_shown_among_the_dials() {
        let mut overrides = std::collections::BTreeMap::new();
        overrides.insert("model".to_string(), "opus".into());
        overrides.insert("effort".to_string(), "high".into());
        let text = dials_text(&overrides);
        assert!(text.contains("effort"), "{text}");
        assert!(!text.contains("model"), "{text}");
    }

    /// A row with nothing changed shows an empty box, not `{}` — the difference between "no dials"
    /// and "an empty set of dials" is the difference between a form you can read and one you cannot.
    #[test]
    fn a_row_with_no_dials_shows_an_empty_box() {
        assert_eq!(dials_text(&std::collections::BTreeMap::new()), "");
    }
}
