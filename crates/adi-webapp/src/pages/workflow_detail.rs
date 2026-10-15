//! One workflow's own page (`Route::WorkflowDetail`, `/extended/workflows/<id>`): its header
//! (scope, enabled toggle, triggers, describe error) and five tabs — Input shape, Output shape,
//! Code, Run manually, Runs.
//!
//! Reached from a row on the global Workflows page or a project's own Workflows section
//! ([`super::workflows::workflow_block`]), or directly: a deep link, a reload, or Back/Forward.
//! Which workflow and which tab are open live on [`WorkflowsConsole`] (`selected`/`tab`), set by
//! the shell's routing the same way `State::current_agent` is for the agent editor — so this one
//! view works whichever way the page was reached, with no separate load path for a cold link vs.
//! a click the way the old inline expand's `Effect` only ever fired on the global page's own view
//! (`agent:workflows-panel-scopes/memory`'s "no page-level Effect" note).

use adi_ui::{CodeEditor, CodeFrame, CodeHeight, Icon, IconSize, Lang, Lucide};
use adi_webapp_api::types::{RunWorkflow, WorkflowDto};
use gloo_timers::future::TimeoutFuture;
use leptos::prelude::*;
use serde_json::{Map, Value};
use wasm_bindgen_futures::spawn_local;

use crate::fetch;
use crate::routing::{
    ProjectSection, Route, WorkflowTab, open_project_section, push_state, spa_click, spa_nav,
    workflow_detail_path,
};
use crate::state::{Flash, State, WorkflowsConsole};
use crate::ui::flash_view;

use super::workflows::{refresh, refresh_run_detail, refresh_runs, refresh_source, run_timeline_view, runs_view, source_view, toggle_enabled};

/// The workflow detail page.
pub(crate) fn workflow_detail_view(state: State, route: RwSignal<Route>, console: WorkflowsConsole) -> AnyView {
    // Load the shared list once per visit to this page — same as the global list's own mount
    // effect, so a deep link or a reload (nothing loaded yet) and a click from the list (already
    // loaded) both end up with it fresh.
    Effect::new(move |loaded: Option<()>| {
        if loaded.is_none() {
            spawn_local(refresh(console));
        }
    });

    // Load this workflow's own source and runs, and seed the manual-run box from its input
    // schema, whenever which workflow is open changes — re-evaluated once the shared list above
    // finishes loading too (tracked via `list.is_some()` in the key), since a cold deep link
    // opens this page before that load has answered and there is nothing to look up yet on the
    // first pass.
    Effect::new(move |prev: Option<(String, bool)>| {
        let id = console.selected.get();
        let list = console.workflows.get();
        let key = (id.clone(), list.is_some());
        if id.is_empty() || prev.as_ref() == Some(&key) {
            return key;
        }
        if let Some(w) = list.and_then(|s| s.workflows.into_iter().find(|w| w.id == id)) {
            spawn_local(refresh_source(console, w.path.clone()));
            if console.seeded_for.get_untracked() != id {
                console.run_input.set(seed_json(w.input_schema.as_ref()));
                console.seeded_for.set(id.clone());
            }
        }
        spawn_local(refresh_runs(console, id.clone()));
        key
    });

    // Fetch the expanded run's own timeline whenever which one is expanded changes — the Runs
    // tab's own row click, or a run just started on the Run manually tab.
    Effect::new(move |prev: Option<String>| {
        let run_key = console.selected_run.get();
        if !run_key.is_empty() && prev.as_deref() != Some(run_key.as_str()) {
            spawn_local(refresh_run_detail(console, run_key.clone()));
        }
        run_key
    });

    view! {
        <section class="adi-panel">
            {move || head_view(state, route, console)}
            {move || tabs_nav(console)}
            <div class="adi-panel__body">
                {move || body_view(state, route, console)}
            </div>
            {flash_view(state.flash)}
        </section>
    }
    .into_any()
}

/// The open workflow's own [`WorkflowDto`] from the shared list, or `None` while it's still
/// loading (or the id names one that doesn't exist — a stale link).
fn current_workflow(console: WorkflowsConsole, id: &str) -> Option<WorkflowDto> {
    console
        .workflows
        .get()
        .and_then(|s| s.workflows.into_iter().find(|w| w.id == id))
}

