//! The Workflows page (ADI-MONO-141 §F): every discovered workflow file, each expandable into
//! its own starters/shapes/enable toggle/run-now box and its own runs list; a run expands
//! further into its saved-step timeline, with a rewind button on each step.
//!
//! Page-local, like [`super::embedding_backends_view`] beside it: nothing here rides the shell's
//! 4s poll or the live channel, so it is fetched once when the page opens and refreshed after
//! each mutation from that mutation's own reply.
//!
//! This page shows every workflow, global and project; a project's own Workflows section shows
//! only its own, filtered client-side from the same load — there is no per-project list endpoint.
//! [`project_rows_view`] and [`refresh`] are `pub(crate)` so that panel can reuse this page's row,
//! detail and run views instead of forking them.

use adi_ui::{CodeEditor, CodeFrame, CodeHeight, Lang};
use adi_webapp_api::types::{RunDto, RunWorkflow, RunsQuery, StepDto, WorkflowDto};
use leptos::prelude::*;
use wasm_bindgen_futures::spawn_local;

use crate::fetch;
use crate::pages::store_file::load_store_file;
use crate::routing::{Route, push_state, scroll_top, store_file_path};
use crate::state::{Flash, State, WorkflowsConsole};
use crate::ui::{apply_mutation, confirm, field_hint, flash_view};

/// The Workflows page.
pub(crate) fn workflows_view(state: State, route: RwSignal<Route>, console: WorkflowsConsole) -> AnyView {
    // Load once when the page opens.
    Effect::new(move |loaded: Option<()>| {
        if loaded.is_none() {
            spawn_local(refresh(console));
        }
    });
    // Fetch the expanded workflow's own runs whenever which one is expanded changes.
    Effect::new(move |prev: Option<String>| {
        let id = console.selected.get();
        if !id.is_empty() && prev.as_deref() != Some(id.as_str()) {
            spawn_local(refresh_runs(console, id.clone()));
        }
        id
    });
    // Fetch the expanded run's own timeline whenever which one is expanded changes.
    Effect::new(move |prev: Option<String>| {
        let run_key = console.selected_run.get();
        if !run_key.is_empty() && prev.as_deref() != Some(run_key.as_str()) {
            spawn_local(refresh_run_detail(console, run_key.clone()));
        }
        run_key
    });

    view! {
        <section class="adi-panel">
            <div class="adi-panel__head">
                <span class="adi-updated">{move || summary(console)}</span>
                <span class="adi-spacer"></span>
                <button class="adi-btn adi-btn--ghost" type="button"
                    on:click=move |_| spawn_local(refresh(console))>
                    "Reload"
                </button>
            </div>
            <div class="adi-panel__body">
                {move || rows_view(state, route, console)}
            </div>
            {move || error_view(console)}
            {flash_view(state.flash)}
            <p class="adi-hint">
                "A workflow file nobody has told this store about yet is disabled by default — "
                <code>"Run now"</code>" still works on one regardless, which is how a draft is
                 tried before it is trusted with its own schedule/event triggers."
            </p>
        </section>
    }
    .into_any()
}

fn summary(console: WorkflowsConsole) -> String {
    console.workflows.get().map_or_else(String::new, |s| match s.workflows.len() {
        0 => "No workflows".to_string(),
        1 => "1 workflow".to_string(),
        n => format!("{n} workflows"),
    })
}

fn error_view(console: WorkflowsConsole) -> AnyView {
    match console.error.get() {
        Some(e) => view! { <p class="adi-error" role="alert">{e}</p> }.into_any(),
        None => ().into_any(),
    }
}

