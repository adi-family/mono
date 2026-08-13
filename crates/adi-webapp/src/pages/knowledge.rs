//! The Knowledge page: scoped collections of text notes, searched by what they *mean*.
//!
//! A **knowledge** is one note of any length; it lives in a **base** at one of three isolation
//! levels — global, a project's, or an agent's own memory. Every note is embedded, so the search
//! box answers "how do I bring the panel back up" with the note titled "Restarting the control
//! panel" even though they share no words. See `docs/knowledge.md`.
//!
//! Search comes first because it is what the page is *for*. The bases and their notes sit under
//! it, in that order: you look something up far more often than you file something.
//!
//! The same body serves two places, differing only by a [`Scope`]: the global page, which shows
//! every base, and a project's Knowledge panel, which shows that project's and searches only
//! those. One body rather than two means the project panel cannot quietly drift from the page it
//! is a narrowing of.
//!
//! Two things the page states rather than hides. **Meaning vs words** is a real switch — the word
//! search needs no model and answers instantly, the meaning search may spend seconds loading one
//! on the very first query, and a user who is waiting deserves to know which they asked for. And
//! a note whose vectors are out of date is marked, because it is findable by word and not by
//! meaning until something re-embeds it.

use adi_ui::{EmptyRow, Row as TableRow, Table};
use adi_webapp_api::types::{
    KnowledgeBaseDto, KnowledgeHitDto, KnowledgeNoteDto, NewKnowledgeBase, NewKnowledgeNote,
};
use leptos::prelude::*;
use wasm_bindgen_futures::spawn_local;

use crate::fetch;
use crate::state::{KnowledgeConsole, State};
use crate::ui::{Key, TableState, TextField, confirm, row_actions, sort_rows};

/// The bases table on the global page. The trailing blank column holds the row actions.
pub(crate) const BASE_COLS: &[&str] = &["Base", "Level", "Provider", "Notes", "Embedded", ""];

/// The same inside a project, which drops Level — every row there is that project's, so the
/// column would say the same thing all the way down.
pub(crate) const PROJECT_BASE_COLS: &[&str] = &["Base", "Provider", "Notes", "Embedded", ""];

/// The open base's notes.
pub(crate) const NOTE_COLS: &[&str] = &["Title", "Tags", "Note", ""];

/// How much of a note's body a list row shows before it becomes the reader's job.
const PREVIEW: usize = 120;

/// Where this rendering sits: which project it is fixed to (if any) and which table states it
/// draws with, so the global page and a project's panel keep their own column layouts.
#[derive(Clone, Copy)]
struct Scope {
    /// The open project, or `None` on the global page. Held as the signal rather than a string
    /// so the panel follows a navigation from project A to project B, which keeps the route.
    project: Option<RwSignal<String>>,
    bases: TableState,
    notes: TableState,
}

impl Scope {
    /// The project id this rendering is fixed to, or `None` on the global page.
    fn project(self) -> Option<String> {
        self.project
            .map(|p| p.get())
            .filter(|p| !p.trim().is_empty())
    }

    /// The bases this rendering may show: every one, or only the open project's.
    fn bases(self, kb: KnowledgeConsole) -> Vec<KnowledgeBaseDto> {
        let all = kb.state.get().map(|s| s.bases).unwrap_or_default();
        match self.project() {
            None => all,
            Some(id) => all
                .into_iter()
                .filter(|b| b.level == "project" && b.owner.as_deref() == Some(id.as_str()))
                .collect(),
        }
    }
}

/// The global Knowledge page: search, then every base, then the open base's notes.
pub(crate) fn knowledge_view(state: State, kb: KnowledgeConsole) -> AnyView {
    body(
        state,
        kb,
        Scope {
            project: None,
            bases: state.tables.knowledge_bases,
            notes: state.tables.knowledge_notes,
        },
    )
}

/// A project's Knowledge panel: the same page narrowed to `project:<id>/…`.
///
/// Narrowed, not filtered after the fact: the search here is put to this project's bases alone,
/// so a question asked inside a project cannot come back answered from somebody else's.
pub(crate) fn knowledge_panel(state: State, kb: KnowledgeConsole) -> AnyView {
    body(
        state,
        kb,
        Scope {
            project: Some(state.current_project),
            bases: state.tables.project_knowledge_bases,
            notes: state.tables.project_knowledge_notes,
        },
    )
}

