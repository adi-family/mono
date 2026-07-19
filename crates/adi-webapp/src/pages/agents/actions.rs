//! The Agents page run, stop, and live-view actions.
//!
//! An agent definition is a *template*. For interactive (tmux) backends a Run starts a session you
//! type into and View watches its pane. For headless (`process` / `harness`) backends each Run is an
//! independent run of the agent's settings (a fresh dialog, never continued): every run keeps its
//! own log, several may be live at once, and the live view is a browsable run history plus a task
//! composer — never a shared, overwritten slot.

use adi_webapp_api::types::{AgentDto, AgentRunInfo, AgentsState};
use leptos::prelude::*;
use wasm_bindgen_futures::spawn_local;

use crate::fetch;
use crate::highlight::Lang;
use crate::routing::scroll_top;
use crate::state::{AgentsWatch, Flash, State};
use crate::ui::{apply_mutation, code_viewer, data_table};

use super::send_bar;

/// The Run / View / Stop action buttons for one agent row. Interactive Run starts a tmux session
/// straight away; headless "Run…" opens the run panel, where a task is entered before launching — a
/// headless `--print` run is seeded by one prompt, not typed into. View opens the same panel (a live
/// pane for tmux, the run history for headless); Stop ends the session, or every live run.
pub(crate) fn agent_actions(state: State, watch: AgentsWatch, a: &AgentDto) -> AnyView {
    let run_name = a.name.clone();
    let show_run = a.runnable && !a.running;
    let running = a.running;
    let interactive = a.executor == "tmux";
    let stop_title = if interactive {
        "kill the tmux session"
    } else {
        "stop every live run"
    };
    let view_title = if interactive {
        "watch the live tmux session"
    } else {
        "browse this agent's runs"
    };
    view! {
        {running.then(|| {
            let watch_name = run_name.clone();
            let stop_name = run_name.clone();
            view! {
                <button class="adi-btn adi-btn--link" title=view_title
                    on:click=move |_| open_watch(watch, watch_name.clone(), interactive)>"● View"</button>
                " "
                <button class="adi-btn adi-btn--link" title=stop_title
                    on:click=move |_| stop_agent(state, watch, stop_name.clone())>"■ Stop"</button>
                " "
            }
        })}
        {show_run.then(|| {
            let run_name = run_name.clone();
            if interactive {
                view! {
                    <button class="adi-btn adi-btn--link" title="start an interactive tmux session"
                        on:click=move |_| run_now(state, run_name.clone())>"▶ Run"</button>
                    " "
                }
                .into_any()
            } else {
                view! {
                    <button class="adi-btn adi-btn--link" title="give it a task and run it headless"
                        on:click=move |_| open_watch(watch, run_name.clone(), false)>"▶ Run…"</button>
                    " "
                }
                .into_any()
            }
        })}
    }
    .into_any()
}

/// Run an agents mutation: set the returned list and a success flash, or an error flash; toggles
/// `busy` around the request when a form is driving it.
pub(crate) fn apply_agents<F>(state: State, busy: Option<RwSignal<bool>>, ok_msg: String, fut: F)
where
    F: std::future::Future<Output = Result<AgentsState, String>> + 'static,
{
    apply_mutation(state, busy, ok_msg, |s, a| s.agents.set(Some(a)), fut);
}

/// Launch an interactive (tmux) agent straight away — no initial task, since the session is typed
/// into after it starts. The server supplies the executor-specific success message.
fn run_now(state: State, name: String) {
    spawn_local(async move {
        match fetch::run_agent(name, String::new()).await {
            Ok(res) => {
                state.agents.set(Some(res.state));
                state.flash.set(Some(Flash::ok(res.message)));
            }
            Err(e) => state.flash.set(Some(Flash::err(e))),
        }
    });
}

/// Launch a new headless run of the agent with `message` as its task, then select that run in the
/// panel so its log streams in. Each launch is independent — never a continuation of a prior run.
fn launch_agent(state: State, watch: AgentsWatch, name: String, message: String) {
    spawn_local(async move {
        match fetch::run_agent(name.clone(), message).await {
            Ok(res) => {
                state.agents.set(Some(res.state));
                state.flash.set(Some(Flash::ok(res.message)));
                watch.peek.set(None);
                watch.log.set(String::new());
                if !res.run_id.is_empty() {
                    watch.run_id.set(Some(res.run_id));
                }
                watch.name.set(Some(name));
                poll_watch(watch);
            }
            Err(e) => state.flash.set(Some(Flash::err(e))),
        }
    });
}

/// Stop the whole agent (the tmux session, or every live run of a headless one), refresh the list,
/// and close its live view.
fn stop_agent(state: State, watch: AgentsWatch, name: String) {
    if watch.name.get_untracked().as_deref() == Some(name.as_str()) {
        watch.close();
    }
    apply_agents(
        state,
        None,
        format!("Stopped {name}."),
        fetch::stop_agent(name),
    );
}