fn rows_view(state: State, route: RwSignal<Route>, console: WorkflowsConsole) -> AnyView {
    let Some(st) = console.workflows.get() else {
        return view! { <p class="adi-empty">"Loading…"</p> }.into_any();
    };
    if st.workflows.is_empty() {
        return view! {
            <p class="adi-empty">
                "No workflow files found — add one under "<code>"workflows/*.ts"</code>
                " (or "<code>"projects/&lt;id&gt;/workflows/*.ts"</code>")."
            </p>
        }
        .into_any();
    }
    st.workflows
        .into_iter()
        .map(|w| workflow_block(state, route, console, w))
        .collect::<Vec<_>>()
        .into_any()
}

/// The same rows, narrowed to one project's own workflow files (`project == Some(id)`) — a
/// project's Workflows section, reusing this page's row/detail/run views rather than forking
/// them. There is no per-project list endpoint, so the filter runs client-side over the one
/// shared load [`refresh`] already did.
pub(crate) fn project_rows_view(
    state: State,
    route: RwSignal<Route>,
    console: WorkflowsConsole,
    project: &str,
) -> AnyView {
    let Some(st) = console.workflows.get() else {
        return view! { <p class="adi-empty">"Loading…"</p> }.into_any();
    };
    let rows: Vec<WorkflowDto> = st
        .workflows
        .into_iter()
        .filter(|w| w.project.as_deref() == Some(project))
        .collect();
    if rows.is_empty() {
        return view! {
            <p class="adi-empty">
                "No workflow files found for this project — add one under "
                <code>"projects/"{project.to_string()}"/workflows/*.ts"</code>"."
            </p>
        }
        .into_any();
    }
    rows.into_iter()
        .map(|w| workflow_block(state, route, console, w))
        .collect::<Vec<_>>()
        .into_any()
}

/// One workflow's header row — its id, project, enabled state, and the enable/disable toggle —
/// expanding into [`workflow_detail`] when clicked.
fn workflow_block(state: State, route: RwSignal<Route>, console: WorkflowsConsole, w: WorkflowDto) -> AnyView {
    let id = w.id.clone();
    let caret_id = id.clone();
    let toggle_id = id.clone();
    let enable_id = id.clone();
    let detail_id = id.clone();
    let enabled = w.enabled;
    let source_path = w.path.clone();

    view! {
        <div class="adi-field adi-field--full">
            <div class="adi-copyrow">
                <button class="adi-btn adi-btn--quiet" type="button"
                    on:click=move |_| toggle_workflow(console, &toggle_id, source_path.clone())>
                    <span class="adi-mono">
                        {move || if console.selected.get() == caret_id { "\u{25be}" } else { "\u{25b8}" }}
                        " "{w.id.clone()}
                    </span>
                </button>
                {match w.project.clone() {
                    Some(p) => view! { <span class="adi-chip">{p}</span> }.into_any(),
                    None => view! { <span class="adi-muted">"Global"</span> }.into_any(),
                }}
                <span class="adi-tstatus" data-status=if enabled { "ready" } else { "archived" }>
                    {if enabled { "Enabled" } else { "Disabled" }}
                </span>
                <span class="adi-spacer"></span>
                <button class="adi-btn adi-btn--ghost" type="button"
                    on:click=move |_| toggle_enabled(state, console, enable_id.clone(), !enabled)>
                    {if enabled { "Disable" } else { "Enable" }}
                </button>
            </div>
            {move || (console.selected.get() == detail_id).then(|| workflow_detail(state, route, console, &w))}
        </div>
    }
    .into_any()
}

/// Expand/collapse `id` (the header row's click), starting the source read the moment it opens —
/// not from a page-level `Effect` keyed on [`WorkflowsConsole::selected`], because that console is
/// shared between the global page and a project's own Workflows section (`workflows_panel.rs`),
/// and only the former currently sets up effects for anything past the initial list load. Doing
/// the fetch right here works on both without needing that effect at all.
fn toggle_workflow(console: WorkflowsConsole, id: &str, path: String) {
    let opening = console.selected.get_untracked() != id;
    console.toggle_selected(id);
    if opening && !path.is_empty() {
        spawn_local(refresh_source(console, path));
    }
}