fn body(state: State, kb: KnowledgeConsole, scope: Scope) -> AnyView {
    // Load the bases when the page opens. This page's data is page-local, so nothing here rides
    // the shell's 4s poll — a base's counts are a status pass over its storage.
    Effect::new(move |loaded: Option<()>| {
        if loaded.is_none() {
            spawn_local(refresh_bases(kb));
        }
    });

    view! {
        {search_panel(kb, scope)}
        {results_panel(kb)}
        {reader_panel(kb)}
        {bases_panel(state, kb, scope)}
        {notes_panel(state, kb, scope)}
    }
    .into_any()
}

/// The search box: the question, what to rank by, and which base to look in.
fn search_panel(kb: KnowledgeConsole, scope: Scope) -> AnyView {
    let run = move || {
        if kb.query.get_untracked().trim().is_empty() {
            return;
        }
        spawn_local(search(kb, scope));
    };
    view! {
        <section class="adi-panel">
            <div class="adi-panel__head">
                <h2 class="adi-panel__title">"Search"</h2>
                {move || scope.project().map(|p| view! {
                    <span class="adi-updated">{format!("in {p}'s knowledge")}</span>
                })}
                <span class="adi-spacer"></span>
                // Which model is answering, once one has been loaded. Before that it says so
                // rather than naming a model that isn't in memory yet.
                <span class="adi-chip adi-mono" title="the embedding model, once loaded">
                    {move || kb.state.get().and_then(|s| s.model)
                        .unwrap_or_else(|| "model not loaded".to_string())}
                </span>
            </div>
            <div class="adi-form">
                <div style="display:flex; gap:var(--space-2); align-items:flex-end; flex-wrap:wrap">
                    // The query box is the page's primary control, so it takes the room:
                    // `flex:1` with a floor wide enough for a real question, and an input that
                    // fills its field rather than sitting at the browser's default size.
                    <div class="adi-field" style="flex:1 1 26rem; min-width:16rem">
                        <label class="adi-label" for="kb-query">"Question"</label>
                        <input id="kb-query" class="adi-input" type="search" style="width:100%"
                            placeholder="how do I bring the control panel back up"
                            prop:value=move || kb.query.get()
                            on:input=move |ev| kb.query.set(event_target_value(&ev))
                            on:keydown=move |ev| if ev.key() == "Enter" { run(); } />
                    </div>
                    <div class="adi-field">
                        <label class="adi-label" for="kb-scope">"In"</label>
                        <select id="kb-scope" class="adi-input adi-mono"
                            on:change=move |ev| kb.scope.set(event_target_value(&ev))>
                            <option value="" selected=move || kb.scope.get().is_empty()>
                                {move || if scope.project().is_some() {
                                    "this project's bases"
                                } else {
                                    "every base"
                                }}
                            </option>
                            {move || scope.bases(kb).into_iter().map(|b| {
                                let (value, shown) = (b.id.clone(), b.id.clone());
                                let selected = kb.scope.get() == b.id;
                                view! { <option value=value selected=selected>{shown}</option> }
                            }).collect_view()}
                        </select>
                    </div>
                    <button class="adi-btn adi-btn--primary" disabled=move || kb.busy.get()
                        on:click=move |_| run()>
                        {move || if kb.busy.get() { "Searching…" } else { "Search" }}
                    </button>
                </div>
                {rank_toggle(kb)}
                {error_view(kb)}
            </div>
        </section>
    }
    .into_any()
}

/// The one switch on this page that changes the answer rather than the view.
fn rank_toggle(kb: KnowledgeConsole) -> AnyView {
    view! {
        <div style="display:flex; gap:var(--space-2); align-items:center">
            <button type="button"
                class=move || if kb.words.get() { "adi-btn" } else { "adi-btn adi-btn--primary" }
                on:click=move |_| kb.words.set(false)>"By meaning"</button>
            <button type="button"
                class=move || if kb.words.get() { "adi-btn adi-btn--primary" } else { "adi-btn" }
                on:click=move |_| kb.words.set(true)>"By words"</button>
            <span class="adi-hint">
                {move || if kb.words.get() {
                    "full text — instant, and finds only the words you typed"
                } else {
                    "embeddings — finds what you meant; the first search loads the model"
                }}
            </span>
        </div>
    }
    .into_any()
}