/// The header: a back link (to the project's own Workflows section once its scope is known,
/// otherwise the global list), the id, scope chip, enabled toggle, triggers summary and any
/// describe error.
fn head_view(state: State, route: RwSignal<Route>, console: WorkflowsConsole) -> AnyView {
    let id = console.selected.get();
    let found = current_workflow(console, &id);
    let back = back_link(state, route, found.as_ref().and_then(|w| w.project.clone()));
    let Some(w) = found else {
        return view! {
            <div class="adi-field adi-field--full">
                <div class="adi-panel__head">
                    {back}
                    <span class="adi-updated">
                        {if id.is_empty() { "No workflow selected.".to_string() } else { "Loading…".to_string() }}
                    </span>
                </div>
            </div>
        }
        .into_any();
    };
    let enabled = w.enabled;
    let toggle_id = w.id.clone();
    view! {
        <div class="adi-field adi-field--full">
            <div class="adi-panel__head">
                {back}
                <h2 class="adi-field__label adi-mono">{w.id.clone()}</h2>
                {match w.project.clone() {
                    Some(p) => view! { <span class="adi-chip">{p}</span> }.into_any(),
                    None => view! { <span class="adi-muted">"Global"</span> }.into_any(),
                }}
                <span class="adi-tstatus" data-status=if enabled { "ready" } else { "archived" }>
                    {if enabled { "Enabled" } else { "Disabled" }}
                </span>
                <span class="adi-spacer"></span>
                <button class="adi-btn adi-btn--ghost" type="button"
                    on:click=move |_| toggle_enabled(state, console, toggle_id.clone(), !enabled)>
                    {if enabled { "Disable" } else { "Enable" }}
                </button>
            </div>
            {(!w.triggers.is_empty()).then(|| view! {
                <p class="adi-cell__sub">{w.triggers.join(" · ")}</p>
            })}
            {w.describe_error.clone().map(|e| view! {
                <p class="adi-error">"Couldn't read this file's `on` export: "{e}</p>
            })}
        </div>
    }
    .into_any()
}

/// The way back to wherever this workflow belongs: its project's own Workflows section once its
/// scope is known, the global list before that (or for a global workflow).
fn back_link(state: State, route: RwSignal<Route>, project: Option<String>) -> AnyView {
    match project {
        Some(p) => {
            let href = ProjectSection::Workflows.path(&p);
            let label = format!("{p} Workflows");
            view! {
                <a class="adi-btn adi-btn--ghost" href=href
                    on:click=move |ev| {
                        if spa_nav(&ev) {
                            open_project_section(state, route, p.clone(), ProjectSection::Workflows);
                        }
                    }>
                    <Icon icon=Lucide::ArrowLeft size=IconSize::Sm/>
                    {label}
                </a>
            }
            .into_any()
        }
        None => view! {
            <a class="adi-btn adi-btn--ghost" href=Route::Workflows.path()
                on:click=move |ev| spa_click(&ev, route, Route::Workflows)>
                <Icon icon=Lucide::ArrowLeft size=IconSize::Sm/>
                "Workflows"
            </a>
        }
        .into_any(),
    }
}

/// The five-way tab switch — reuses the segmented-control look every settings form already uses
/// ([`crate::ui::segmented`]'s own markup, just with more than two options), bound to
/// [`WorkflowsConsole::tab`] and the URL's `?tab=` together so the open tab deep-links and
/// survives a reload.
fn tabs_nav(console: WorkflowsConsole) -> AnyView {
    let id = console.selected.get();
    view! {
        <div class="adi-segmented" role="group" aria-label="Workflow view">
            {WorkflowTab::ALL.into_iter().map(|t| {
                let id = id.clone();
                view! {
                    <button class="adi-segmented__option" type="button"
                        aria-pressed=move || (console.tab.get() == t).to_string()
                        on:click=move |_| select_tab(console, &id, t)>
                        {t.title()}
                    </button>
                }
            }).collect::<Vec<_>>()}
        </div>
    }
    .into_any()
}

fn select_tab(console: WorkflowsConsole, id: &str, tab: WorkflowTab) {
    push_state(&workflow_detail_path(id, tab));
    console.tab.set(tab);
}

fn body_view(state: State, route: RwSignal<Route>, console: WorkflowsConsole) -> AnyView {
    let id = console.selected.get();
    let Some(w) = current_workflow(console, &id) else {
        return view! { <p class="adi-empty">"Loading…"</p> }.into_any();
    };
    match console.tab.get() {
        WorkflowTab::Input => schema_tab_view("Input", w.input_schema.clone()),
        WorkflowTab::Output => schema_tab_view("Output", w.output_schema.clone()),
        WorkflowTab::Code => source_view(state, route, console, w.path.clone()),
        WorkflowTab::Run => run_manually_view(state, console, &w),
        WorkflowTab::Runs => runs_view(state, console),
    }
}

/// The Input/Output shape tabs: the declared JSON Schema, pretty-printed in a read-only
/// [`CodeEditor`] — the same frame/highlight [`super::workflows::source_view`] shows the file's
/// own TypeScript in, so the three read-only tabs look like one family rather than two.
fn schema_tab_view(label: &'static str, schema: Option<Value>) -> AnyView {
    let Some(schema) = schema else {
        return view! {
            <p class="adi-empty">
                {format!("This workflow declares no {} shape.", label.to_lowercase())}
            </p>
        }
        .into_any();
    };
    let pretty = serde_json::to_string_pretty(&schema).unwrap_or_default();
    let buffer = RwSignal::new(pretty);
    let id = format!("workflow-{}-schema", label.to_lowercase());
    view! {
        <CodeFrame title=format!("{label} shape") height=CodeHeight::Form class="island">
            <CodeEditor value=buffer lang=Lang::Json readonly=true height=CodeHeight::Form id=id/>
        </CodeFrame>
    }
    .into_any()
}