/// A workflow's expanded detail: its starters, its declared input/output shapes, a describe
/// error if it has one, its own TypeScript source, the run-now box, and its own runs list.
fn workflow_detail(state: State, route: RwSignal<Route>, console: WorkflowsConsole, w: &WorkflowDto) -> AnyView {
    let run_id = w.id.clone();
    view! {
        <div class="adi-field adi-field--full">
            {(!w.triggers.is_empty()).then(|| view! {
                <p class="adi-cell__sub">
                    {w.triggers.join(" · ")}
                </p>
            })}
            {w.describe_error.clone().map(|e| view! {
                <p class="adi-error">"Couldn't read this file's `on` export: "{e}</p>
            })}
            {schema_view("Input", w.input_schema.clone())}
            {schema_view("Output", w.output_schema.clone())}

            {source_view(state, route, console, w.path.clone())}

            <div class="adi-copyrow">
                <textarea class="adi-textarea adi-mono" rows="2" placeholder="{} (the workflow's own input, as JSON)"
                    prop:value=move || console.run_input.get()
                    on:input=move |ev| console.run_input.set(event_target_value(&ev))></textarea>
                <button class="adi-btn adi-btn--primary" type="button"
                    prop:disabled=move || console.busy.get()
                    on:click=move |_| run_now(state, console, &run_id)>
                    "Run now"
                </button>
            </div>
            {field_hint("starts `wf@manual:<id>` by hand — works even while disabled")}

            {move || runs_view(state, console)}
        </div>
    }
    .into_any()
}

fn schema_view(label: &'static str, schema: Option<serde_json::Value>) -> AnyView {
    match schema {
        Some(schema) => view! {
            <details class="adi-field">
                <summary class="adi-field__label">{label}" shape"</summary>
                <pre class="adi-mono adi-cell__sub">
                    {serde_json::to_string_pretty(&schema).unwrap_or_default()}
                </pre>
            </details>
        }
        .into_any(),
        None => ().into_any(),
    }
}

/// The workflow file's own TypeScript source, read-only and syntax-highlighted — shown
/// unconditionally, even when [`WorkflowDto::describe_error`] is set: that is exactly when
/// reading the file by eye matters most. The content comes from the store browser's own
/// `/api/fs/read` (what [`crate::pages::store_file`] itself reads through), fetched once when the
/// row opens ([`toggle_workflow`]) rather than on every render.
fn source_view(state: State, route: RwSignal<Route>, console: WorkflowsConsole, path: String) -> AnyView {
    if path.is_empty() {
        return ().into_any();
    }
    let edit_path = path.clone();
    let actions = move || {
        let edit_path = edit_path.clone();
        view! {
            <button class="adi-btn adi-btn--sm" type="button"
                on:click=move |_| open_in_editor(state, route, edit_path.clone())>
                "Open in editor"
            </button>
        }
        .into_any()
    };
    view! {
        <CodeFrame title=path.clone() actions=actions height=CodeHeight::Form class="island">
            {move || match (console.source.get(), console.source_error.get()) {
                (_, Some(e)) => view! {
                    <p class="adi-error">"Couldn't read this file: "{e}</p>
                }
                .into_any(),
                (None, None) => view! { <p class="adi-empty">"Loading source…"</p> }.into_any(),
                (Some(_), None) => view! {
                    <CodeEditor value=console.source_buffer lang=Lang::Ts readonly=true
                        height=CodeHeight::Form id="workflow-source"/>
                }
                .into_any(),
            }}
        </CodeFrame>
    }
    .into_any()
}

/// Navigate to the store-file editor (`/files/<path>`, [`Route::StoreFile`]) open on this
/// workflow file — the "Open in editor" action, the same navigation [`crate::store_browser`]'s own
/// tree uses to open a file. No second editor: this reuses that one rather than forking it.
fn open_in_editor(state: State, route: RwSignal<Route>, path: String) {
    push_state(&store_file_path(&path));
    route.set(Route::StoreFile);
    scroll_top();
    load_store_file(state, path);
}