/// Stop one specific run of a headless agent, then refresh the run history and the agent list (so
/// the row's running flag settles).
fn stop_one_run(state: State, watch: AgentsWatch, run_id: String) {
    let Some(name) = watch.name.get_untracked() else {
        return;
    };
    spawn_local(async move {
        match fetch::stop_run(name.clone(), run_id).await {
            Ok(runs) => {
                if watch.name.get_untracked().as_deref() == Some(name.as_str()) {
                    watch.runs.set(runs.runs);
                }
                if let Ok(st) = fetch::agents().await {
                    state.agents.set(Some(st));
                }
            }
            Err(e) => state.flash.set(Some(Flash::err(e))),
        }
    });
}

/// Open the run panel on an agent (View / Run…): remember whether it is interactive, clear any
/// previous run selection, fetch the first snapshot (the 1s poll takes over), and scroll to it.
fn open_watch(watch: AgentsWatch, name: String, interactive: bool) {
    watch.peek.set(None);
    watch.log.set(String::new());
    watch.run_id.set(None);
    watch.runs.set(Vec::new());
    watch.interactive.set(interactive);
    watch.name.set(Some(name));
    poll_watch(watch);
    scroll_top();
}

/// Select a run of a headless agent to view its log. Clears the previous run's tail so the viewer
/// doesn't briefly show it before the first poll of the newly selected run lands.
fn select_run(watch: AgentsWatch, run_id: String) {
    watch.peek.set(None);
    watch.log.set(String::new());
    watch.run_id.set(Some(run_id));
    poll_watch(watch);
}

/// Close the expanded log view (the detail row's Close, or a second click on the row's own button):
/// deselect the run so its detail row collapses, and drop the tail so reopening starts clean. The
/// run history stays open — this closes only the log, not the whole panel.
fn close_run_view(watch: AgentsWatch) {
    watch.run_id.set(None);
    watch.peek.set(None);
    watch.log.set(String::new());
}

/// Refresh the open live view. The shell calls this every second; it no-ops while closed. For an
/// interactive agent it fetches the pane; for a headless one it refreshes the run history and, if a
/// run is selected, that run's log. A response landing after the view moved on is dropped.
pub(crate) fn poll_watch(watch: AgentsWatch) {
    let Some(name) = watch.name.get_untracked() else {
        return;
    };
    if watch.interactive.get_untracked() {
        spawn_local(async move {
            if let Ok(peek) = fetch::peek_agent(name).await
                && watch.name.get_untracked().as_deref() == Some(peek.name.as_str())
            {
                watch.peek.set(Some(peek));
            }
        });
        return;
    }
    // Headless: refresh the run history… Only write on change, so a settled history doesn't
    // re-render the table (and its "N ago" ages) every second for nothing.
    {
        let name = name.clone();
        spawn_local(async move {
            if let Ok(runs) = fetch::agent_runs(name.clone()).await
                && watch.name.get_untracked().as_deref() == Some(name.as_str())
                && watch.runs.get_untracked() != runs.runs
            {
                watch.runs.set(runs.runs);
            }
        });
    }
    // …and the selected run's log, if one is selected. The tail feeds a dedicated `log` signal
    // that the inline viewer follows; both `log` and `peek` are written only when they actually
    // change, so a finished run's viewer sits perfectly still (no per-second churn or scroll nudge)
    // while a live one still updates as it grows.
    if let Some(run_id) = watch.run_id.get_untracked() {
        spawn_local(async move {
            if let Ok(peek) = fetch::peek_run(name.clone(), run_id).await
                && watch.name.get_untracked().as_deref() == Some(name.as_str())
                && watch.run_id.get_untracked().as_deref() == Some(peek.run_id.as_str())
            {
                if watch.log.get_untracked() != peek.output {
                    watch.log.set(peek.output.clone());
                }
                if watch.peek.get_untracked().as_ref() != Some(&peek) {
                    watch.peek.set(Some(peek));
                }
            }
        });
    }
}

/// The live-view / run panel. Renders nothing while no agent is watched. Shared with a project's
/// Agents panel. Interactive backends show a live pane + send bar; headless ones show a run history.
pub(crate) fn live_view(state: State, watch: AgentsWatch) -> Option<AnyView> {
    let name = watch.name.get()?;
    if watch.interactive.get() {
        Some(tmux_live_view(state, watch, name))
    } else {
        Some(runs_panel(state, watch, name))
    }
}