/// The last search's answer.
fn results_panel(kb: KnowledgeConsole) -> AnyView {
    (move || {
        let results = kb.results.get()?;
        let heading = format!(
            "{} result(s) for “{}” — {} across {} base(s)",
            results.hits.len(),
            results.query,
            if results.semantic { "by meaning" } else { "by words" },
            results.bases.len(),
        );
        Some(view! {
            <section class="adi-panel">
                <div class="adi-panel__head">
                    <h2 class="adi-panel__title">"Results"</h2>
                    <span class="adi-updated">{heading}</span>
                </div>
                {if results.hits.is_empty() {
                    view! { <p class="adi-hint" style="padding:var(--space-3)">
                        "Nothing matched. A base with no embedded notes answers only to “By words” —
                         check the Embedded column below."
                    </p> }.into_any()
                } else {
                    results.hits.iter().map(|hit| hit_view(kb, hit)).collect_view().into_any()
                }}
            </section>
        })
    })
    .into_any()
}

/// One hit: how well it matched, where it came from, and enough of it to recognise.
fn hit_view(kb: KnowledgeConsole, hit: &KnowledgeHitDto) -> AnyView {
    let note = hit.note.clone();
    let (base, id) = (note.base.clone(), note.id.clone());
    let score = format!("{:.3}", hit.score);
    view! {
        <button type="button" class="adi-row-btn" style="width:100%; text-align:left"
            on:click=move |_| {
                let (b, i) = (base.clone(), id.clone());
                spawn_local(open_note(kb, b, i));
            }>
            <div style="display:flex; gap:var(--space-2); align-items:baseline; padding:var(--space-2) var(--space-3)">
                <span class="adi-chip adi-mono" title="similarity">{score}</span>
                <strong>{note.title.clone()}</strong>
                <span class="adi-chip adi-mono">{note.base.clone()}</span>
                {(!note.embedded).then(|| view! {
                    <span class="adi-chip" title="not embedded — found by words only">"stale"</span>
                })}
                <span class="adi-hint">{preview(&note.body)}</span>
            </div>
        </button>
    }
    .into_any()
}

/// The open note, in full. A list shows a line of a note; this shows the note.
fn reader_panel(kb: KnowledgeConsole) -> AnyView {
    (move || {
        let note = kb.open_note.get()?;
        let (base, id) = (note.base.clone(), note.id.clone());
        Some(view! {
            <section class="adi-panel">
                <div class="adi-panel__head">
                    <h2 class="adi-panel__title">{note.title.clone()}</h2>
                    <span class="adi-chip adi-mono">{note.base.clone()}</span>
                    <span class="adi-chip adi-mono">{note.id.clone()}</span>
                    <span class="adi-spacer"></span>
                    <span class="adi-hint">{embedding_label(&note)}</span>
                    <button class="adi-btn adi-btn--icon-sm" title="Close"
                        on:click=move |_| kb.open_note.set(None)>"\u{00d7}"</button>
                </div>
                <div style="padding:var(--space-3)">
                    {(!note.tags.is_empty()).then(|| view! {
                        <p>{note.tags.iter().map(|t| view! {
                            <span class="adi-chip">{t.clone()}</span>
                        }).collect_view()}</p>
                    })}
                    {note.source.clone().map(|s| view! {
                        <p class="adi-hint">"source: "{s}</p>
                    })}
                    // `white-space: pre-wrap` so a note keeps the shape it was written in —
                    // a runbook is mostly commands and indentation.
                    <pre style="white-space:pre-wrap; margin:0">{note.body.clone()}</pre>
                </div>
                <div class="adi-panel__head">
                    <button class="adi-btn adi-btn--danger" on:click=move |_| {
                        if confirm(&format!("Delete “{id}” from {base}?")) {
                            let (b, i) = (base.clone(), id.clone());
                            spawn_local(remove_note(kb, b, i));
                        }
                    }>"Delete note"</button>
                </div>
            </section>
        })
    })
    .into_any()
}