/// A workflow's own runs list, under its expanded detail — each row expands into
/// [`run_timeline_view`].
fn runs_view(state: State, console: WorkflowsConsole) -> AnyView {
    let Some(st) = console.runs.get() else {
        return view! { <p class="adi-empty">"Loading runs…"</p> }.into_any();
    };
    if st.runs.is_empty() {
        return view! { <p class="adi-empty">"No runs yet."</p> }.into_any();
    }
    st.runs
        .iter()
        .map(|run| run_row(state, console, run))
        .collect::<Vec<_>>()
        .into_any()
}

fn run_row(state: State, console: WorkflowsConsole, run: &RunDto) -> AnyView {
    let run_key = run.run_key.clone();
    let caret_key = run_key.clone();
    let toggle_key = run_key.clone();
    let timeline_key = run_key.clone();
    view! {
        <div class="adi-field">
            <div class="adi-copyrow">
                <button class="adi-btn adi-btn--quiet" type="button"
                    on:click=move |_| console.toggle_run(&toggle_key)>
                    <span class="adi-mono">
                        {move || if console.selected_run.get() == caret_key { "\u{25be}" } else { "\u{25b8}" }}
                        " "{run.run_key.clone()}
                    </span>
                </button>
                <span class="adi-tstatus" data-status=status_dot(&run.status)>{run.status.clone()}</span>
                <span class="adi-spacer"></span>
                <span class="adi-cell__sub">{crate::ui::fmt_date(run.created_at)}</span>
            </div>
            {move || (console.selected_run.get() == timeline_key).then(|| run_timeline_view(state, console, &run_key))}
        </div>
    }
    .into_any()
}

/// The status badge's dot colour — a loose mapping, not load-bearing: "done" reads ready,
/// "failed"/"stuck" read archived (the design system's two existing status colours), everything
/// still moving (running, waiting on …) reads ready too.
fn status_dot(status: &str) -> &'static str {
    match status {
        "failed" | "stuck" => "archived",
        _ => "ready",
    }
}

/// A run's expanded timeline: every journaled step, in resolution order, each with a "rewind to
/// here" button that keeps everything before it and redoes it (and everything after) for real.
fn run_timeline_view(state: State, console: WorkflowsConsole, run_key: &str) -> AnyView {
    let Some(detail) = console.run_detail.get() else {
        return view! { <p class="adi-empty">"Loading…"</p> }.into_any();
    };
    if detail.run_key != run_key {
        // A stale detail from the previously expanded run, about to be replaced by the effect.
        return view! { <p class="adi-empty">"Loading…"</p> }.into_any();
    }
    if detail.steps.is_empty() {
        return view! { <p class="adi-empty">"No steps journaled yet."</p> }.into_any();
    }
    detail
        .steps
        .iter()
        .enumerate()
        .map(|(i, step)| step_row(state, console, run_key.to_string(), i, step))
        .collect::<Vec<_>>()
        .into_any()
}

fn step_row(state: State, console: WorkflowsConsole, run_key: String, index: usize, step: &StepDto) -> AnyView {
    let value = step.value.as_ref().map(|v| serde_json::to_string_pretty(v).unwrap_or_default());
    view! {
        <div class="adi-field">
            <div class="adi-copyrow">
                <span class="adi-mono">{step.step_key.clone()}</span>
                <span class="adi-chip">{step.kind.clone()}</span>
                <span class="adi-tstatus" data-status=if step.outcome == "ok" { "ready" } else { "archived" }>
                    {step.outcome.clone()}
                </span>
                <span class="adi-spacer"></span>
                <button class="adi-btn adi-btn--ghost" type="button"
                    on:click=move |_| rewind_to_here(state, console, run_key.clone(), index)>
                    "Rewind to here"
                </button>
            </div>
            {step.task_id.clone().map(|id| view! {
                <p class="adi-cell__sub">"task "<span class="adi-mono">{id}</span></p>
            })}
            {value.map(|v| view! { <pre class="adi-mono adi-cell__sub">{v}</pre> })}
        </div>
    }
    .into_any()
}