/// The Run manually tab: a JSON input box pre-filled from the workflow's own input schema, a Run
/// button, and — once started — the run's key/status and its step timeline, live.
fn run_manually_view(state: State, console: WorkflowsConsole, w: &WorkflowDto) -> AnyView {
    let workflow_id = w.id.clone();
    view! {
        <div class="adi-field adi-field--full">
            <CodeFrame title="Input" height=CodeHeight::Form class="island">
                <CodeEditor value=console.run_input lang=Lang::Json height=CodeHeight::Form
                    id="workflow-run-input"/>
            </CodeFrame>
            <div class="adi-copyrow">
                <span class="adi-spacer"></span>
                <button class="adi-btn adi-btn--primary" type="button"
                    prop:disabled=move || console.busy.get()
                    on:click=move |_| run_manually(state, console, workflow_id.clone())>
                    "Run"
                </button>
            </div>
            {move || started_run_view(state, console)}
        </div>
    }
    .into_any()
}

/// Validate the input box as JSON (blank means `{}`), start the workflow, then show and poll its
/// run — reusing [`run_timeline_view`] rather than a view of its own.
fn run_manually(state: State, console: WorkflowsConsole, workflow_id: String) {
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
    let wf = workflow_id;
    spawn_local(async move {
        match fetch::run_workflow(RunWorkflow {
            id: wf.clone(),
            input,
            request_id: None,
        })
        .await
        {
            Ok(started) => {
                state.flash.set(None);
                console.run_detail.set(None);
                console.selected_run.set(started.run_key.clone());
                spawn_local(refresh_runs(console, wf));
                spawn_local(refresh_run_detail(console, started.run_key.clone()));
                spawn_local(poll_run(console, started.run_key));
            }
            Err(e) => state.flash.set(Some(Flash::err(e))),
        }
        console.busy.set(false);
    });
}

/// Poll `run_key`'s own detail every two seconds until its derived status settles (`done` /
/// `failed` / `stuck`) or the caller has moved on — either a different run got expanded/started
/// ([`WorkflowsConsole::selected_run`] no longer names this one) or the whole workflow changed
/// out from under it. A read failure also stops the loop rather than retrying forever.
async fn poll_run(console: WorkflowsConsole, run_key: String) {
    loop {
        TimeoutFuture::new(2_000).await;
        if console.selected_run.get_untracked() != run_key {
            return;
        }
        match fetch::workflow_run_detail(run_key.clone()).await {
            Ok(d) => {
                let terminal = matches!(d.status.as_str(), "done" | "failed" | "stuck");
                console.run_detail.set(Some(d));
                if terminal {
                    return;
                }
            }
            Err(_) => return,
        }
    }
}

fn started_run_view(state: State, console: WorkflowsConsole) -> AnyView {
    let run_key = console.selected_run.get();
    if run_key.is_empty() {
        return ().into_any();
    }
    let status = console
        .run_detail
        .get()
        .filter(|d| d.run_key == run_key)
        .map(|d| d.status);
    view! {
        <div class="adi-field adi-field--full">
            <div class="adi-copyrow">
                <span class="adi-mono">{run_key.clone()}</span>
                {status.map(|s| {
                    let dot = if matches!(s.as_str(), "failed" | "stuck") { "archived" } else { "ready" };
                    view! { <span class="adi-tstatus" data-status=dot>{s}</span> }.into_any()
                })}
            </div>
            {run_timeline_view(state, console, &run_key)}
        </div>
    }
    .into_any()
}

/// A skeleton value for the manual-run input box, built from the workflow's own declared input
/// schema — `{}` pretty-printed when it has none at all.
fn seed_json(schema: Option<&Value>) -> String {
    let skeleton = schema.map_or_else(|| Value::Object(Map::new()), skeleton_from_schema);
    serde_json::to_string_pretty(&skeleton).unwrap_or_else(|_| "{}".to_string())
}

/// One JSON Schema node's placeholder value: its own `default`/`examples[0]` if it declares one,
/// otherwise a bare value for its `type` — an object's own properties recurse, so a nested shape
/// gets a nested skeleton rather than just `{}`.
fn skeleton_from_schema(schema: &Value) -> Value {
    if let Some(v) = schema.get("default") {
        return v.clone();
    }
    if let Some(first) = schema.get("examples").and_then(Value::as_array).and_then(|a| a.first()) {
        return first.clone();
    }
    match schema.get("type").and_then(Value::as_str) {
        Some("string") => Value::String(String::new()),
        Some("number" | "integer") => Value::Number(0.into()),
        Some("boolean") => Value::Bool(false),
        Some("array") => Value::Array(Vec::new()),
        Some("null") => Value::Null,
        // "object", or no `type` at all (a bare `{ "properties": … }` is common) — build from
        // its properties when it has any, `{}` otherwise.
        _ => {
            let mut obj = Map::new();
            if let Some(props) = schema.get("properties").and_then(Value::as_object) {
                for (k, v) in props {
                    obj.insert(k.clone(), skeleton_from_schema(v));
                }
            }
            Value::Object(obj)
        }
    }
}