/// The bases in view, what holds them, and how much of each is searchable by meaning.
fn bases_panel(state: State, kb: KnowledgeConsole, scope: Scope) -> AnyView {
    view! {
        <section class="adi-panel">
            <div class="adi-panel__head">
                <h2 class="adi-panel__title">"Bases"</h2>
                <span class="adi-chip adi-mono">{move || scope.bases(kb).len()}</span>
                <span class="adi-spacer"></span>
                <button class="adi-btn adi-btn--icon-sm" title="Reload"
                    on:click=move |_| spawn_local(refresh_bases(kb))>"\u{21bb}"</button>
            </div>
            <Table state=scope.bases>{move || base_rows(state, kb, scope)}</Table>
            <div class="adi-panel__head">
                <h2 class="adi-panel__title">"New base"</h2>
            </div>
            {new_base_form(kb, scope)}
        </section>
    }
    .into_any()
}

fn base_rows(state: State, kb: KnowledgeConsole, scope: Scope) -> AnyView {
    let table = scope.bases;
    if kb.state.get().is_none() {
        return view! { <EmptyRow state=table>"Loading…"</EmptyRow> }.into_any();
    }
    let mut rows = scope.bases(kb);
    if rows.is_empty() {
        return view! {
            <EmptyRow state=table>"No knowledge bases here yet — make one below."</EmptyRow>
        }
        .into_any();
    }
    sort_rows(
        &mut rows,
        table.sort.get(),
        |b, col| match col {
            "Level" => Key::text(&b.level),
            "Provider" => Key::text(&b.provider),
            "Notes" => Key::count(b.notes),
            // By what is *missing*, not by what is done: the reason to sort this column is to
            // find the bases that still need embedding.
            "Embedded" => Key::count(b.stale),
            _ => Key::text(&b.id),
        },
        |b| Key::text(&b.id),
    );
    rows.into_iter()
        .map(|b| {
            let actions = base_actions(state, kb, &b);
            let base = b.clone();
            view! {
                <TableRow state=table cell=move |col| base_cell(col, &base, kb) actions=actions/>
            }
            .into_any()
        })
        .collect::<Vec<_>>()
        .into_any()
}

/// One base's cell under `col` — matched on the header text, which is what lets a user hide and
/// reorder columns without this knowing about it, and is also how the project panel's narrower
/// column set works from the same code.
fn base_cell(col: &str, base: &KnowledgeBaseDto, kb: KnowledgeConsole) -> AnyView {
    match col {
        "Level" => view! {
            <span class="adi-chip">{base.level.clone()}</span>
            {base.memory.then(|| view! {
                <span class="adi-chip" title="this agent's own memory">"memory"</span>
            })}
        }
        .into_any(),
        "Provider" => view! { <span class="adi-mono">{base.provider.clone()}</span> }.into_any(),
        "Notes" => view! { <span class="adi-mono">{base.notes}</span> }.into_any(),
        "Embedded" => embedded_cell(base),
        _ => {
            let id = base.id.clone();
            let shown = base.id.clone();
            view! {
                <button type="button" class="adi-link adi-mono" on:click=move |_| {
                    let b = id.clone();
                    spawn_local(open_base(kb, b));
                }>{shown}</button>
                {base.description.clone().map(|d| view! { <div class="adi-hint">{d}</div> })}
            }
            .into_any()
        }
    }
}

/// The count worth watching: anything stale is findable by word and not by meaning until a
/// re-embed catches it up, so the cell says so rather than showing a bare fraction.
fn embedded_cell(base: &KnowledgeBaseDto) -> AnyView {
    if let Some(err) = base.error.clone() {
        return view! { <span class="adi-hint" title=err>"unavailable"</span> }.into_any();
    }
    let text = format!("{} / {}", base.embedded, base.notes);
    if base.stale > 0 {
        let stale = format!("{} stale", base.stale);
        view! {
            <span class="adi-mono">{text}</span>
            <span class="adi-chip" title="these answer only to “By words”">{stale}</span>
        }
        .into_any()
    } else {
        view! { <span class="adi-mono">{text}</span> }.into_any()
    }
}