pub(crate) async fn refresh(console: WorkflowsConsole) {
    match fetch::workflows().await {
        Ok(s) => {
            console.workflows.set(Some(s));
            console.error.set(None);
        }
        Err(e) => console.error.set(Some(e)),
    }
}

/// Read a workflow file's own source through the store browser's `/api/fs/read` — called once,
/// when its row opens ([`toggle_workflow`]).
async fn refresh_source(console: WorkflowsConsole, path: String) {
    console.source.set(None);
    console.source_error.set(None);
    match fetch::fs_read(&path).await {
        Ok(c) => {
            console.source_buffer.set(c.content.clone());
            console.source.set(Some(c.content));
        }
        Err(e) => console.source_error.set(Some(e)),
    }
}

async fn refresh_runs(console: WorkflowsConsole, workflow: String) {
    console.runs.set(None);
    match fetch::workflow_runs(RunsQuery {
        workflow: Some(workflow),
        limit: None,
    })
    .await
    {
        Ok(s) => {
            console.runs.set(Some(s));
            console.error.set(None);
        }
        Err(e) => console.error.set(Some(e)),
    }
}

async fn refresh_run_detail(console: WorkflowsConsole, run_key: String) {
    console.run_detail.set(None);
    match fetch::workflow_run_detail(run_key).await {
        Ok(d) => {
            console.run_detail.set(Some(d));
            console.error.set(None);
        }
        Err(e) => console.error.set(Some(e)),
    }
}

/// Flip a workflow's enabled flag (the Enable/Disable button).
fn toggle_enabled(state: State, console: WorkflowsConsole, id: String, enable: bool) {
    let store = move |_s: State, fresh| console.workflows.set(Some(fresh));
    if enable {
        apply_mutation(state, None, store, fetch::enable_workflow(id));
    } else {
        apply_mutation(state, None, store, fetch::disable_workflow(id));
    }
}

/// Start a workflow by hand (the Run now button) — an explicit action, so it works even while
/// disabled. The box's text is parsed as the workflow's own input JSON; blank means `{}`.
fn run_now(state: State, console: WorkflowsConsole, workflow_id: &str) {
    let raw = console.run_input.get_untracked();
    let input = if raw.trim().is_empty() {
        None
    } else {
        match serde_json::from_str(&raw) {
            Ok(v) => Some(v),
            Err(e) => {
                state.flash.set(Some(Flash::err(format!("the input box isn't valid JSON: {e}"))));
                return;
            }
        }
    };
    console.busy.set(true);
    let wf = workflow_id.to_string();
    spawn_local(async move {
        match fetch::run_workflow(RunWorkflow {
            id: wf.clone(),
            input,
            request_id: None,
        })
        .await
        {
            Ok(_) => {
                state.flash.set(None);
                refresh_runs(console, wf).await;
            }
            Err(e) => state.flash.set(Some(Flash::err(e))),
        }
        console.busy.set(false);
    });
}

/// Discard a run's journal from `index` onward (the Rewind to here button) — a genuine redo, not
/// a replay of the stale answer; confirmed first, since it throws away real work.
fn rewind_to_here(state: State, console: WorkflowsConsole, run_key: String, index: usize) {
    if !confirm(&format!(
        "Rewind {run_key} to keep only the first {index} step(s)? Everything from here onward \
         will be discarded and redone for real."
    )) {
        return;
    }
    spawn_local(async move {
        match fetch::rewind_workflow_run(run_key, index).await {
            Ok(detail) => {
                console.run_detail.set(Some(detail));
                state.flash.set(None);
            }
            Err(e) => state.flash.set(Some(Flash::err(e))),
        }
    });
}
