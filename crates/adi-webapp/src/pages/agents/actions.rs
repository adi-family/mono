//! The Agents page run, stop, and live-view actions.
//!
//! An agent definition is a *template*. For interactive (pty) backends a Run starts a session you
//! type into and View watches its pane. For headless (`process` / `harness`) backends each Run is an
//! independent run of the agent's settings (a fresh dialog, never continued): every run keeps its
//! own log, several may be live at once, and the live view is a browsable run history plus a task
//! composer — never a shared, overwritten slot.

use adi_webapp_api::types::{
    AgentDto, AgentRunInfo, AgentStep, AgentToolStatus, AgentTurn, AgentTurnMetrics, AgentsState,
    Dashboard,
};
use leptos::prelude::*;
use wasm_bindgen_futures::spawn_local;

use crate::fetch;
use crate::routing::scroll_top;
use crate::state::{AgentsWatch, Flash, ROOT_AGENT, State};
use crate::ui::{
    Key, Sort, TableState, apply_mutation, body_row, configurable_table, placeholder_row, sort_rows,
};

use super::send_bar;

/// The cross-agent **All chats** index's columns; the trailing blank one holds Open.
pub(crate) const CHAT_COLS: &[&str] = &["Agent", "When", "Status", "Conversation", ""];

/// One agent's run history, for a backend whose runs read as a conversation.
pub(crate) const CHAT_RUN_COLS: &[&str] = &["When", "Status", "Conversation", ""];

/// The same for a one-shot backend, where a run is a task rather than a dialog.
pub(crate) const RUN_COLS: &[&str] = &["When", "Status", "Task", ""];

/// A run list reads newest-first, so these tables declare that rather than inheriting the
/// first-column-ascending default — which would silently show the oldest run at the top.
pub(crate) const NEWEST_FIRST: Sort = Sort {
    col: "When",
    desc: true,
};