/// A base's row controls: re-embed what needs it, or delete the whole thing.
fn base_actions(state: State, kb: KnowledgeConsole, base: &KnowledgeBaseDto) -> AnyView {
    let (reembed_id, del_id) = (base.id.clone(), base.id.clone());
    let inline = view! {
        <button class="adi-btn adi-btn--sm" title="Embed everything in this base that needs it"
            disabled=move || kb.busy.get()
            on:click=move |_| { let b = reembed_id.clone(); spawn_local(reembed(kb, b)); }>
            "Re-embed"
        </button>
    };
    let delete = view! {
        <button class="adi-btn adi-btn--sm adi-btn--danger" on:click=move |_| {
            let b = del_id.clone();
            if confirm(&format!("Delete {b} and every note in it?")) {
                spawn_local(remove_base(kb, b));
            }
        }>"Delete base"</button>
    }
    .into_any();
    row_actions(state, format!("kb-base:{}", base.id), inline, vec![delete])
}

/// The create form. Inside a project it asks for a *name* and builds the id, so the scope is not
/// something the user can typo their way out of.
fn new_base_form(kb: KnowledgeConsole, scope: Scope) -> AnyView {
    view! {
        <div class="adi-form">
            {move || match scope.project() {
                Some(id) => view! {
                    <TextField id="kb-new-base" label="Name" mono=true placeholder="notes"
                        hint="filed under this project" value=kb.new_base />
                    <p class="adi-hint adi-mono">{format!("project:{id}/…")}</p>
                }
                .into_any(),
                None => view! {
                    <TextField id="kb-new-base" label="Base" mono=true
                        placeholder="global/runbooks"
                        hint="global/<name>, project:<id>/<name>, or agent:<name>/<base>"
                        value=kb.new_base />
                }
                .into_any(),
            }}
            <div class="adi-field">
                <label class="adi-label" for="kb-provider">"Provider"</label>
                <select id="kb-provider" class="adi-input adi-mono"
                    on:change=move |ev| kb.new_provider.set(event_target_value(&ev))>
                    <option value="">"default (sqlite)"</option>
                    {move || kb.state.get().map(|s| s.providers).unwrap_or_default()
                        .into_iter().map(|p| {
                            let (value, shown) = (p.name.clone(), p.name.clone());
                            view! { <option value=value title=p.description>{shown}</option> }
                        }).collect_view()}
                </select>
            </div>
            <button class="adi-btn adi-btn--primary" disabled=move || kb.busy.get()
                on:click=move |_| spawn_local(create_base(kb, scope))>"Create base"</button>
        </div>
    }
    .into_any()
}

/// The open base's notes, and the form that adds one.
fn notes_panel(state: State, kb: KnowledgeConsole, scope: Scope) -> AnyView {
    (move || {
        let base = kb.open_base.get();
        if base.is_empty() {
            return None;
        }
        let title = base.clone();
        Some(view! {
            <section class="adi-panel">
                <div class="adi-panel__head">
                    <h2 class="adi-panel__title">"Notes"</h2>
                    <span class="adi-chip adi-mono">{title}</span>
                    <span class="adi-spacer"></span>
                    <button class="adi-btn adi-btn--icon-sm" title="Close"
                        on:click=move |_| { kb.open_base.set(String::new()); kb.notes.set(None); }>
                        "\u{00d7}"
                    </button>
                </div>
                <Table state=scope.notes>{move || note_rows(state, kb, scope)}</Table>
                <div class="adi-panel__head">
                    <h2 class="adi-panel__title">"New note"</h2>
                </div>
                {new_note_form(kb)}
            </section>
        })
    })
    .into_any()
}