/// The interactive (tmux) live view: a 1s-refreshed pane capture with a send bar to type into it.
fn tmux_live_view(state: State, watch: AgentsWatch, name: String) -> AnyView {
    let peek = watch.peek.get();
    let attach = peek.as_ref().map(|p| p.attach.clone()).unwrap_or_default();
    let running = peek.as_ref().is_some_and(|p| p.running);
    let body = match peek {
        None => view! { <div class="adi-empty">"Connecting…"</div> }.into_any(),
        Some(p) if !p.running => view! {
            <div class="adi-empty">"The session has ended — run the agent again to restart it."</div>
        }
        .into_any(),
        Some(p) => view! { <pre class="adi-term">{p.output}</pre> }.into_any(),
    };
    view! {
        <section class="adi-panel">
            <div class="adi-panel__head">
                <h2 class="adi-panel__title">{format!("Live view — {name}")}</h2>
                <span class="adi-spacer"></span>
                {(!attach.is_empty()).then(|| view! {
                    <code class="adi-mono adi-muted" style="font-size:var(--text-sm)">{attach}</code>
                })}
                <button class="adi-btn adi-btn--link" on:click=move |_| watch.close()>"Close"</button>
            </div>
            {body}
            {running.then(|| send_bar(state, watch))}
        </section>
    }
    .into_any()
}

/// The headless run panel: a task composer and this agent's run history (newest first, each with
/// View / Stop). Viewing a run expands its log inline as a detail row directly beneath that run's
/// row (see [`runs_list`]) — an inline viewer that follows the tail.
///
/// The per-poll signal reads are deferred into the nested `{move || …}` island rather than read
/// here, so the 1s poll re-renders only what changed: the history table on a new run, the viewer's
/// content as the log grows. The table re-renders only when the history or selection changes (both
/// gated on real change), so a live run's log grows in place without rebuilding — and so tearing —
/// the expanded viewer.
fn runs_panel(state: State, watch: AgentsWatch, name: String) -> AnyView {
    view! {
        <section class="adi-panel">
            <div class="adi-panel__head">
                <h2 class="adi-panel__title">{format!("Runs — {name}")}</h2>
                <span class="adi-chip adi-mono" title="runs in history">
                    {move || watch.runs.get().len().to_string()}
                </span>
                <span class="adi-spacer"></span>
                <button class="adi-btn adi-btn--link" on:click=move |_| watch.close()>"Close"</button>
            </div>
            {run_bar(state, watch)}
            {move || runs_list(state, watch)}
        </section>
    }
    .into_any()
}

/// The run-history table (or the empty hint). The selected run's row is immediately followed by a
/// full-width detail row holding its log viewer, so the log opens right under the run it belongs to
/// rather than beneath the whole table. Reads the history and the selection, so it re-renders when
/// a run is added or stops, or the viewed run changes — never for log growth.
fn runs_list(state: State, watch: AgentsWatch) -> AnyView {
    let runs = watch.runs.get();
    if runs.is_empty() {
        return view! {
            <div class="adi-empty">"No runs yet — type a task above and press Run."</div>
        }
        .into_any();
    }
    let selected = watch.run_id.get();
    let mut rows: Vec<AnyView> = Vec::with_capacity(runs.len() + 1);
    for r in &runs {
        let is_selected = selected.as_deref() == Some(r.run_id.as_str());
        rows.push(run_row(state, watch, r, selected.as_deref()));
        // The log opens as a detail row right beneath the run it belongs to.
        if is_selected {
            rows.push(run_detail_row(watch, r.run_id.clone()));
        }
    }
    data_table(&["When", "Status", "Task", ""], rows).into_any()
}

/// The expanded log for the selected run: a full-width table row directly under it, holding the run
/// header (id, the `tail -f` hint, a Close) and the inline viewer that follows the tail. Built once
/// per selected run — the log merely growing updates the bound `log` signal in place rather than
/// rebuilding this row, so the follow-scroll is never reset.
fn run_detail_row(watch: AgentsWatch, run_id: String) -> AnyView {
    view! {
        <tr class="adi-runlog">
            <td class="adi-runlog__cell" colspan="4">
                // A titled, bordered terminal card — obviously a log console, not another grey row.
                <div class="adi-runlog__card">
                    <div class="adi-runlog__bar">
                        <span class="adi-runlog__title">"\u{25A4} Log"</span>
                        <span class="adi-runlog__run adi-mono">{run_id}</span>
                        <span class="adi-spacer"></span>
                        {move || run_log_status(watch)}
                        <button class="adi-runlog__close" type="button" title="close this log"
                            on:click=move |_| close_run_view(watch)>"\u{2715} Close"</button>
                    </div>
                    {move || watch.log.get().is_empty().then(|| run_log_empty(watch))}
                    {code_viewer(move || Lang::None, watch.log, "", "run-log")}
                </div>
            </td>
        </tr>
    }
    .into_any()
}