/// The Run / View / Stop action buttons for one agent row. Interactive Run starts a pty session
/// straight away; headless "Run…" opens the run panel, where a task is entered before launching — a
/// headless `--print` run is seeded by one prompt, not typed into. View opens the same panel (a live
/// pane for pty, the run history for headless); Stop ends the session, or every live run.
pub(crate) fn agent_actions(state: State, watch: AgentsWatch, a: &AgentDto) -> AnyView {
    let run_name = a.name.clone();
    let show_run = a.runnable && !a.running;
    let running = a.running;
    let interactive = a.executor == "pty";
    // Harness backends keep answerable conversations; the run controls read as a chat there.
    let answerable = a.executor == "harness";
    let stop_title = if interactive {
        "kill the session"
    } else if answerable {
        "stop the current answer of every live conversation"
    } else {
        "stop every live run"
    };
    let view_title = if interactive {
        "watch the live session"
    } else if answerable {
        "open this agent's conversations"
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
                    <button class="adi-btn adi-btn--link" title="start an interactive session"
                        on:click=move |_| run_now(state, run_name.clone())>"▶ Run"</button>
                    " "
                }
                .into_any()
            } else if answerable {
                view! {
                    <button class="adi-btn adi-btn--link" title="start a conversation you can answer"
                        on:click=move |_| open_watch(watch, run_name.clone(), false)>"▶ Chat…"</button>
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

/// Launch an interactive (pty) agent straight away — no initial task, since the session is typed
/// into after it starts. The server supplies the executor-specific success message.
fn run_now(state: State, name: String) {
    spawn_local(async move {
        match fetch::run_agent(name, String::new(), None).await {
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
/// `working_dir` is the composer's optional "run here"; `None` starts the agent where it is defined.
fn launch_agent(state: State, watch: AgentsWatch, name: String, message: String, working_dir: Option<String>) {
    spawn_local(async move {
        match fetch::run_agent(name.clone(), message, working_dir).await {
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

/// Stop the whole agent (the pty session, or every live run of a headless one), refresh the list,
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

/// Stop one specific run of a headless agent — for a conversation, the answer being produced, and
/// with it anything queued behind that answer. Then refresh the run history and the agent list (so
/// the row's running flag settles), and re-poll so the cut-short turn appears settled at once rather
/// than a second later.
fn stop_one_run(state: State, watch: AgentsWatch, run_id: String) {
    let Some(name) = watch.name.get_untracked() else {
        return;
    };
    if run_id.is_empty() {
        return;
    }
    spawn_local(async move {
        match fetch::stop_run(name.clone(), run_id).await {
            Ok(runs) => {
                if watch.name.get_untracked().as_deref() == Some(name.as_str()) {
                    watch.runs.set(runs.runs);
                    poll_watch(watch);
                }
                if let Ok(st) = fetch::agents().await {
                    state.agents.set(Some(st));
                }
            }
            Err(e) => state.flash.set(Some(Flash::err(e))),
        }
    });
}

/// Delete one run — for a harness agent, the whole conversation, transcript and all — behind an
/// explicit confirmation, since nothing here is recoverable. A live run is stopped by the server
/// first. If the deleted run is the one on screen, its detail view closes rather than polling a
/// conversation that no longer exists.
fn delete_one_run(state: State, watch: AgentsWatch, run_id: String, title: String) {
    let Some(name) = watch.name.get_untracked() else {
        return;
    };
    if run_id.is_empty() {
        return;
    }
    let what = if title.trim().is_empty() {
        "this chat".to_string()
    } else {
        format!("“{}”", title.trim())
    };
    if !crate::ui::confirm(&format!(
        "Permanently delete {what}? Its whole transcript goes with it, and this cannot be undone."
    )) {
        return;
    }
    if watch.run_id.get_untracked().as_deref() == Some(run_id.as_str()) {
        close_run_view(watch);
    }
    spawn_local(async move {
        match fetch::delete_run(name.clone(), run_id).await {
            Ok(runs) => {
                if watch.name.get_untracked().as_deref() == Some(name.as_str()) {
                    watch.runs.set(runs.runs);
                }
                // The agent list carries a running flag that a deleted live run may have settled.
                if let Ok(st) = fetch::agents().await {
                    state.agents.set(Some(st));
                }
            }
            Err(e) => state.flash.set(Some(Flash::err(e))),
        }
    });
}

/// Point the live view at an agent: drop everything the one before it left behind (its snapshot, log
/// tail, run selection and history), remember whether this one is interactive, and fetch the first
/// snapshot — the 1s poll takes over from there. The view moves; nothing on the page scrolls, which
/// is what the chat home wants when its picker switches agents mid-screen.
fn point_watch(watch: AgentsWatch, name: String, interactive: bool) {
    watch.peek.set(None);
    watch.log.set(String::new());
    watch.run_id.set(None);
    watch.runs.set(Vec::new());
    // Reset until the first history poll reports whether this backend keeps answerable conversations.
    watch.answerable.set(false);
    watch.reply.set(String::new());
    watch.interactive.set(interactive);
    watch.name.set(Some(name));
    poll_watch(watch);
}

/// Open the run panel on an agent (View / Run…): point the view at it, then scroll to the panel —
/// on the Agents page it sits below the list, so a click near the bottom would otherwise open it
/// out of sight.
fn open_watch(watch: AgentsWatch, name: String, interactive: bool) {
    point_watch(watch, name, interactive);
    scroll_top();
}

/// Select a run of a headless agent to view its log (or, for a harness backend, its conversation).
/// Clears the previous run's tail and reply draft so nothing bleeds across before the first poll of
/// the newly selected run lands.
fn select_run(watch: AgentsWatch, run_id: String) {
    watch.peek.set(None);
    watch.log.set(String::new());
    watch.reply.set(String::new());
    watch.run_id.set(Some(run_id));
    poll_watch(watch);
}

/// Prepend the watch's context prefix (if any) to a message before it is sent — how the
/// dashboard-agent embed tags every message with which dashboard it was opened from. Inert (returns
/// the message unchanged) whenever no prefix is set, so the normal app is unaffected.
fn with_context(watch: AgentsWatch, message: String) -> String {
    let prefix = watch.context_prefix.get_untracked();
    if prefix.trim().is_empty() {
        message
    } else {
        format!("{prefix}\n\n{message}")
    }
}

/// Point the shared live view at one specific conversation — its agent *and* that run — so the
/// transcript is what shows. Interactive agents keep no run history, so `run_id` is only selected
/// when present. Used wherever a conversation is picked from a list that spans several agents.
fn point_conversation(watch: AgentsWatch, name: String, run_id: String, interactive: bool) {
    point_watch(watch, name, interactive);
    if !run_id.is_empty() {
        watch.run_id.set(Some(run_id));
        poll_watch(watch);
    }
}

/// Open a specific conversation from the cross-agent "All chats" index: as above, plus a scroll to
/// the panel it opens in, which on the Agents page sits below the index.
pub(crate) fn open_conversation(
    watch: AgentsWatch,
    name: String,
    run_id: String,
    interactive: bool,
) {
    point_conversation(watch, name, run_id, interactive);
    scroll_top();
}

/// Close the expanded log view (the detail row's Close, or a second click on the row's own button):
/// deselect the run so its detail row collapses, and drop the tail so reopening starts clean. The
/// run history stays open — this closes only the log, not the whole panel.
fn close_run_view(watch: AgentsWatch) {
    watch.run_id.set(None);
    watch.peek.set(None);
    watch.log.set(String::new());
    watch.reply.set(String::new());
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
            {
                // Whether these runs are answerable conversations — drives the chat vs. log view.
                if watch.answerable.get_untracked() != runs.answerable {
                    watch.answerable.set(runs.answerable);
                }
                if watch.runs.get_untracked() != runs.runs {
                    watch.runs.set(runs.runs);
                }
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
        Some(pty_live_view(state, watch, name))
    } else {
        Some(runs_panel(state, watch, name))
    }
}

/// The cross-agent **All chats** index: every conversation across the agents visible on the page,
/// newest first, each openable in the shared live view below. `only` restricts it to agents filed
/// under the given project ids (the project detail page passes its project + sub-projects); `None`
/// includes every agent (the standalone Agents page). Its own reactive island is the table, so the
/// 1s/4s polls refresh the list in place without rebuilding the panel.
pub(crate) fn all_chats_view(state: State, watch: AgentsWatch, only: Option<Vec<String>>) -> AnyView {
    let only_head = only.clone();
    view! {
        <section class="adi-panel">
            <div class="adi-panel__head">
                <h2 class="adi-panel__title">"All chats"</h2>
                <span class="adi-chip adi-mono" title="conversations across every agent on this page">
                    {move || all_chats_flatten(state, &only_head).len().to_string()}
                </span>
                <span class="adi-spacer"></span>
                <span class="adi-updated">"every agent's conversations — open one below"</span>
            </div>
            {configurable_table(state.tables.chats, CHAT_COLS,
                move || all_chats_rows(state, watch, &only))}
        </section>
    }
    .into_any()
}

/// Flatten every included agent's runs into `(agent, answerable, interactive, run)` tuples, newest
/// first. `only` (project ids) filters by each agent's project, read from the loaded agents list;
/// `None` includes them all.
fn all_chats_flatten(
    state: State,
    only: &Option<Vec<String>>,
) -> Vec<(String, bool, bool, AgentRunInfo)> {
    let Some(all) = state.all_chats.get() else {
        return Vec::new();
    };
    let project_of: std::collections::HashMap<String, Option<String>> = state
        .agents
        .get()
        .map(|a| a.agents.into_iter().map(|d| (d.name, d.project)).collect())
        .unwrap_or_default();
    let included = |name: &str| match only {
        None => true,
        Some(ids) => project_of
            .get(name)
            .and_then(|p| p.as_deref())
            .is_some_and(|p| ids.iter().any(|id| id == p)),
    };
    let mut rows: Vec<(String, bool, bool, AgentRunInfo)> = Vec::new();
    for ar in all.agents {
        if !included(&ar.name) {
            continue;
        }
        for r in ar.runs {
            rows.push((ar.name.clone(), ar.answerable, ar.interactive, r));
        }
    }
    // Newest conversation first, across all agents.
    rows.sort_by(|a, b| b.3.started_at.cmp(&a.3.started_at));
    rows
}

/// Rows for the All chats table: one per conversation (its agent, age, status, first message, and an
/// Open that reveals it in the live view below). Loading/empty placeholders otherwise.
fn all_chats_rows(state: State, watch: AgentsWatch, only: &Option<Vec<String>>) -> AnyView {
    let table = state.tables.chats;
    let layout = table.layout.get();
    if state.all_chats.get().is_none() {
        return placeholder_row(layout.span(), "Loading…");
    }
    let mut rows = all_chats_flatten(state, only);
    if rows.is_empty() {
        return placeholder_row(
            layout.span(),
            "No chats yet — start one from an agent below.",
        );
    }
    // By the start time, not the "3m ago" the cell renders — the two disagree the moment the
    // ages cross a unit boundary. Ties go to the newest, so the index stays newest-first.
    sort_rows(
        &mut rows,
        table.sort.get(),
        |(agent, answerable, _, r), col| match col {
            "Agent" => Key::text(agent),
            "Status" => Key::text(run_status(*answerable, r.running)),
            "Conversation" => Key::text(&r.message),
            _ => Key::num(r.started_at),
        },
        |(_, _, _, r)| Key::num(u64::MAX - r.started_at),
    );
    let shown = layout.shown();
    rows.into_iter()
        .map(|(agent, answerable, interactive, r)| {
            let (name, run_id) = (agent.clone(), r.run_id.clone());
            let open = view! {
                <button class="adi-btn adi-btn--link"
                    on:click=move |_| open_conversation(watch, name.clone(), run_id.clone(), interactive)>
                    "Open"
                </button>
            }
            .into_any();
            body_row(
                &shown,
                |col| match col {
                    "Agent" => view! { <td class="adi-mono">{agent.clone()}</td> }.into_any(),
                    other => run_cell(other, &r, answerable),
                },
                Some(open),
            )
        })
        .collect::<Vec<_>>()
        .into_any()
}

/// A run's status word: what it says depends on whether the backend holds a conversation or runs
/// a task to completion. Shared by the cross-agent index and one agent's history.
fn run_status(answerable: bool, running: bool) -> &'static str {
    match (answerable, running) {
        (true, true) => "\u{25CF} answering",
        (true, false) => "idle",
        (false, true) => "\u{25CF} running",
        (false, false) => "done",
    }
}

/// One run's cell under `col`. `Conversation` and `Task` are the same cell under two headers —
/// which of them a table declares is what the backend's kind decides. Matching the header text —
/// the same key the sort uses — is what lets the user hide and reorder columns without the row
/// builder knowing about it.
fn run_cell(col: &str, r: &AgentRunInfo, answerable: bool) -> AnyView {
    match col {
        "Status" => view! { <td>{run_status(answerable, r.running)}</td> }.into_any(),
        "Conversation" | "Task" => {
            let full = r.message.clone();
            let short = truncate_task(&full);
            view! { <td class="adi-mono" title=full>{short}</td> }.into_any()
        }
        // "When", and anything the layout offers that this match doesn't name.
        _ => view! {
            <td class="adi-muted" style="white-space:nowrap">{run_age(r.started_at)}</td>
        }
        .into_any(),
    }
}

/// The interactive (pty) live view: a 1s-refreshed pane capture with a send bar to type into it.
fn pty_live_view(state: State, watch: AgentsWatch, name: String) -> AnyView {
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
    let title_name = name;
    view! {
        <section class="adi-panel">
            <div class="adi-panel__head">
                <h2 class="adi-panel__title">
                    {move || if watch.answerable.get() {
                        format!("Chats — {title_name}")
                    } else {
                        format!("Runs — {title_name}")
                    }}
                </h2>
                <span class="adi-chip adi-mono" title="conversations in history">
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
    let mut runs = watch.runs.get();
    // Read here so the list re-renders (labels, headers, empty text) when the backend's kind lands.
    let answerable = watch.answerable.get();
    if runs.is_empty() {
        let msg = if answerable {
            "No conversations yet — type a message above to start one."
        } else {
            "No runs yet — type a task above and press Run."
        };
        return view! { <div class="adi-empty">{msg}</div> }.into_any();
    }
    // Two backends, two column sets — and so two arrangements to remember, since a conversation
    // history and a task history are not the same table.
    let (table, headers): (TableState, &'static [&'static str]) = if answerable {
        (state.tables.chat_runs, CHAT_RUN_COLS)
    } else {
        (state.tables.runs, RUN_COLS)
    };
    let layout = table.layout.get();
    sort_rows(
        &mut runs,
        table.sort.get(),
        |r, col| match col {
            "Status" => Key::text(run_status(answerable, r.running)),
            "Conversation" | "Task" => Key::text(&r.message),
            _ => Key::num(r.started_at),
        },
        |r| Key::num(u64::MAX - r.started_at),
    );
    let shown = layout.shown();
    let selected = watch.run_id.get();
    let mut rows: Vec<AnyView> = Vec::with_capacity(runs.len() + 1);
    for r in &runs {
        let is_selected = selected.as_deref() == Some(r.run_id.as_str());
        rows.push(run_row(state, watch, r, &shown, is_selected, answerable));
        // The log / chat opens as a detail row right beneath the run it belongs to.
        if is_selected {
            rows.push(run_detail_row(
                state,
                watch,
                r.run_id.clone(),
                layout.span(),
                answerable,
            ));
        }
    }
    configurable_table(table, headers, rows).into_any()
}

/// The expanded log for the selected run: a full-width table row directly under it, holding the run
/// header (id, the `tail -f` hint, a Close) and the inline viewer that follows the tail. Built once
/// per selected run — the log merely growing updates the bound `log` signal in place rather than
/// rebuilding this row, so the follow-scroll is never reset.
fn run_detail_row(
    state: State,
    watch: AgentsWatch,
    run_id: String,
    // Whatever the user left showing — a drawer narrower than its table would leave a gap.
    span: usize,
    answerable: bool,
) -> AnyView {
    // Conversations read as a chat; one-shot runs as a progress feed of the same shape.
    let title = if answerable { "\u{25A4} Chat" } else { "\u{25A4} Run" };
    view! {
        <tr class="adi-runlog">
            <td class="adi-runlog__cell" colspan=span>
                // A titled, bordered card — obviously a console/chat, not another grey row.
                <div class="adi-runlog__card">
                    <div class="adi-runlog__bar">
                        <span class="adi-runlog__title">{title}</span>
                        <span class="adi-runlog__run adi-mono">{run_id}</span>
                        <span class="adi-spacer"></span>
                        // The `tail -f` hint stays available for following the raw log by hand.
                        {move || run_log_status(watch)}
                        <button class="adi-runlog__close" type="button" title="close this"
                            on:click=move |_| close_run_view(watch)>"\u{2715} Close"</button>
                    </div>
                    {feed_view(state, watch, answerable)}
                </div>
            </td>
        </tr>
    }
    .into_any()
}

/// The progress feed under a selected run: the turns (each with its tool/thinking steps and metrics)
/// which the poll refreshes as they stream in — plus, for answerable backends, the reply box.
///
/// The transcript reads as one alternating stream — message, that turn's tool calls, the message
/// before it, its tool calls, and so on — **newest first**, so the latest answer is at the top.
///
/// The reply box sits at the **top**, above the transcript — new turns land at the top of the
/// transcript, right beneath where you type. The scroll container (`.adi-chat`) is built **once**
/// here, never inside the 1s poll's reactive island — so its scroll offset survives a refresh
/// instead of snapping to the top every second. Inside it, a keyed [`For`] reconciles the transcript:
/// a settled turn keeps its exact DOM (and the scroll position with it); only the still-streaming
/// turn — whose key folds in its growth — re-renders as it updates.
///
/// Messages still waiting in the queue trail the transcript, so in a newest-first feed they sit at
/// the very top: what you have already said is nearest the box you said it in.
fn feed_view(state: State, watch: AgentsWatch, answerable: bool) -> AnyView {
    view! {
        {answerable.then(|| reply_bar(state, watch))}
        <div class="adi-chat">
            <For
                each=move || {
                    let turns = watch.peek.get().map(|p| p.turns).unwrap_or_default();
                    // Number the queued messages as they go past: a queued turn's place in the queue
                    // is exactly what unqueueing it sends back.
                    let mut place = 0usize;
                    let mut indexed: Vec<(usize, Option<usize>, AgentTurn)> = turns
                        .into_iter()
                        .enumerate()
                        .map(|(idx, turn)| {
                            let queued = turn.queued.then(|| {
                                let at = place;
                                place += 1;
                                at
                            });
                            (idx, queued, turn)
                        })
                        .collect();
                    // Enumerated first (stable keys), then reversed so the newest renders at the top.
                    indexed.reverse();
                    indexed
                }
                key=|(idx, queued, turn): &(usize, Option<usize>, AgentTurn)| {
                    // A settled turn is keyed by its stable index, so its bubble is never rebuilt. The
                    // live turn folds its growth into the key, so it — and only it — re-renders as it
                    // streams. A queued one folds in its place, so a bubble that moves up the queue —
                    // or leaves it for the transcript proper — is rebuilt rather than left stale.
                    if let Some(place) = queued {
                        format!("{idx}:queued:{place}:{}", turn.text.len())
                    } else if turn.pending {
                        format!(
                            "{idx}:live:{}:{}",
                            turn.text.len(),
                            steps_fingerprint(&turn.steps)
                        )
                    } else {
                        idx.to_string()
                    }
                }
                children=move |(_, queued, turn)| match queued {
                    Some(place) => queued_bubble(state, watch, turn, place),
                    None => chat_bubble(state, turn, answerable),
                }
            />
            {move || chat_placeholder(watch)}
        </div>
    }
    .into_any()
}

/// A message still waiting its turn: your own bubble, dashed and dimmed — said, but not yet asked —
/// carrying an × that takes it back before the agent ever sees it.
fn queued_bubble(state: State, watch: AgentsWatch, turn: AgentTurn, place: usize) -> AnyView {
    view! {
        <div class="adi-chat__turn adi-chat__turn--user adi-chat__turn--queued">
            <div class="adi-chat__role">
                "you"
                <span class="adi-chat__queued">" · queued"</span>
                <button class="adi-chat__unqueue" type="button" title="don't send this after all"
                    on:click=move |_| unqueue_message(state, watch, place)>"\u{2715}"</button>
            </div>
            {crate::markdown::render(&turn.text)}
        </div>
    }
    .into_any()
}

/// The placeholder shown inside the (persistent) chat container while the transcript is still empty —
/// before the first turn lands, or for a finished run that produced nothing. Renders nothing once any
/// turn exists, so it never sits among the bubbles.
fn chat_placeholder(watch: AgentsWatch) -> Option<AnyView> {
    let peek = watch.peek.get();
    if peek.as_ref().is_some_and(|p| !p.turns.is_empty()) {
        return None;
    }
    let msg = match peek {
        None => "Loading…",
        Some(p) if p.running => "Working…",
        Some(_) => "No output.",
    };
    Some(view! { <div class="adi-chat__empty">{msg}</div> }.into_any())
}

/// A cheap fingerprint of a still-streaming turn's activity: how many steps there are, each one's
/// status, and how much detail has landed. Folded into the [`For`] key so a tool flipping ⟳ → ✓ —
/// which leaves the step *count* unchanged — still re-renders the live turn.
fn steps_fingerprint(steps: &[AgentStep]) -> String {
    let mut out = String::with_capacity(steps.len() * 6);
    for s in steps {
        let (mark, len) = match s {
            AgentStep::Message { text } => ('m', text.len()),
            AgentStep::Thinking { text } => ('t', text.len()),
            AgentStep::Tool { status, output, .. } => {
                let mark = match status {
                    AgentToolStatus::Running => 'r',
                    AgentToolStatus::Ok => 'o',
                    AgentToolStatus::Error => 'e',
                };
                (mark, output.len())
            }
        };
        out.push(mark);
        out.push_str(&len.to_string());
        out.push('.');
    }
    out
}

/// One transcript turn, laid out **newest-first** down the feed: the turn's final message on top
/// (with the role label and, for an assistant turn, its metrics footer), then its timeline in
/// reverse — the tool calls it ran, the message it wrote before those, the tools before *that*, and
/// so on. So a turn reads as `message → toolcall ×N → message → toolcall ×N`, with the most recent
/// thing at the top and every mid-turn message left where it was written rather than merged into the
/// answer. Nothing is hidden behind a disclosure. The still-streaming answer is tagged and, while it
/// has no body yet, shows a typing ellipsis.
fn chat_bubble(state: State, turn: AgentTurn, answerable: bool) -> AnyView {
    let is_user = turn.role == "user";
    // Only an assistant turn on a conversation backend may carry an `adi-form` block to render.
    let forms = answerable && !is_user;
    let pending = turn.pending;
    let errored = turn.metrics.as_ref().is_some_and(|m| m.is_error);
    let has_body = !turn.text.trim().is_empty() || !turn.steps.is_empty();
    let text = if pending && !has_body {
        "\u{2026}".to_string()
    } else {
        turn.text
    };
    let steps = turn.steps;
    let metrics = turn.metrics;
    let turn_class = if is_user {
        "adi-chat__turn adi-chat__turn--user"
    } else {
        "adi-chat__turn adi-chat__turn--agent"
    };
    let who = if is_user { "you" } else { "agent" };

    // The turn's final message comes first (on top); the text renders as Markdown, and the metrics
    // footer rides with it.
    let message = view! {
        <div class=turn_class data-error=errored.then_some("1")>
            <div class="adi-chat__role">
                {who}
                {pending.then(|| view! { <span class="adi-chat__typing">" · answering…"</span> })}
            </div>
            {(!text.trim().is_empty()).then(|| super::emitted_form::render_message(state, &text, forms))}
            // A settled answer with nothing in it at all — most often a turn stopped before it spoke.
            // Say so, rather than leaving an empty box the reader has to interpret.
            {(!is_user && !pending && !has_body).then(|| view! {
                <div class="adi-chat__none">"no answer"</div>
            })}
            {metrics.map(metrics_view)}
        </div>
    };

    view! {
        {message}
        {timeline_views(state, steps)}
    }
    .into_any()
}

/// The turn's timeline below its final message, in reverse (newest first). A run of tool/thinking
/// steps becomes one stack of rows; a message the agent wrote mid-turn breaks that stack and renders
/// as its own bubble — which is what makes the feed alternate `message → toolcall ×N → message`
/// instead of showing one merged wall of text above an undifferentiated pile of tool calls.
fn timeline_views(state: State, steps: Vec<AgentStep>) -> Vec<AnyView> {
    let mut out: Vec<AnyView> = Vec::new();
    let mut run: Vec<AnyView> = Vec::new();
    for step in steps.into_iter().rev() {
        match step {
            AgentStep::Message { text } => {
                flush_steps(&mut run, &mut out);
                out.push(mid_turn_message(state, &text));
            }
            other => run.push(step_bubble(other)),
        }
    }
    flush_steps(&mut run, &mut out);
    out
}

/// Close off the current run of tool/thinking rows into one stack, if it has any.
fn flush_steps(run: &mut Vec<AnyView>, out: &mut Vec<AnyView>) {
    if run.is_empty() {
        return;
    }
    let rows = std::mem::take(run);
    out.push(view! { <div class="adi-chat__steps">{rows}</div> }.into_any());
}

/// Something the agent said *during* the turn, between tool calls: a normal agent bubble, dimmed a
/// touch so the turn's final answer still leads. Rendered as plain Markdown — an emitted form is an
/// ask, and only the turn's final message is the live one, so a superseded mid-turn form must not
/// come back as a second interactive card.
fn mid_turn_message(state: State, text: &str) -> AnyView {
    view! {
        <div class="adi-chat__turn adi-chat__turn--agent adi-chat__turn--mid">
            {super::emitted_form::render_message(state, text, false)}
        </div>
    }
    .into_any()
}

/// One activity step as its own row beneath the message — a tool call or a thinking block.
fn step_bubble(step: AgentStep) -> AnyView {
    view! {
        <div class="adi-chat__turn adi-chat__turn--agent adi-chat__turn--step">
            {step_row(step)}
        </div>
    }
    .into_any()
}

/// One activity row. A `<details>` so its arguments/output (or reasoning) expand in place — no JS.
/// A [`AgentStep::Message`] never reaches here — [`timeline_views`] renders it as its own bubble —
/// but it is handled rather than dropped so a message can never silently vanish from the feed.
fn step_row(step: AgentStep) -> AnyView {
    match step {
        AgentStep::Message { text } => crate::markdown::render(&text),
        AgentStep::Thinking { text } => view! {
            <details class="adi-step adi-step--thinking">
                <summary class="adi-step__head">
                    <span class="adi-step__icon">"💭"</span>
                    <span class="adi-step__name">"thinking"</span>
                </summary>
                <pre class="adi-step__detail">{text}</pre>
            </details>
        }
        .into_any(),
        AgentStep::Tool {
            name,
            input,
            status,
            output,
        } => {
            let (badge, status_attr) = match status {
                AgentToolStatus::Running => ("\u{27F3}", "running"),
                AgentToolStatus::Ok => ("\u{2713}", "ok"),
                AgentToolStatus::Error => ("\u{2717}", "error"),
            };
            let arg = truncate_task(&input);
            let detail = match (input.trim().is_empty(), output.trim().is_empty()) {
                (true, true) => String::new(),
                (false, true) => input,
                (true, false) => output,
                (false, false) => format!("{input}\n\u{2500}\u{2500}\u{2500}\n{output}"),
            };
            view! {
                <details class="adi-step adi-step--tool" data-status=status_attr>
                    <summary class="adi-step__head">
                        <span class="adi-step__icon">"🔧"</span>
                        <span class="adi-step__name adi-mono">{name}</span>
                        {(!arg.is_empty()).then(|| view! {
                            <span class="adi-step__arg adi-mono">{arg}</span>
                        })}
                        <span class="adi-step__status">{badge}</span>
                    </summary>
                    {(!detail.is_empty()).then(|| view! {
                        <pre class="adi-step__detail adi-mono">{detail}</pre>
                    })}
                </details>
            }
            .into_any()
        }
    }
}

/// The metrics footer of a settled turn: tokens · cost · duration, plus any blocked-tool warning.
fn metrics_view(m: AgentTurnMetrics) -> AnyView {
    let mut chips: Vec<String> = Vec::new();
    let tokens = m.input_tokens.unwrap_or(0) + m.output_tokens.unwrap_or(0);
    if tokens > 0 {
        chips.push(format!("{} tok", fmt_count(tokens)));
    }
    if let Some(micro) = m.cost_micro_usd.filter(|c| *c > 0) {
        chips.push(fmt_cost(micro));
    }
    if let Some(ms) = m.duration_ms.filter(|d| *d > 0) {
        chips.push(fmt_duration(ms));
    }
    let denied = m.permission_denials.len();
    if chips.is_empty() && denied == 0 {
        return ().into_any();
    }
    view! {
        <div class="adi-chat__metrics adi-mono">
            {chips.join(" \u{00B7} ")}
            {(denied > 0).then(|| view! {
                <span class="adi-chat__denied">{format!(" \u{00B7} \u{26A0} {denied} blocked")}</span>
            })}
        </div>
    }
    .into_any()
}

/// A compact count: `1.2k` past a thousand, else the plain number.
fn fmt_count(n: u64) -> String {
    if n >= 1000 {
        format!("{:.1}k", n as f64 / 1000.0)
    } else {
        n.to_string()
    }
}

/// Micro-dollars as a short dollar amount (`$0.0195`), trailing zeros trimmed.
fn fmt_cost(micro: u64) -> String {
    let dollars = micro as f64 / 1_000_000.0;
    let s = format!("{dollars:.4}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    format!("${s}")
}

/// Milliseconds as `850ms` under a second, else `8.6s`.
fn fmt_duration(ms: u64) -> String {
    if ms < 1000 {
        format!("{ms}ms")
    } else {
        format!("{:.1}s", ms as f64 / 1000.0)
    }
}

// ---- the chat composers -------------------------------------------------------------------
// A message to an agent is prose, and usually Markdown: a list, a fenced block, a paragraph that
// wants a blank line after it. Typed into a single-line field those all collapse onto one line and
// arrive as something the agent reads differently from what was meant. So both composers — the
// new-run task and the reply — are textareas that grow with what is written.

/// What the composers say about themselves on hover. Enter sending (rather than breaking the line)
/// is the convention every chat box uses, and the one the muscle memory here already had; the
/// modifier is how you earn a newline instead.
const COMPOSER_HINT: &str = "Enter sends · Shift-Enter for a new line";

/// Whether this keydown means *send*. A bare Enter does; every modified Enter writes a newline, so
/// a multi-line message can be composed without ever submitting half of it.
///
/// `is_composing` is the one that isn't cosmetic: while an IME is open, Enter accepts the candidate
/// word being typed and must not be read as "send" — the same keystroke means two different things
/// depending on state the keyboard event alone doesn't show.
fn sends(ev: &leptos::ev::KeyboardEvent) -> bool {
    ev.key() == "Enter"
        && !ev.shift_key()
        && !ev.ctrl_key()
        && !ev.meta_key()
        && !ev.alt_key()
        && !ev.is_composing()
}

/// Grow (or shrink) the composer to exactly the height of what it holds.
///
/// Measured rather than counted: a row per `\n` would leave a long pasted line — a prompt, a URL —
/// standing one row tall and scrolling inside itself, since wrapping makes lines the box's own
/// width decides. `scrollHeight` is that answer already computed by the browser. It only reports a
/// *smaller* height once the box stops holding itself open, hence the collapse to `auto` first. The
/// cap lives in CSS (`max-height`), so an inline height past it is simply ignored and the composer
/// scrolls instead of swallowing the transcript.
fn autosize(area: &web_sys::HtmlTextAreaElement) {
    // Spelled out through `HtmlElement`: Leptos has a `style` of its own in scope for element
    // types, and plain `area.style()` would resolve to that one.
    let style = web_sys::HtmlElement::style(area.as_ref());
    // An empty box is measured by its `rows`, never by `scrollHeight` — for an empty textarea the
    // browser reports the height of the *placeholder*, and these placeholders are a sentence long,
    // so a sent-and-cleared composer would settle one row taller than an untouched one.
    if area.value().is_empty() {
        let _ = style.remove_property("height");
        return;
    }
    let _ = style.set_property("height", "auto");
    let _ = style.set_property("height", &format!("{}px", area.scroll_height()));
}

/// Empty the composer after a message goes, and let it fall back to one row.
///
/// The DOM value is cleared here rather than left to the signal: the height has to be re-measured
/// from an *already empty* box, and the framework writes the emptied signal back on its own clock —
/// so measuring before that lands would just re-measure the message that was already sent.
fn clear_composer(area: NodeRef<leptos::html::Textarea>) {
    if let Some(el) = area.get_untracked() {
        el.set_value("");
        autosize(&el);
    }
}

/// The reply box: says the next thing into the selected conversation. It never locks you out while
/// the agent is working — one turn runs at a time, so a message sent mid-answer is *queued* and
/// starts when the current turn lands (the button says so). Beside it, while an answer is streaming,
/// a Stop that cuts the turn short and drops anything lined up behind it.
fn reply_bar(state: State, watch: AgentsWatch) -> impl IntoView {
    let answering = move || watch.peek.get().is_some_and(|p| p.running);
    let area: NodeRef<leptos::html::Textarea> = NodeRef::new();
    let send = move || {
        let message = watch.reply.get_untracked();
        if message.trim().is_empty() {
            return;
        }
        watch.reply.set(String::new());
        clear_composer(area);
        send_reply(state, watch, with_context(watch, message));
    };
    view! {
        <form class="adi-form adi-chat__replybar"
            on:submit=move |ev| {
                ev.prevent_default();
                send();
            }>
            <textarea class="adi-input adi-input--wide adi-input--composer adi-mono"
                node_ref=area rows="1" autocomplete="off" title=COMPOSER_HINT
                placeholder=move || if answering() { "queue the next message…" } else { "reply…" }
                prop:value=move || watch.reply.get()
                on:keydown=move |ev| if sends(&ev) { ev.prevent_default(); send(); }
                on:input=move |ev| {
                    let el = event_target::<web_sys::HtmlTextAreaElement>(&ev);
                    watch.reply.set(el.value());
                    autosize(&el);
                } />
            <button class="adi-btn adi-btn--primary" type="submit"
                title=move || if answering() {
                    "send this once the current answer lands"
                } else {
                    "send this now"
                }
                prop:disabled=move || watch.reply.get().trim().is_empty()>
                {move || if answering() { "Queue" } else { "Send" }}
            </button>
            {move || answering().then(|| view! {
                <button class="adi-btn adi-chat__stop" type="button"
                    title="stop this answer, and drop anything queued behind it"
                    on:click=move |_| stop_one_run(
                        state, watch, watch.run_id.get_untracked().unwrap_or_default(),
                    )>"\u{25A0} Stop"</button>
            })}
        </form>
    }
}

/// Say the reply box's message into the conversation, applying the returned snapshot at once (so the
/// message — asked or queued — and any streaming answer appear immediately) and resuming the poll.
/// Errors go to flash.
fn send_reply(state: State, watch: AgentsWatch, message: String) {
    let Some(name) = watch.name.get_untracked() else {
        return;
    };
    let Some(run_id) = watch.run_id.get_untracked() else {
        return;
    };
    spawn_local(async move {
        match fetch::reply_to_run(name.clone(), run_id.clone(), message).await {
            Ok(peek) => {
                // Only apply if the view is still on this same conversation.
                if watch.name.get_untracked().as_deref() == Some(name.as_str())
                    && watch.run_id.get_untracked().as_deref() == Some(run_id.as_str())
                {
                    watch.peek.set(Some(peek));
                    poll_watch(watch);
                }
            }
            Err(e) => state.flash.set(Some(Flash::err(e))),
        }
    });
}

/// Take back a message still waiting in the open conversation's queue, before it is ever asked. The
/// answer is a fresh snapshot, so the bubble disappears without waiting for the next poll.
fn unqueue_message(state: State, watch: AgentsWatch, index: usize) {
    let Some(name) = watch.name.get_untracked() else {
        return;
    };
    let Some(run_id) = watch.run_id.get_untracked() else {
        return;
    };
    spawn_local(async move {
        match fetch::unqueue_from_run(name.clone(), run_id.clone(), index).await {
            Ok(peek) => {
                if watch.name.get_untracked().as_deref() == Some(name.as_str())
                    && watch.run_id.get_untracked().as_deref() == Some(run_id.as_str())
                {
                    watch.peek.set(Some(peek));
                }
            }
            Err(e) => state.flash.set(Some(Flash::err(e))),
        }
    });
}

/// The title bar's `tail -f <log>` hint, shown once a snapshot has landed — the human-runnable
/// equivalent of what the console below shows.
fn run_log_status(watch: AgentsWatch) -> Option<AnyView> {
    let attach = watch.peek.get().map(|p| p.attach).unwrap_or_default();
    (!attach.is_empty()).then(|| {
        view! { <code class="adi-runlog__cmd adi-mono">{attach}</code> }.into_any()
    })
}

/// One run row in the history table: when it started, status, its task (or the conversation's first
/// message), and Open / Stop. For an answerable conversation the status reads "answering" while a
/// turn is in flight and "idle" when it is waiting for the next message.
fn run_row(
    state: State,
    watch: AgentsWatch,
    r: &AgentRunInfo,
    shown: &[&'static str],
    is_selected: bool,
    answerable: bool,
) -> AnyView {
    let run_id = r.run_id.clone();
    let running = r.running;
    let view_id = run_id.clone();
    let stop_id = run_id.clone();
    let del_id = run_id.clone();
    // Truncated, because it goes into a confirm dialog — not the whole first message.
    let del_title = truncate_task(&r.message);
    // Built by hand rather than through `body_row`: the selected row tints itself, so its `<tr>`
    // carries a style the shared helper has no business knowing about.
    let row_style = if is_selected {
        "background:var(--surface-2)"
    } else {
        ""
    };
    // The action toggles this row's detail drawer: Open reveals the chat/log beneath it, and while
    // open it reads "● Open" and a second click collapses it. Only the drawer carries an explicit
    // "Close", so there is one thing labelled Close, not two.
    let open_verb = if answerable { "Open" } else { "View" };
    let view_label = if is_selected {
        format!("● {open_verb}")
    } else {
        open_verb.to_string()
    };
    let stop_title = if answerable {
        "stop the current answer"
    } else {
        "stop this run"
    };
    let delete_title = if answerable {
        "delete this chat and its transcript"
    } else {
        "delete this run and its log"
    };
    let cells: Vec<AnyView> = shown
        .iter()
        .map(|col| run_cell(col, r, answerable))
        .collect();
    view! {
        <tr style=row_style>
            {cells}
            <td class="adi-table__actions">
                <button class="adi-btn adi-btn--link"
                    on:click=move |_| if is_selected {
                        close_run_view(watch);
                    } else {
                        select_run(watch, view_id.clone());
                    }>{view_label}</button>
                " "
                {running.then(|| { let stop_id = stop_id.clone(); view! {
                    <button class="adi-btn adi-btn--link" title=stop_title
                        on:click=move |_| stop_one_run(state, watch, stop_id.clone())>"Stop"</button>
                    " "
                }})}
                <button class="adi-btn adi-btn--link adi-btn--danger" title=delete_title
                    on:click=move |_| delete_one_run(
                        state, watch, del_id.clone(), del_title.clone(),
                    )>"Delete"</button>
            </td>
        </tr>
    }
    .into_any()
}

/// The composer that starts a new run/conversation: a message input, an optional directory to run
/// it in, and a Start/Run button. A message is required — the button stays disabled (and submit
/// no-ops) until one is typed. Submitting launches it and opens its detail: a streaming log for a
/// one-shot run, or the chat for an answerable conversation you then reply to.
///
/// The directory box is the answer to "this agent, but against *that* target". Left blank — the
/// normal case — the run starts where the agent is defined to. It applies to the launch only; a
/// conversation then keeps the directory it started in for every reply.
fn run_bar(state: State, watch: AgentsWatch) -> impl IntoView {
    let area: NodeRef<leptos::html::Textarea> = NodeRef::new();
    let placeholder = move || {
        if watch.answerable.get() {
            "start a conversation — your first message (required)"
        } else {
            "task for a new run (required) — e.g. review the latest commit and summarize it"
        }
    };
    let send = move || {
        let Some(name) = watch.name.get_untracked() else {
            return;
        };
        let message = watch.input.get_untracked();
        if message.trim().is_empty() {
            return;
        }
        let dir = watch.run_dir.get_untracked();
        let dir = dir.trim();
        let working_dir = (!dir.is_empty()).then(|| dir.to_string());
        watch.input.set(String::new());
        clear_composer(area);
        launch_agent(state, watch, name, with_context(watch, message), working_dir);
    };
    view! {
        <form class="adi-form"
            on:submit=move |ev| {
                ev.prevent_default();
                send();
            }>
            <textarea class="adi-input adi-input--wide adi-input--composer adi-mono"
                node_ref=area rows="1" autocomplete="off" title=COMPOSER_HINT
                placeholder=placeholder
                prop:value=move || watch.input.get()
                on:keydown=move |ev| if sends(&ev) { ev.prevent_default(); send(); }
                on:input=move |ev| {
                    let el = event_target::<web_sys::HtmlTextAreaElement>(&ev);
                    watch.input.set(el.value());
                    autosize(&el);
                } />
            <input class="adi-input adi-mono" autocomplete="off"
                title="Run this launch in a directory other than the agent's own. Blank = as defined."
                placeholder="run here (optional) — /path/to/target"
                prop:value=move || watch.run_dir.get()
                on:input=move |ev| watch.run_dir.set(event_target_value(&ev)) />
            <button class="adi-btn adi-btn--primary" type="submit"
                prop:disabled=move || watch.input.get().trim().is_empty()>
                {move || if watch.answerable.get() { "▶ Start" } else { "▶ Run" }}
            </button>
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

// ---- chat home (`/` once the root agent exists) ------------------------------------------------
// The app *is* the chat: the watched agent's sessions on the left, its conversation in the centre,
// the dashboards list on the right. Built on the same live-view machinery as the Agents page, so a
// pty agent shows a live terminal + input and a headless one shows its conversations.
//
// Which agent that is, is the user's to pick: the left rail's picker names it, and every other
// agent's sessions are listed under it — so one screen reaches all of them, not only the root.

/// The three-pane chat home. `watch` is expected to already point at an agent — the root one, until
/// the picker moves it (the caller sets the name + interactive flag and drives the 1s poll).
/// `state.agents` feeds the picker, `state.all_chats` the other agents' sessions, and
/// `state.dashboards` the right rail.
pub(crate) fn chat_home_view(state: State, watch: AgentsWatch) -> AnyView {
    view! {
        <div class="adi-chome">
            <aside class="adi-chome__side adi-chome__side--left">
                <div class="adi-chome__side-head">
                    <span class="adi-chome__side-title">"Sessions"</span>
                    <span class="adi-spacer"></span>
                    {move || chat_new_button(state, watch)}
                </div>
                <div class="adi-chome__pickrow">
                    {move || chat_agent_picker(state, watch)}
                </div>
                <div class="adi-chome__side-body">
                    {move || chat_sessions(state, watch)}
                    {move || chat_other_sessions(state, watch)}
                </div>
            </aside>

            <main class="adi-chome__chat">
                {move || chat_center(state, watch)}
            </main>

            <aside class="adi-chome__side adi-chome__side--right">
                <div class="adi-chome__side-head">
                    <span class="adi-chome__side-title">"Dashboards"</span>
                    <span class="adi-spacer"></span>
                    <a class="adi-chome__side-link" href="/extended/dashboards">"Manage"</a>
                </div>
                <div class="adi-chome__side-body">
                    {move || chat_dashboards(state)}
                </div>
            </aside>
        </div>
    }
    .into_any()
}

/// The rail's agent picker — which agent this screen is a chat with. Every registered agent is an
/// option (the root agent first, then the list's own name order), and one that is live right now
/// carries a dot. Choosing one repoints the whole screen at once: the sessions below, the
/// conversation in the centre, and the agent "+ New" starts a session on.
fn chat_agent_picker(state: State, watch: AgentsWatch) -> AnyView {
    let Some(list) = state.agents.get() else {
        return view! { <span class="adi-chome__pickhint">"Loading agents…"</span> }.into_any();
    };
    let mut agents = list.agents;
    if agents.is_empty() {
        return view! { <span class="adi-chome__pickhint">"No agents yet"</span> }.into_any();
    }
    // The root agent leads — it's the one the app is set up around. A stable sort, so the rest keep
    // the order the list arrived in (by name) behind it.
    agents.sort_by_key(|a| a.name != ROOT_AGENT);
    let current = watch.name.get().unwrap_or_default();
    view! {
        <select class="adi-input adi-chome__pick" title="which agent this chat runs on"
            prop:value=current.clone()
            on:change=move |ev| switch_agent(state, watch, event_target_value(&ev))>
            {agents.into_iter().map(|a| {
                let selected = a.name == current;
                // An option carries no markup, so the live dot rides in its text — worth spotting
                // in a collapsed select, which is all you see of the other agents until you open it.
                let label = if a.running {
                    format!("\u{25CF} {}", a.name)
                } else {
                    a.name.clone()
                };
                view! { <option value=a.name selected=selected>{label}</option> }
            }).collect::<Vec<_>>()}
        </select>
    }
    .into_any()
}

/// Switch the chat home to another agent. Everything on this screen reads `watch`, so pointing it at
/// `name` moves the rail, the centre pane and "+ New" together. Whether the agent is interactive
/// comes from the loaded list; a name that isn't in it is left alone rather than watched blind.
fn switch_agent(state: State, watch: AgentsWatch, name: String) {
    if name.is_empty() || watch.name.get_untracked().as_deref() == Some(name.as_str()) {
        return;
    }
    let Some(interactive) = agent_interactive(state, &name) else {
        return;
    };
    point_watch(watch, name, interactive);
}

/// Whether `name` is a pty (interactive) agent, or `None` when the loaded agents list holds no such
/// agent — the caller's cue not to point the view at it.
fn agent_interactive(state: State, name: &str) -> Option<bool> {
    let list = state.agents.get_untracked()?;
    list.agents
        .iter()
        .find(|a| a.name == name)
        .map(|a| a.executor == "pty")
}

/// The rail's **other agents** section: every *other* agent's sessions, grouped under its name, so
/// one screen reaches all of them. A headless agent contributes its conversations; a pty agent
/// contributes its live session, and only while it runs — it keeps no history, so an ended one has
/// nothing to list (its picker entry is still the way back to it). These rows only *select*: a click
/// switches the screen to that agent, which is where its per-chat delete lives.
fn chat_other_sessions(state: State, watch: AgentsWatch) -> AnyView {
    let Some(all) = state.all_chats.get() else {
        return ().into_any();
    };
    let current = watch.name.get().unwrap_or_default();
    // A pty agent's run history is empty either way; the agents list is what says it's live now.
    let live: std::collections::HashSet<String> = state
        .agents
        .get()
        .map(|s| {
            s.agents
                .into_iter()
                .filter(|a| a.running)
                .map(|a| a.name)
                .collect()
        })
        .unwrap_or_default();

    let mut out: Vec<AnyView> = Vec::new();
    for ar in all.agents {
        if ar.name == current {
            continue;
        }
        if ar.interactive {
            if !live.contains(&ar.name) {
                continue;
            }
            out.push(chat_group(&ar.name));
            out.push(chat_other_live(watch, &ar.name));
        } else {
            if ar.runs.is_empty() {
                continue;
            }
            out.push(chat_group(&ar.name));
            for r in &ar.runs {
                out.push(chat_other_row(watch, &ar.name, r));
            }
        }
    }
    if out.is_empty() {
        return ().into_any();
    }
    view! {
        <div class="adi-chome__divider">"Other agents"</div>
        {out}
    }
    .into_any()
}

/// One of another agent's conversations: the same strip as the selected agent's own sessions, minus
/// the delete — a click here switches the screen to that chat rather than acting on it in place.
fn chat_other_row(watch: AgentsWatch, agent: &str, r: &AgentRunInfo) -> AnyView {
    let title = truncate_task(&r.message);
    let title = if title.trim().is_empty() { "New chat".to_string() } else { title };
    let when = run_age(r.started_at);
    let dot = if r.running { "adi-chome__dot adi-chome__dot--on" } else { "adi-chome__dot" };
    let hint = format!("open this chat with {agent}");
    let (name, rid) = (agent.to_string(), r.run_id.clone());
    view! {
        <button class="adi-chome__session adi-chome__session--other" type="button" title=hint
            on:click=move |_| point_conversation(watch, name.clone(), rid.clone(), false)>
            <span class=dot></span>
            <span class="adi-chome__session-main">
                <span class="adi-chome__session-title">{title}</span>
                <span class="adi-chome__session-when">{when}</span>
            </span>
        </button>
    }
    .into_any()
}

/// Another agent's live pty session: one row, since a terminal agent's session *is* its history.
fn chat_other_live(watch: AgentsWatch, agent: &str) -> AnyView {
    let hint = format!("watch {agent}'s live session");
    let name = agent.to_string();
    view! {
        <button class="adi-chome__session adi-chome__session--other" type="button" title=hint
            on:click=move |_| point_watch(watch, name.clone(), true)>
            <span class="adi-chome__dot adi-chome__dot--on"></span>
            <span class="adi-chome__session-main">
                <span class="adi-chome__session-title">"Live session"</span>
                <span class="adi-chome__session-when">"interactive terminal"</span>
            </span>
        </button>
    }
    .into_any()
}

/// The sessions rail's "New" action: for a pty agent it (re)starts the live session; for a headless
/// one it clears the selection so the centre shows the new-conversation composer. Either way it acts
/// on the *picked* agent — the one the rail's picker names.
fn chat_new_button(state: State, watch: AgentsWatch) -> AnyView {
    if watch.interactive.get() {
        let name = watch.name.get().unwrap_or_default();
        view! {
            <button class="adi-btn adi-btn--ghost adi-chome__new" type="button"
                title="start a fresh session"
                on:click=move |_| run_now(state, name.clone())>"+ New"</button>
        }
        .into_any()
    } else {
        view! {
            <button class="adi-btn adi-btn--ghost adi-chome__new" type="button"
                title="start a new chat"
                on:click=move |_| close_run_view(watch)>"+ New"</button>
        }
        .into_any()
    }
}

/// The sessions rail's body for the *picked* agent: a pty agent has a single live session; a
/// headless one lists its conversations (newest first), each selectable into the centre. Every other
/// agent's sessions follow below it — see [`chat_other_sessions`].
fn chat_sessions(state: State, watch: AgentsWatch) -> AnyView {
    if watch.interactive.get() {
        let running = watch.peek.get().is_some_and(|p| p.running);
        let dot = if running { "adi-chome__dot adi-chome__dot--on" } else { "adi-chome__dot" };
        let label = if running { "Live session" } else { "No live session" };
        return view! {
            <div class="adi-chome__session is-active">
                <span class=dot></span>
                <span class="adi-chome__session-main">
                    <span class="adi-chome__session-title">{label}</span>
                    <span class="adi-chome__session-when">"interactive terminal"</span>
                </span>
            </div>
        }
        .into_any();
    }
    let runs = watch.runs.get();
    if runs.is_empty() {
        return view! {
            <div class="adi-chome__empty">
                "No chats with this agent yet — press New to start one."
            </div>
        }
        .into_any();
    }
    let selected = watch.run_id.get();
    runs.into_iter()
        .map(|r| {
            let is_sel = selected.as_deref() == Some(r.run_id.as_str());
            let title = truncate_task(&r.message);
            let title = if title.trim().is_empty() { "New chat".to_string() } else { title };
            let when = run_age(r.started_at);
            let dot = if r.running { "adi-chome__dot adi-chome__dot--on" } else { "adi-chome__dot" };
            let cls = if is_sel { "adi-chome__session is-active" } else { "adi-chome__session" };
            let rid = r.run_id.clone();
            let del_id = r.run_id.clone();
            let del_title = title.clone();
            // A row, not a bare button: the strip selects the chat and a delete rides at its right
            // edge — and one button may not nest inside another, so they are siblings.
            view! {
                <div class="adi-chome__sessionrow">
                    <button class=cls type="button" on:click=move |_| select_run(watch, rid.clone())>
                        <span class=dot></span>
                        <span class="adi-chome__session-main">
                            <span class="adi-chome__session-title">{title}</span>
                            <span class="adi-chome__session-when">{when}</span>
                        </span>
                    </button>
                    <button class="adi-chome__session-del" type="button"
                        title="delete this chat and its transcript"
                        on:click=move |_| delete_one_run(
                            state, watch, del_id.clone(), del_title.clone(),
                        )>"\u{2715}"</button>
                </div>
            }
        })
        .collect::<Vec<_>>()
        .into_any()
}

/// The centre pane: a pty agent's live terminal (+ input), or a headless agent's selected
/// conversation (or the new-conversation composer when nothing is selected).
fn chat_center(state: State, watch: AgentsWatch) -> AnyView {
    let Some(name) = watch.name.get() else {
        return view! { <div class="adi-chome__center-empty">"Connecting…"</div> }.into_any();
    };
    if watch.interactive.get() {
        chat_center_pty(state, watch, name)
    } else {
        chat_center_headless(state, watch, name)
    }
}

/// The pty centre: the live pane while a session runs, with the same send bar as the Agents page;
/// otherwise a Start affordance that launches the session.
fn chat_center_pty(state: State, watch: AgentsWatch, name: String) -> AnyView {
    view! {
        <div class="adi-chome__chatwrap">
            <div class="adi-chome__chatbody">
                {move || match watch.peek.get() {
                    Some(p) if p.running => {
                        view! { <pre class="adi-term adi-chome__term">{p.output}</pre> }.into_any()
                    }
                    other => {
                        let ended = other.is_some();
                        let msg = if ended {
                            "The session has ended."
                        } else {
                            "No live session yet."
                        };
                        let name = name.clone();
                        view! {
                            <div class="adi-chome__center-empty">
                                <p>{msg}</p>
                                <button class="adi-btn adi-btn--primary" type="button"
                                    on:click=move |_| run_now(state, name.clone())>
                                    "\u{25B6} Start session"
                                </button>
                            </div>
                        }
                        .into_any()
                    }
                }}
            </div>
            {move || watch.peek.get().is_some_and(|p| p.running).then(|| send_bar(state, watch))}
        </div>
    }
    .into_any()
}

/// The headless centre: the selected conversation's transcript (+ reply), or a composer to start a
/// new one when no session is selected. The composer names `agent` — with several agents to pick
/// between, which one a new chat would go to is the thing worth saying out loud.
fn chat_center_headless(state: State, watch: AgentsWatch, agent: String) -> AnyView {
    view! {
        <div class="adi-chome__chatwrap">
            {move || match watch.run_id.get() {
                Some(_) => view! {
                    <div class="adi-chome__feed">{feed_view(state, watch, watch.answerable.get())}</div>
                }
                .into_any(),
                None => view! {
                    <div class="adi-chome__compose">
                        <div class="adi-chome__compose-intro">
                            <h2 class="adi-chome__compose-title">
                                "Start a chat with "
                                <span class="adi-chome__compose-agent">{agent.clone()}</span>
                            </h2>
                            <p class="adi-chome__compose-sub">
                                "Ask it to set something up — or pick a past session on the left."
                            </p>
                        </div>
                        {run_bar(state, watch)}
                    </div>
                }
                .into_any(),
            }}
        </div>
    }
    .into_any()
}

/// The dashboards rail: every live dashboard, **grouped by the project it's filed under** (a
/// header per project that has at least one, then an "Ungrouped" bucket), each a link to open it
/// when its frontend is up. Project names come from `state.projects`; an unknown/archived project
/// id folds into Ungrouped.
fn chat_dashboards(state: State) -> AnyView {
    let Some(ds) = state.dashboards.get() else {
        return view! { <div class="adi-chome__empty">"Loading…"</div> }.into_any();
    };
    let live: Vec<Dashboard> = ds.dashboards.into_iter().filter(|d| !d.is_archived()).collect();
    if live.is_empty() {
        return view! {
            <div class="adi-chome__empty">"No dashboards yet — ask your agent to build one."</div>
        }
        .into_any();
    }
    let projects = state.projects.get().map(|p| p.projects).unwrap_or_default();

    let mut out: Vec<AnyView> = Vec::new();
    let mut placed: std::collections::HashSet<String> = std::collections::HashSet::new();
    // One group per project (in the projects list's order) that owns at least one dashboard.
    for p in projects.iter().filter(|p| !p.is_archived()) {
        let items: Vec<&Dashboard> = live
            .iter()
            .filter(|d| d.project.as_deref() == Some(p.id.as_str()))
            .collect();
        if items.is_empty() {
            continue;
        }
        out.push(chat_group(&p.name));
        for d in items {
            placed.insert(d.id.clone());
            out.push(chat_dash_item(d));
        }
    }
    // Whatever's left — unfiled, or filed under a project that no longer exists — trails as one
    // Ungrouped bucket. Only labelled when it sits alongside real groups.
    let rest: Vec<&Dashboard> = live.iter().filter(|d| !placed.contains(&d.id)).collect();
    if !rest.is_empty() {
        if !out.is_empty() {
            out.push(chat_group("Ungrouped"));
        }
        for d in rest {
            out.push(chat_dash_item(d));
        }
    }
    out.into_any()
}

/// A header row inside a rail: the project a group of dashboards is filed under, or the agent a
/// group of sessions belongs to.
fn chat_group(name: &str) -> AnyView {
    view! { <div class="adi-chome__group">{name.to_string()}</div> }.into_any()
}

/// One dashboard row in the rail: a link to its running frontend, or a dimmed row when it's down.
fn chat_dash_item(d: &Dashboard) -> AnyView {
    let name = d.name.clone();
    match d.frontend_port {
        Some(port) if d.frontend_running => view! {
            <a class="adi-chome__dash" href=format!("http://127.0.0.1:{port}")
                target="_blank" rel="noreferrer" title=d.name.clone()>
                <span class="adi-chome__dot adi-chome__dot--on"></span>
                <span class="adi-chome__dash-name">{name}</span>
                <span class="adi-chome__dash-open">"\u{2197}"</span>
            </a>
        }
        .into_any(),
        _ => view! {
            <div class="adi-chome__dash is-off" title="not running">
                <span class="adi-chome__dot"></span>
                <span class="adi-chome__dash-name">{name}</span>
            </div>
        }
        .into_any(),
    }
}