fn note_rows(state: State, kb: KnowledgeConsole, scope: Scope) -> AnyView {
    let table = scope.notes;
    let Some(loaded) = kb.notes.get() else {
        return view! { <EmptyRow state=table>"Loading…"</EmptyRow> }.into_any();
    };
    let mut rows = loaded.notes;
    if rows.is_empty() {
        return view! { <EmptyRow state=table>"Nothing in this base yet."</EmptyRow> }.into_any();
    }
    sort_rows(
        &mut rows,
        table.sort.get(),
        |n, col| match col {
            "Tags" => Key::text(n.tags.join(",")),
            "Note" => Key::text(&n.body),
            _ => Key::text(&n.title),
        },
        |n| Key::text(&n.id),
    );
    rows.into_iter()
        .map(|n| {
            let actions = note_actions(state, kb, &n);
            let note = n.clone();
            view! {
                <TableRow state=table cell=move |col| note_cell(col, &note, kb) actions=actions/>
            }
            .into_any()
        })
        .collect::<Vec<_>>()
        .into_any()
}

fn note_cell(col: &str, note: &KnowledgeNoteDto, kb: KnowledgeConsole) -> AnyView {
    match col {
        "Tags" => note
            .tags
            .iter()
            .map(|t| view! { <span class="adi-chip">{t.clone()}</span> })
            .collect_view()
            .into_any(),
        "Note" => view! {
            <span class="adi-hint">{preview(&note.body)}</span>
            {(!note.embedded).then(|| view! {
                <span class="adi-chip" title="findable by words only until re-embedded">"stale"</span>
            })}
        }
        .into_any(),
        _ => {
            let (base, id) = (note.base.clone(), note.id.clone());
            let (title, shown_id) = (note.title.clone(), note.id.clone());
            view! {
                <button type="button" class="adi-link" on:click=move |_| {
                    let (b, i) = (base.clone(), id.clone());
                    spawn_local(open_note(kb, b, i));
                }>{title}</button>
                <div class="adi-hint adi-mono">{shown_id}</div>
            }
            .into_any()
        }
    }
}

fn note_actions(state: State, kb: KnowledgeConsole, note: &KnowledgeNoteDto) -> AnyView {
    let (base, id) = (note.base.clone(), note.id.clone());
    let delete = view! {
        <button class="adi-btn adi-btn--sm adi-btn--danger" on:click=move |_| {
            let (b, i) = (base.clone(), id.clone());
            if confirm(&format!("Delete “{i}”?")) {
                spawn_local(remove_note(kb, b, i));
            }
        }>"Delete"</button>
    }
    .into_any();
    row_actions(state, format!("kb-note:{}", note.id), (), vec![delete])
}

fn new_note_form(kb: KnowledgeConsole) -> AnyView {
    view! {
        <div class="adi-form">
            <TextField id="kb-title" label="Title" wide=true
                placeholder="Restarting the control panel" value=kb.title />
            <div class="adi-field" style="width:100%">
                <label class="adi-label" for="kb-body">"Note"</label>
                // Any length: the store chunks a long note rather than truncating it, so there is
                // no reason for this box to imply a limit.
                <textarea id="kb-body" class="adi-input adi-mono" rows="6" style="width:100%"
                    placeholder="launchctl kickstart -k gui/$(id -u)/family.adi.app.control-panel"
                    prop:value=move || kb.body.get()
                    on:input=move |ev| kb.body.set(event_target_value(&ev))></textarea>
            </div>
            <TextField id="kb-tags" label="Tags" mono=true placeholder="ops, deploy"
                hint="comma-separated" value=kb.tags />
            <button class="adi-btn adi-btn--primary" disabled=move || kb.busy.get()
                on:click=move |_| spawn_local(add_note(kb))>
                {move || if kb.busy.get() { "Embedding…" } else { "Add note" }}
            </button>
        </div>
    }
    .into_any()
}

fn error_view(kb: KnowledgeConsole) -> AnyView {
    (move || {
        kb.error
            .get()
            .map(|e| view! { <p class="adi-error" role="alert">{e}</p> })
    })
    .into_any()
}

// ------------------------------------------------------------------ actions

async fn refresh_bases(kb: KnowledgeConsole) {
    match fetch::knowledge().await {
        Ok(s) => {
            kb.state.set(Some(s));
            kb.error.set(None);
        }
        Err(e) => kb.error.set(Some(e)),
    }
}

