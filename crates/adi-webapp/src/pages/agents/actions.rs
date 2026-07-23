//! The Agents page run, stop, and live-view actions.
//!
//! An agent definition is a *template*. For interactive (tmux) backends a Run starts a session you
//! type into and View watches its pane. For headless (`process` / `harness`) backends each Run is an
//! independent run of the agent's settings (a fresh dialog, never continued): every run keeps its
//! own log, several may be live at once, and the live view is a browsable run history plus a task
//! composer — never a shared, overwritten slot.

use adi_webapp_api::types::{
    AgentDto, AgentRunInfo, AgentStep, AgentToolStatus, AgentTurn, AgentTurnMetrics, AgentsState,
};
use leptos::prelude::*;
use wasm_bindgen_futures::spawn_local;

use crate::fetch;
use crate::routing::scroll_top;
use crate::state::{AgentsWatch, Flash, State};
use crate::ui::{apply_mutation, data_table, placeholder_row};

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
    // Harness backends keep answerable conversations; the run controls read as a chat there.
    let answerable = a.executor == "harness";
    let stop_title = if interactive {
        "kill the tmux session"
    } else if answerable {
        "stop the current answer of every live conversation"
    } else {
        "stop every live run"
    };
    let view_title = if interactive {
        "watch the live tmux session"
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
                    <button class="adi-btn adi-btn--link" title="start an interactive tmux session"
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
    // Reset until the first history poll reports whether this backend keeps answerable conversations.
    watch.answerable.set(false);
    watch.reply.set(String::new());
    watch.interactive.set(interactive);
    watch.name.set(Some(name));
    poll_watch(watch);
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

/// Open a specific conversation from the cross-agent "All chats" index: point the shared live view
/// at its agent and select that run, so its transcript opens in the panel below (and scrolls into
/// view). Interactive agents keep no run history, so `run_id` is only selected when present.
pub(crate) fn open_conversation(
    watch: AgentsWatch,
    name: String,
    run_id: String,
    interactive: bool,
) {
    open_watch(watch, name, interactive);
    if !run_id.is_empty() {
        watch.run_id.set(Some(run_id));
        poll_watch(watch);
    }
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
        Some(tmux_live_view(state, watch, name))
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
            {data_table(&["Agent", "When", "Status", "Conversation", ""],
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
    if state.all_chats.get().is_none() {
        return placeholder_row("5", "Loading…");
    }
    let rows = all_chats_flatten(state, only);
    if rows.is_empty() {
        return placeholder_row("5", "No chats yet — start one from an agent below.");
    }
    rows.into_iter()
        .map(|(agent, answerable, interactive, r)| {
            let when = run_age(r.started_at);
            let status = match (answerable, r.running) {
                (true, true) => "\u{25CF} answering",
                (true, false) => "idle",
                (false, true) => "\u{25CF} running",
                (false, false) => "done",
            };
            let msg_full = r.message.clone();
            let msg_short = truncate_task(&msg_full);
            let (name, run_id) = (agent.clone(), r.run_id.clone());
            view! {
                <tr>
                    <td class="adi-mono">{agent}</td>
                    <td class="adi-muted" style="white-space:nowrap">{when}</td>
                    <td>{status}</td>
                    <td class="adi-mono" title=msg_full>{msg_short}</td>
                    <td class="adi-table__actions">
                        <button class="adi-btn adi-btn--link"
                            on:click=move |_| open_conversation(watch, name.clone(), run_id.clone(), interactive)>
                            "Open"
                        </button>
                    </td>
                </tr>
            }
        })
        .collect::<Vec<_>>()
        .into_any()
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
    let runs = watch.runs.get();
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
    let selected = watch.run_id.get();
    let mut rows: Vec<AnyView> = Vec::with_capacity(runs.len() + 1);
    for r in &runs {
        let is_selected = selected.as_deref() == Some(r.run_id.as_str());
        rows.push(run_row(state, watch, r, selected.as_deref(), answerable));
        // The log / chat opens as a detail row right beneath the run it belongs to.
        if is_selected {
            rows.push(run_detail_row(state, watch, r.run_id.clone(), answerable));
        }
    }
    let headers: &'static [&'static str] = if answerable {
        &["When", "Status", "Conversation", ""]
    } else {
        &["When", "Status", "Task", ""]
    };
    data_table(headers, rows).into_any()
}

/// The expanded log for the selected run: a full-width table row directly under it, holding the run
/// header (id, the `tail -f` hint, a Close) and the inline viewer that follows the tail. Built once
/// per selected run — the log merely growing updates the bound `log` signal in place rather than
/// rebuilding this row, so the follow-scroll is never reset.
fn run_detail_row(state: State, watch: AgentsWatch, run_id: String, answerable: bool) -> AnyView {
    // Conversations read as a chat; one-shot runs as a progress feed of the same shape.
    let title = if answerable { "\u{25A4} Chat" } else { "\u{25A4} Run" };
    view! {
        <tr class="adi-runlog">
            <td class="adi-runlog__cell" colspan="4">
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
/// The reply box sits at the **top**, above the transcript — new turns land at the top of the
/// transcript, right beneath where you type. The scroll container (`.adi-chat`) is built **once**
/// here, never inside the 1s poll's reactive island — so its scroll offset survives a refresh
/// instead of snapping to the top every second. Inside it, a keyed [`For`] reconciles the transcript:
/// a settled turn keeps its exact DOM (and the scroll position with it); only the still-streaming
/// turn — whose key folds in its growth — re-renders as it updates. Turns render **newest-first**
/// (the latest turn at the top).
fn feed_view(state: State, watch: AgentsWatch, answerable: bool) -> AnyView {
    view! {
        {answerable.then(|| reply_bar(state, watch))}
        <div class="adi-chat">
            <For
                each=move || {
                    let turns = watch.peek.get().map(|p| p.turns).unwrap_or_default();
                    // Enumerate first (stable keys), then reverse so the newest turn renders at the top.
                    let mut indexed: Vec<(usize, AgentTurn)> = turns.into_iter().enumerate().collect();
                    indexed.reverse();
                    indexed
                }
                key=|(idx, turn): &(usize, AgentTurn)| {
                    // A settled turn is keyed by its stable index, so its bubble is never rebuilt. The
                    // live turn folds its growth into the key, so it — and only it — re-renders as it streams.
                    if turn.pending {
                        format!("{idx}:live:{}:{}", turn.text.len(), turn.steps.len())
                    } else {
                        idx.to_string()
                    }
                }
                children=move |(_, turn)| chat_bubble(turn)
            />
            {move || chat_placeholder(watch)}
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

/// One transcript turn, rendered as **separate message bubbles**: the answer text first (with the
/// role label and, for an assistant turn, its metrics footer), then — for an assistant turn — each
/// tool call / thinking block as its own bubble below the answer, in **reverse** order (newest
/// activity nearest the answer). The still-streaming answer is tagged and, while it has no body yet,
/// shows a typing ellipsis.
fn chat_bubble(turn: AgentTurn) -> AnyView {
    let is_user = turn.role == "user";
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

    // The answer/message bubble comes first (on top); the text renders as Markdown, and the metrics
    // footer rides with it.
    let message = view! {
        <div class=turn_class data-error=errored.then_some("1")>
            <div class="adi-chat__role">
                {who}
                {pending.then(|| view! { <span class="adi-chat__typing">" · answering…"</span> })}
            </div>
            {(!text.trim().is_empty()).then(|| crate::markdown::render(&text))}
            {metrics.map(metrics_view)}
        </div>
    };

    // The activity — tool calls and thinking — is collapsed by default under a single disclosure, so
    // the message reads clean; expanding it reveals each step (newest first) as its own row.
    let step_count = steps.len();
    let activity = steps.into_iter().rev().map(step_bubble).collect::<Vec<_>>();
    let activity_group = (step_count > 0).then(|| {
        let label = format!("{step_count} step{}", if step_count == 1 { "" } else { "s" });
        view! {
            <details class="adi-chat__steps">
                <summary class="adi-chat__steps-head">
                    <span class="adi-chat__steps-icon">"🔧"</span>
                    {label}
                </summary>
                {activity}
            </details>
        }
    });

    view! {
        {message}
        {activity_group}
    }
    .into_any()
}

/// One activity step as its own message bubble beneath the answer — a tool call or a thinking block.
fn step_bubble(step: AgentStep) -> AnyView {
    view! {
        <div class="adi-chat__turn adi-chat__turn--agent adi-chat__turn--step">
            {step_row(step)}
        </div>
    }
    .into_any()
}

/// One activity row. A `<details>` so its arguments/output (or reasoning) expand in place — no JS.
fn step_row(step: AgentStep) -> AnyView {
    match step {
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

/// The reply box: sends the next turn into the selected conversation. Disabled (and no-ops) while a
/// turn is still being answered — one turn runs at a time — and while empty.
fn reply_bar(state: State, watch: AgentsWatch) -> impl IntoView {
    let answering = move || watch.peek.get().is_some_and(|p| p.running);
    view! {
        <form class="adi-form adi-chat__replybar"
            on:submit=move |ev| {
                ev.prevent_default();
                let message = watch.reply.get();
                if message.trim().is_empty() || answering() {
                    return;
                }
                watch.reply.set(String::new());
                send_reply(state, watch, with_context(watch, message));
            }>
            <input class="adi-input adi-input--wide adi-mono" autocomplete="off"
                placeholder="reply…"
                prop:value=move || watch.reply.get()
                on:input=move |ev| watch.reply.set(event_target_value(&ev)) />
            <button class="adi-btn adi-btn--primary" type="submit"
                prop:disabled=move || watch.reply.get().trim().is_empty() || answering()>
                {move || if answering() { "Answering…" } else { "Send" }}
            </button>
        </form>
    }
}

/// Send the reply box's message as the next turn, applying the returned snapshot at once (so the
/// question and the streaming answer appear immediately) and resuming the poll. Errors go to flash.
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
    selected: Option<&str>,
    answerable: bool,
) -> AnyView {
    let run_id = r.run_id.clone();
    let is_selected = selected == Some(run_id.as_str());
    let running = r.running;
    let when = run_age(r.started_at);
    let task_full = r.message.clone();
    let task_short = truncate_task(&task_full);
    let status = match (answerable, running) {
        (true, true) => "● answering",
        (true, false) => "idle",
        (false, true) => "● running",
        (false, false) => "done",
    };
    let view_id = run_id.clone();
    let stop_id = run_id.clone();
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
                    <button class="adi-btn adi-btn--link" title=stop_title
                        on:click=move |_| stop_one_run(state, watch, stop_id.clone())>"Stop"</button>
                }})}
            </td>
        </tr>
    }
    .into_any()
}

/// The composer that starts a new run/conversation: a message input plus a Start/Run button. A
/// message is required — the button stays disabled (and submit no-ops) until one is typed.
/// Submitting launches it and opens its detail: a streaming log for a one-shot run, or the chat for
/// an answerable conversation you then reply to.
fn run_bar(state: State, watch: AgentsWatch) -> impl IntoView {
    let placeholder = move || {
        if watch.answerable.get() {
            "start a conversation — your first message (required)"
        } else {
            "task for a new run (required) — e.g. review the latest commit and summarize it"
        }
    };
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
                launch_agent(state, watch, name, with_context(watch, message));
            }>
            <input class="adi-input adi-input--wide adi-mono" autocomplete="off"
                placeholder=placeholder
                prop:value=move || watch.input.get()
                on:input=move |ev| watch.input.set(event_target_value(&ev)) />
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