/// The title bar's `tail -f <log>` hint, shown once a snapshot has landed — the human-runnable
/// equivalent of what the console below shows.
fn run_log_status(watch: AgentsWatch) -> Option<AnyView> {
    let attach = watch.peek.get().map(|p| p.attach).unwrap_or_default();
    (!attach.is_empty()).then(|| {
        view! { <code class="adi-runlog__cmd adi-mono">{attach}</code> }.into_any()
    })
}

/// The placeholder inside the (empty) console before any output: waiting while the run is live, or
/// a plain note once it has finished with nothing to show.
fn run_log_empty(watch: AgentsWatch) -> AnyView {
    let msg = match watch.peek.get() {
        None => "Loading…",
        Some(p) if p.running => "Running — waiting for output…",
        Some(_) => "No output yet.",
    };
    view! { <div class="adi-runlog__empty">{msg}</div> }.into_any()
}

/// One run row in the history table: when it started, status, its task, and View / Stop.
fn run_row(state: State, watch: AgentsWatch, r: &AgentRunInfo, selected: Option<&str>) -> AnyView {
    let run_id = r.run_id.clone();
    let is_selected = selected == Some(run_id.as_str());
    let running = r.running;
    let when = run_age(r.started_at);
    let task_full = r.message.clone();
    let task_short = truncate_task(&task_full);
    let status = if running { "● running" } else { "done" };
    let view_id = run_id.clone();
    let stop_id = run_id.clone();
    let row_style = if is_selected {
        "background:var(--surface-2)"
    } else {
        ""
    };
    // The action toggles this row's log: View opens the drawer beneath it, and while open it reads
    // "● Viewing" and a second click collapses it. Only the drawer carries an explicit "Close", so
    // there is one thing labelled Close, not two.
    let view_label = if is_selected { "● Viewing" } else { "View" };
    view! {
        <tr style=row_style>
            <td class="adi-muted" style="white-space:nowrap">{when}</td>
            <td>{status}</td>
            <td class="adi-mono" title=task_full>{task_short}</td>
            <td class="adi-table__actions">
                <button class="adi-btn adi-btn--link"
                    on:click=move |_| if is_selected {
                        close_run_view(watch);
                    } else {
                        select_run(watch, view_id.clone());
                    }>{view_label}</button>
                " "
                {running.then(|| { let stop_id = stop_id.clone(); view! {
                    <button class="adi-btn adi-btn--link" title="stop this run"
                        on:click=move |_| stop_one_run(state, watch, stop_id.clone())>"Stop"</button>
                }})}
            </td>
        </tr>
    }
    .into_any()
}

/// The run composer: a task input plus a Run button. A headless run is one `--print` turn seeded by
/// this prompt, so a task is required — the button stays disabled (and submit no-ops) until one is
/// typed. Submitting launches a new run and streams its log.
fn run_bar(state: State, watch: AgentsWatch) -> impl IntoView {
    view! {
        <form class="adi-form"
            on:submit=move |ev| {
                ev.prevent_default();
                let Some(name) = watch.name.get_untracked() else { return; };
                let message = watch.input.get();
                if message.trim().is_empty() {
                    return;
                }
                watch.input.set(String::new());
                launch_agent(state, watch, name, message);
            }>
            <input class="adi-input adi-input--wide adi-mono" autocomplete="off"
                placeholder="task for a new run (required) — e.g. review the latest commit and summarize it"
                prop:value=move || watch.input.get()
                on:input=move |ev| watch.input.set(event_target_value(&ev)) />
            <button class="adi-btn adi-btn--primary" type="submit"
                prop:disabled=move || watch.input.get().trim().is_empty()>"▶ Run"</button>
        </form>
    }
}

/// A short "N ago" for a run's start time (unix ms), against the browser clock. The panel re-renders
/// each second (the poll refreshes the run list), so this stays roughly live.
fn run_age(started_at_ms: u64) -> String {
    if started_at_ms == 0 {
        return String::new();
    }
    let now = js_sys::Date::now() as u64;
    let secs = now.saturating_sub(started_at_ms) / 1000;
    if secs < 5 {
        "just now".to_string()
    } else if secs < 60 {
        format!("{secs}s ago")
    } else if secs < 3_600 {
        format!("{}m ago", secs / 60)
    } else if secs < 86_400 {
        format!("{}h ago", secs / 3_600)
    } else {
        format!("{}d ago", secs / 86_400)
    }
}

/// Clip a task to a single readable line for the history table; the full text is the cell's title.
fn truncate_task(task: &str) -> String {
    const MAX: usize = 72;
    if task.chars().count() > MAX {
        format!("{}…", task.chars().take(MAX).collect::<String>())
    } else {
        task.to_string()
    }
}