/// Run the search over the bases this rendering covers.
///
/// An empty base list means "everything readable" to the server, which is right on the global
/// page and wrong inside a project — so a project panel names its bases explicitly rather than
/// letting the default widen the question.
async fn search(kb: KnowledgeConsole, scope: Scope) {
    let narrowed = kb.scope.get_untracked();
    let bases = if !narrowed.is_empty() {
        vec![narrowed]
    } else if scope.project().is_some() {
        let ids: Vec<String> = scope.bases(kb).into_iter().map(|b| b.id).collect();
        if ids.is_empty() {
            kb.error
                .set(Some("this project has no knowledge bases yet.".into()));
            return;
        }
        ids
    } else {
        Vec::new()
    };

    kb.busy.set(true);
    kb.error.set(None);
    match fetch::knowledge_search(kb.query.get_untracked(), bases, kb.words.get_untracked()).await {
        Ok(results) => {
            kb.results.set(Some(results));
            // A hit from an earlier query would otherwise sit under the new results claiming to
            // be one of them.
            kb.open_note.set(None);
        }
        Err(e) => kb.error.set(Some(e)),
    }
    kb.busy.set(false);
}

async fn open_base(kb: KnowledgeConsole, base: String) {
    kb.open_base.set(base.clone());
    kb.notes.set(None);
    match fetch::knowledge_notes(base).await {
        Ok(notes) => kb.notes.set(Some(notes)),
        Err(e) => kb.error.set(Some(e)),
    }
}

async fn open_note(kb: KnowledgeConsole, base: String, id: String) {
    match fetch::knowledge_note(base, id).await {
        Ok(note) => kb.open_note.set(Some(note)),
        Err(e) => kb.error.set(Some(e)),
    }
}

async fn add_note(kb: KnowledgeConsole) {
    let base = kb.open_base.get_untracked();
    if base.is_empty() {
        return;
    }
    kb.busy.set(true);
    kb.error.set(None);
    let body = NewKnowledgeNote {
        base: base.clone(),
        title: kb.title.get_untracked(),
        body: kb.body.get_untracked(),
        tags: split_tags(&kb.tags.get_untracked()),
        source: None,
    };
    match fetch::add_knowledge_note(body).await {
        Ok(saved) => {
            kb.title.set(String::new());
            kb.body.set(String::new());
            kb.tags.set(String::new());
            // A note that could not be embedded is still stored. Say so here rather than let the
            // user discover it as a search that finds nothing.
            if let Some(reason) = saved.embed_error {
                kb.error
                    .set(Some(format!("stored, but not embedded: {reason}")));
            }
            spawn_local(open_base(kb, base));
            spawn_local(refresh_bases(kb));
        }
        Err(e) => kb.error.set(Some(e)),
    }
    kb.busy.set(false);
}

async fn remove_note(kb: KnowledgeConsole, base: String, id: String) {
    match fetch::remove_knowledge_note(base, id).await {
        Ok(notes) => {
            kb.notes.set(Some(notes));
            kb.open_note.set(None);
            spawn_local(refresh_bases(kb));
        }
        Err(e) => kb.error.set(Some(e)),
    }
}

/// Create a base. Inside a project the typed name is filed under it, so the panel cannot produce
/// a base that isn't the open project's.
async fn create_base(kb: KnowledgeConsole, scope: Scope) {
    let typed = kb.new_base.get_untracked().trim().to_string();
    if typed.is_empty() {
        kb.error
            .set(Some("a base needs a name, e.g. runbooks".into()));
        return;
    }
    let base = match scope.project() {
        Some(id) => format!("project:{id}/{typed}"),
        None => typed,
    };
    kb.busy.set(true);
    kb.error.set(None);
    let provider = kb.new_provider.get_untracked();
    let body = NewKnowledgeBase {
        base,
        provider: (!provider.is_empty()).then_some(provider),
        description: None,
    };
    match fetch::create_knowledge_base(body).await {
        Ok(s) => {
            kb.state.set(Some(s));
            kb.new_base.set(String::new());
        }
        Err(e) => kb.error.set(Some(e)),
    }
    kb.busy.set(false);
}

async fn remove_base(kb: KnowledgeConsole, base: String) {
    match fetch::remove_knowledge_base(base.clone()).await {
        Ok(s) => {
            kb.state.set(Some(s));
            if kb.open_base.get_untracked() == base {
                kb.close();
            }
        }
        Err(e) => kb.error.set(Some(e)),
    }
}

async fn reembed(kb: KnowledgeConsole, base: String) {
    kb.busy.set(true);
    kb.error.set(None);
    match fetch::reembed_knowledge(base).await {
        Ok(report) => {
            let mut msg = format!(
                "Embedded {} of {} note(s) into {} chunk(s); {} already current.",
                report.embedded, report.scanned, report.chunks, report.unchanged
            );
            for failure in &report.failed {
                msg.push_str(&format!("\n{failure}"));
            }
            // Not an error, but the same place a user is already looking for the answer.
            kb.error.set(Some(msg));
            spawn_local(refresh_bases(kb));
        }
        Err(e) => kb.error.set(Some(e)),
    }
    kb.busy.set(false);
}

// ------------------------------------------------------------------ helpers

/// A note's opening words, flattened onto one line.
fn preview(body: &str) -> String {
    let flat = body.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= PREVIEW {
        return flat;
    }
    let cut: String = flat.chars().take(PREVIEW.saturating_sub(1)).collect();
    format!("{}\u{2026}", cut.trim_end())
}

/// What the reader says about a note's vectors.
fn embedding_label(note: &KnowledgeNoteDto) -> String {
    match (&note.model, note.embedded) {
        (Some(model), true) => format!("{} chunk(s) by {model}", note.chunks),
        _ => "not embedded — found by words only".to_string(),
    }
}

/// `a, b` → `["a", "b"]`.
fn split_tags(raw: &str) -> Vec<String> {
    raw.split(',')
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .map(ToString::to_string)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_preview_never_runs_past_its_width_and_is_one_line() {
        let body = format!("first line\nsecond line{}", " padding".repeat(60));
        let p = preview(&body);
        assert!(p.chars().count() <= PREVIEW);
        assert!(!p.contains('\n'), "a preview must fit one row");
        assert_eq!(preview("short note"), "short note");
    }

    #[test]
    fn tags_are_split_and_de_blanked() {
        assert_eq!(split_tags("ops, deploy ,, "), vec!["ops", "deploy"]);
        assert!(split_tags("   ").is_empty());
    }

    /// The label is the page's whole claim about whether a note is searchable by meaning, so it
    /// must never say "embedded" about one that isn't.
    #[test]
    fn the_embedding_label_tells_the_truth_either_way() {
        let mut note = KnowledgeNoteDto {
            id: "a".into(),
            base: "global/n".into(),
            title: "A".into(),
            body: String::new(),
            tags: Vec::new(),
            source: None,
            embedded: true,
            chunks: 3,
            model: Some("jina".into()),
            created_at: 0,
            updated_at: 0,
        };
        assert_eq!(embedding_label(&note), "3 chunk(s) by jina");

        note.embedded = false;
        assert!(embedding_label(&note).contains("not embedded"));

        // A model name with no current vectors is the case that must not read as embedded.
        note.model = Some("jina".into());
        assert!(embedding_label(&note).contains("not embedded"));
    }

    /// The project panel's whole job: a question asked inside a project must not be answered
    /// from another project's knowledge, nor from an agent's memory.
    #[test]
    fn a_project_scope_keeps_only_that_projects_bases() {
        let all = [
            base_dto("global/runbooks", "global", None),
            base_dto("project:acme/notes", "project", Some("acme")),
            base_dto("project:other/notes", "project", Some("other")),
            base_dto("agent:solver/memory", "agent", Some("solver")),
        ];
        let kept: Vec<&str> = all
            .iter()
            .filter(|b| b.level == "project" && b.owner.as_deref() == Some("acme"))
            .map(|b| b.id.as_str())
            .collect();
        assert_eq!(kept, vec!["project:acme/notes"]);
    }

    fn base_dto(id: &str, level: &str, owner: Option<&str>) -> KnowledgeBaseDto {
        KnowledgeBaseDto {
            id: id.into(),
            level: level.into(),
            owner: owner.map(ToString::to_string),
            name: "n".into(),
            provider: "sqlite".into(),
            description: None,
            memory: false,
            notes: 0,
            embedded: 0,
            stale: 0,
            error: None,
            created_at: 0,
            updated_at: 0,
        }
    }
}
