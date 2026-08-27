//! The Agents page run, stop, and live-view actions.
//!
//! An agent definition is a *template*. For interactive (pty) backends a Run starts a session you
//! type into and View watches its pane. For headless (`process` / `harness`) backends each Run is an
//! independent run of the agent's settings (a fresh dialog, never continued): every run keeps its
//! own log, several may be live at once, and the live view is a browsable run history plus a task
//! composer — never a shared, overwritten slot.

use adi_webapp_api::types::{
    AgentAsk, AgentAwait, AgentDto, AgentGoal, AgentNearDup, AgentRepeat, AgentRepeatShape,
    AgentRunInfo,
    AgentRuns, AgentStep, AgentTokenSource,
    AgentTokens, AgentToolStatus, AgentTurn, AgentsState, Dashboard,
    FleetDashboards, NodeDashboard, NodeDashboards,
};
use adi_ui::{EmptyRow, Row as TableRow, Table};
use leptos::prelude::*;
use wasm_bindgen_futures::spawn_local;

use crate::fetch;
use crate::routing::scroll_top;
use crate::state::{
    AgentsWatch, ChatDrawer, Flash, ROOT_AGENT, SESSION_PAGE, SessionMenu, State,
    refresh_fleet_dashboards,
};
use crate::ui::{
    Key, Sort, TableState, apply_mutation, sort_rows,
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

/// Whether a launch of `name` would be refused right now. The verdict is the server's own, carried
/// per agent, so a project's cap is honoured as exactly as the global one — a launch button can say
/// "anyway" *before* anything has to be refused.
pub(crate) fn at_run_limit(agents: Option<&AgentsState>, name: &str) -> bool {
    agents.is_some_and(|s| s.agents.iter().any(|a| a.name == name && a.at_run_limit))
}

/// The global run cap, stated and editable: how many runs are live against how many may be. It is
/// what holds *automatic* launches back — a trigger firing, a queued chat turn — so it belongs next
/// to the agents rather than in a settings screen. `0` lifts it.
pub(crate) fn run_limit_view(state: State) -> impl IntoView {
    let agents = state.agents;
    let load = Memo::new(move |_| {
        agents
            .get()
            .map(|a| (a.running_runs, a.max_concurrent_runs))
    });
    limit_control(
        state,
        "Runs live now, against the most allowed at once",
        "How many agent runs may be live at once (0 lifts it). Automatic launches — a trigger firing, a queued chat turn — wait for a free slot; you can always run one anyway.",
        load,
        move |max| {
            let msg = if max == 0 {
                "No overall limit — runs start whenever they are asked for.".to_string()
            } else {
                format!("At most {max} agent runs at once.")
            };
            apply_agents(state, None, msg, fetch::set_run_limit(max, None));
        },
    )
}

/// The open project's own run cap: at most this many of the global allowance may be its agents'.
/// `0` clears it, leaving the project bound only by the global number.
pub(crate) fn project_run_limit_view(state: State) -> impl IntoView {
    let agents = state.agents;
    let project = state.current_project;
    let load = Memo::new(move |_| {
        let id = project.get();
        let state = agents.get()?;
        // A project with neither a cap nor anything running has no row of its own — that is zero of
        // an unset limit, not "unknown".
        Some(
            state
                .project_run_limits
                .iter()
                .find(|p| p.project == id)
                .map_or((0, 0), |p| (p.running_runs, p.max_concurrent_runs)),
        )
    });
    limit_control(
        state,
        "This project's runs live now, against its own limit",
        "How many of this project's agents may run at once (0 = no limit of its own). It narrows the global limit, never lifts it — an automatic launch waits for a free slot, and you can always run one anyway.",
        load,
        move |max| {
            let id = project.get_untracked();
            let msg = if max == 0 {
                format!("“{id}” is bound only by the overall limit now.")
            } else {
                format!("At most {max} runs of “{id}” at once.")
            };
            apply_agents(state, None, msg, fetch::set_run_limit(max, Some(id)));
        },
    )
}

/// The shape both caps are read and edited by: a "N/M running" chip and a box that sets M. `load`
/// is `(running, limit)` — `None` while the agents state is still loading, and a `0` limit reads as
/// "no limit" rather than a cap of nothing.
fn limit_control(
    state: State,
    chip_title: &'static str,
    hint: &'static str,
    load: Memo<Option<(u32, u32)>>,
    save: impl Fn(u32) + Copy + 'static,
) -> impl IntoView {
    // `None` follows the server's value; typing pins a draft until it is saved.
    let draft: RwSignal<Option<String>> = RwSignal::new(None);
    let value = move || {
        draft
            .get()
            .unwrap_or_else(|| load.get().map_or_else(String::new, |(_, max)| max.to_string()))
    };
    let submit = move || {
        let Ok(max) = value().trim().parse::<u32>() else {
            state.flash.set(Some(Flash::err(
                "Enter a whole number of runs — 0 lifts the limit.".to_string(),
            )));
            return;
        };
        draft.set(None);
        save(max);
    };
    view! {
        <span class="adi-chip adi-mono" title=chip_title>
            {move || load.get().map_or_else(|| "\u{2014}".to_string(), |(running, max)| if max == 0 {
                format!("{running} running")
            } else {
                format!("{running}/{max} running")
            })}
        </span>
        <input class="adi-input adi-input--num adi-mono" type="number" min="0" step="1"
            title=hint
            prop:value=value
            on:input=move |ev| draft.set(Some(event_target_value(&ev)))
            on:keydown=move |ev| if ev.key() == "Enter" { ev.prevent_default(); submit(); } />
        <button class="adi-btn adi-btn--link" type="button" title="save the run limit"
            on:click=move |_| submit()>"Set limit"</button>
    }
}

/// The Run / View / Stop action buttons for one agent row. Interactive Run starts a pty session
/// straight away; headless "Run…" opens the run panel, where a task is entered before launching — a
/// headless `--print` run is seeded by one prompt, not typed into. View opens the same panel (a live
/// pane for pty, the run history for headless); Stop ends the session, or every live run.
pub(crate) fn agent_actions(state: State, watch: AgentsWatch, a: &AgentDto) -> AnyView {
    let run_name = a.name.clone();
    let show_run = a.runnable && !a.running;
    // At a cap — the machine's or this agent's project's — a pty Run would be refused, so the
    // button asks for the override instead of pretending nothing is in the way.
    let full = a.at_run_limit;
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
                let title = if full {
                    "as many runs are live as the limit allows — start this session anyway"
                } else {
                    "start an interactive session"
                };
                view! {
                    <button class="adi-btn adi-btn--link" title=title
                        on:click=move |_| run_now(state, run_name.clone(), full)>
                        {if full { "▶ Run anyway" } else { "▶ Run" }}
                    </button>
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
/// into after it starts. The server supplies the executor-specific success message. `force` is the
/// human's "run it anyway" past a full concurrency limit.
fn run_now(state: State, name: String, force: bool) {
    spawn_local(async move {
        // No task and no attachments: a pty session is typed into after it starts, so there is no
        // message here for a picture to belong to.
        match fetch::run_agent(name, String::new(), None, force, Vec::new()).await {
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
/// `force` launches past a full concurrency limit — the composer sends it once the cap is reached,
/// where the button already reads "Run anyway".
fn launch_agent(
    state: State,
    watch: AgentsWatch,
    name: String,
    message: String,
    working_dir: Option<String>,
    force: bool,
    images: Vec<String>,
) {
    spawn_local(async move {
        match fetch::run_agent(name.clone(), message, working_dir, force, images).await {
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
                // A new conversation has none, so this is what clears the last one's off screen.
                load_goals(watch);
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
///
/// `agent` is named rather than taken from the watch, because the sessions rail lists every agent's
/// chats at once: the row's own agent is the one to delete from, not whichever one is on screen.
fn delete_one_run(state: State, watch: AgentsWatch, agent: String, run_id: String, title: String) {
    if agent.is_empty() || run_id.is_empty() {
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
    // Only when it is *this* conversation on screen — the rail can delete another agent's chat, and
    // the run ids of two different agents are no reason to close what is open.
    if watch.name.get_untracked().as_deref() == Some(agent.as_str())
        && watch.run_id.get_untracked().as_deref() == Some(run_id.as_str())
    {
        close_run_view(watch);
    }
    spawn_local(async move {
        match fetch::delete_run(agent.clone(), run_id).await {
            Ok(runs) => {
                if watch.name.get_untracked().as_deref() == Some(agent.as_str()) {
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
    load_goals(watch);
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
        load_goals(watch);
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

/// Put the chat home back the way it opened: no conversation selected, so the centre pane is the
/// composer again and the rails are unobstructed. What the bar's wordmark does — the same corner
/// the rail's "+ New" gets you out of, from the one control every screen has in the same place.
///
/// A reset and nothing more. "+ New" on a terminal agent *starts* a session; the mark must not,
/// because a mark that launches a process is a mark people stop clicking. Closing the live view is
/// enough: a pty agent then shows its Start affordance, which is exactly how the screen opened.
/// The composer's draft is left alone for the same reason — it is the reader's text, not view state.
pub(crate) fn reset_chat_home(state: State, watch: AgentsWatch) {
    close_run_view(watch);
    // On a narrow viewport the mark is reachable with a drawer open over the chat.
    state.chat_drawer.set(None);
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
            <Table state=state.tables.chats>{move || all_chats_rows(state, watch, &only)}</Table>
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
    if state.all_chats.get().is_none() {
        return view! { <EmptyRow state=table>"Loading…"</EmptyRow> }.into_any();
    }
    let mut rows = all_chats_flatten(state, only);
    if rows.is_empty() {
        return view! { <EmptyRow state=table>"No chats yet — start one from an agent below."</EmptyRow> }.into_any();
    }
    // By the start time, not the "3m ago" the cell renders — the two disagree the moment the
    // ages cross a unit boundary. Ties go to the newest, so the index stays newest-first.
    sort_rows(
        &mut rows,
        table.sort.get(),
        |(agent, answerable, _, r), col| match col {
            "Agent" => Key::text(agent),
            "Status" => Key::text(run_status(*answerable, r)),
            "Conversation" => Key::text(&r.message),
            _ => Key::num(r.started_at),
        },
        |(_, _, _, r)| Key::num(u64::MAX - r.started_at),
    );
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
            view! { <TableRow state=table cell=move |col| match col {
                    "Agent" => view! { <span class="font-mono">{agent.clone()}</span> }.into_any(),
                    other => run_cell(other, &r, answerable),
                } actions=open/> }.into_any()
        })
        .collect::<Vec<_>>()
        .into_any()
}

/// A run's status word: what it says depends on whether the backend holds a conversation or runs
/// a task to completion. Shared by the cross-agent index and one agent's history.
///
/// A conversation holding a registered wake is neither of the two words a stopped run has: it is
/// not idle, and it is not done. Saying "idle" of one is the same mistake the rail's `Recent` band
/// would make — it reads as *nothing more is coming from this*, which is exactly wrong.
fn run_status(answerable: bool, r: &AgentRunInfo) -> &'static str {
    match (answerable, r.running, r.awaits.is_empty()) {
        (true, true, _) => "\u{25CF} answering",
        (false, true, _) => "\u{25CF} running",
        (_, false, false) => "\u{25D0} awaiting",
        (true, false, true) => "idle",
        (false, false, true) => "done",
    }
}

/// What a conversation is waiting on the world for, written for a tooltip: the run's own note for
/// the wake, then the condition that would fire it.
///
/// The note leads because it is the only part a person wrote — the condition is the store's own
/// sentence, exact down to the payload filter that makes a wake this run's rather than a stranger's,
/// and worth carrying verbatim rather than paraphrasing into something that could disagree with it.
/// A conversation holding several says how many first: at that point *what* it is waiting for is no
/// longer one thing, and only the oldest is named.
fn awaiting_hint(awaits: &[AgentAwait]) -> String {
    let said = |a: &AgentAwait| {
        if a.note.trim().is_empty() {
            format!("waiting {}", a.summary)
        } else {
            format!("waiting {}\n\n{}", a.summary, a.note)
        }
    };
    match awaits {
        [] => String::new(),
        [one] => said(one),
        many => format!("waiting on {} wakes. The first: {}", many.len(), said(&many[0])),
    }
}

/// One run's cell under `col`. `Conversation` and `Task` are the same cell under two headers —
/// which of them a table declares is what the backend's kind decides. Matching the header text —
/// the same key the sort uses — is what lets the user hide and reorder columns without the row
/// builder knowing about it.
fn run_cell(col: &str, r: &AgentRunInfo, answerable: bool) -> AnyView {
    match col {
        // Blue only where there is a wake to be blue about, and the same blue the rail uses: two
        // surfaces showing the same state of the same conversation should not disagree about it.
        "Status" => {
            let awaiting = !r.running && !r.awaits.is_empty();
            let hint = awaiting_hint(&r.awaits);
            view! {
                <span class=if awaiting { "text-await" } else { "" } title=hint>
                    {run_status(answerable, r)}
                </span>
            }
            .into_any()
        }
        "Conversation" | "Task" => {
            let full = r.message.clone();
            let short = truncate_task(&full);
            view! { <span class="font-mono" title=full>{short}</span> }.into_any()
        }
        // "When", and anything the layout offers that this match doesn't name.
        _ => view! {
            <span class="text-meta" style="white-space:nowrap">{run_age(r.started_at)}</span>
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
    // history and a task history are not the same table. Each `TableState` already carries the
    // headers it was built with, so only the state has to be picked here.
    let table: TableState = if answerable {
        state.tables.chat_runs
    } else {
        state.tables.runs
    };
    sort_rows(
        &mut runs,
        table.sort.get(),
        |r, col| match col {
            "Status" => Key::text(run_status(answerable, r)),
            "Conversation" | "Task" => Key::text(&r.message),
            _ => Key::num(r.started_at),
        },
        |r| Key::num(u64::MAX - r.started_at),
    );
    let selected = watch.run_id.get();
    let mut rows: Vec<AnyView> = Vec::with_capacity(runs.len() + 1);
    for r in &runs {
        let is_selected = selected.as_deref() == Some(r.run_id.as_str());
        rows.push(run_row(state, watch, r, table, is_selected, answerable));
        // The log / chat opens as a detail row right beneath the run it belongs to.
        if is_selected {
            rows.push(run_detail_row(
                state,
                watch,
                r.run_id.clone(),
                table.layout.get().span(),
                answerable,
            ));
        }
    }
    view! { <Table state=table>{rows}</Table> }.into_any()
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
            // The padding, tint and wrapping used to come from `.adi-table td.adi-runlog__cell`,
            // which stopped matching when this table left the `adi-*` layer. The card inside is
            // still adi-css — only the cell around it had to move.
            <td
                class="adi-runlog__cell border-t border-divider bg-bubble px-3.5 pt-2.5 pb-3.5 \
                       whitespace-normal"
                colspan=span
            >
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

/// The progress feed under a selected run: the turns (each with its tool/thinking steps) which the
/// poll refreshes as they stream in — plus, for answerable backends, the reply box. What a turn cost
/// is not here; the analytics rail totals it.
///
/// The transcript reads as one alternating stream — message, that turn's tool calls, the message
/// before it, its tool calls, and so on — **newest first**, so the latest answer is at the top.
///
/// The reply box sits at the **top**, above the transcript — new turns land at the top of the
/// transcript, right beneath where you type. The transcript itself is [`adi_ui::Chat`], which owns
/// the scroll container and reconciles the turns as the poll rewrites them.
///
/// **Everything else in the feed rides in that same scroll**, as the transcript's `lead`: the open
/// question first, then whatever is queued, then the turns. Only the composer is pinned. This is
/// not a tidiness point — pinned above the scroll, a question card taller than the pane had its
/// bottom half cut off by the chat frame's `overflow: hidden`, with no scrollbar anywhere that
/// reached the remaining questions or the Answer button. Inside the scroll it is one more thing in
/// the feed, and a long one scrolls like a long message does.
///
/// Newest-first is what keeps that safe: the scroll opens at the top, so the question is the first
/// thing on screen even though it is no longer pinned there.
///
/// Messages still waiting in the queue trail the transcript, so in a newest-first feed they sit at
/// the very top: what you have already said is nearest the box you said it in.
fn feed_view(state: State, watch: AgentsWatch, answerable: bool) -> AnyView {
    // Memos, not derived signals, and that is the whole optimisation on this side: a poll writes a
    // fresh snapshot whenever *anything* in it moved, and a plain derive would hand that on as
    // "the transcript changed" every time. A memo compares what it produced, so the settled list
    // notifies only when a turn is genuinely added or finalised — which, for a conversation being
    // watched, is a handful of times rather than once a second.
    let settled = Memo::new(move |_| feed_entries(watch, false));
    let live = Memo::new(move |_| feed_entries(watch, true));
    // Narrowed for the same reason: the question card owns the selections you are making in it,
    // and rebuilding it re-creates them empty. Bound to the whole snapshot it was thrown away and
    // rebuilt on every poll, so a tool call finishing behind an open question cleared the answer
    // half-typed. It now only rebuilds when the question itself changes.
    let question = Memo::new(move |_| {
        watch.peek.with(|p| p.as_ref().and_then(|p| p.pending_question.clone()))
    });
    // Likewise: the queue changes when you queue or unqueue something, not when the agent works.
    let queued = Memo::new(move |_| {
        watch.peek.with(|p| {
            p.as_ref()
                .map(|p| p.turns.iter().filter(|t| t.queued).cloned().collect::<Vec<_>>())
                .unwrap_or_default()
        })
    });
    view! {
        // Above the composer, because a goal is a standing condition on the conversation rather
        // than a thing said in it — and only where there is a conversation to hold one.
        {answerable.then(|| goal_bar(state, watch))}

        // And under it, for the same reason and the other direction: a goal is what the
        // conversation is *for*, a wake is what it is *waiting on* before it can go on. Both
        // outlive every turn beneath them, and neither was said in the transcript.
        {answerable.then(|| awaits_bar(state, watch))}

        // The composer sits above the transcript, because the transcript reads newest-first:
        // what you type appears at the top, next to the box you typed it in.
        {answerable.then(|| reply_bar(state, watch))}

        <adi_ui::Chat
            class="adi-ui-type min-h-0 flex-1 p-3"
            lead=move || {
                view! {
                    // First in the feed while it is up: the conversation is not going anywhere
                    // until it is answered, and it is what the eye should land on when the pane
                    // opens.
                    {move || question_card(state, watch, question.get())}

                    // Queued messages are said but not yet asked, so they belong above everything
                    // that has happened — as [`adi_ui::Queued`], the hollowed-out twin of the
                    // bubbles below, each carrying the × that takes one back before the agent ever
                    // sees it.
                    {move || {
                        let mut bubbles: Vec<AnyView> = queued
                            .get()
                            .into_iter()
                            .enumerate()
                            .map(|(place, t)| queued_bubble(state, watch, t, place))
                            .collect();
                        if bubbles.is_empty() {
                            return None;
                        }
                        // Newest first, like everything below them.
                        bubbles.reverse();
                        Some(view! { <div class="flex flex-col gap-3">{bubbles}</div> })
                    }}
                }
                .into_any()
            }
            turns=settled
            live=live
        />
        {move || chat_placeholder(watch)}
    }
    .into_any()
}

/// A tool call's arguments, one per parameter — **the way the model wrote them**.
///
/// The wire hands the whole input over as one string, because that is what the engine
/// captured. Almost always it is a JSON object, and showing it as one is the difference
/// between reading a call and decoding it: `<parameter name="file_path">…` is what the model
/// emitted, while `<parameter name="input">{"file_path":…}` is a transport's idea of it,
/// wrapped in quotes and escapes the model never saw.
///
/// A string value is unwrapped, so a newline in an edit is a newline on screen rather than
/// `\n`. Anything that is not an object — a bare string, a number, something that does not
/// parse — stays one `input`, because inventing a shape for it would be a lie.
fn tool_params(input: &str) -> Vec<(String, String)> {
    let one = || vec![("input".to_string(), input.to_string())];
    let Ok(serde_json::Value::Object(map)) = serde_json::from_str::<serde_json::Value>(input)
    else {
        return one();
    };
    if map.is_empty() {
        return one();
    }
    map.into_iter()
        .map(|(k, v)| {
            let text = match v {
                serde_json::Value::String(s) => s,
                other => other.to_string(),
            };
            (k, text)
        })
        .collect()
}

/// One wire turn, as the transcript's own entries.
///
/// A user turn is one thing said. An assistant turn is a *sequence*: what it did, what it
/// said in the middle, what it did next — and its final message, which the wire keeps apart
/// from the steps. Text is the divider, so every run of tool calls between two things it
/// said becomes one foldable run.
///
/// `at` is the turn's index in the whole snapshot — **including queued turns**, which the feed
/// draws elsewhere but which still occupy an index. It is what every key and anchor here is built
/// from, and [`collect_stats`] counts the same way, so the rail's links address the elements this
/// actually renders.
///
/// The keys are the point of the split ([`adi_ui::Entry`] explains what they buy). A settled
/// turn's parts are a fixed sequence, so `{turn}-{part}` names the same bubble for as long as the
/// conversation lives, and the keyed list leaves it alone. The parts of a turn still being
/// written are *not* fixed — a run splits in two the moment the agent says something mid-turn —
/// which is exactly why that turn is drawn outside the keyed list.
fn feed_turn(at: usize, turn: &AgentTurn) -> Vec<adi_ui::Entry> {
    use adi_ui::{Entry, Role, ToolCall, ToolState, Turn as T};

    // The first part carries the turn's own anchor, unadorned: that is the id the rail hands out
    // for a turn the engine reported as failed, and the first bubble is where a reader sent to
    // "this turn" should land.
    let key = |part: usize| {
        if part == 0 {
            turn_anchor(at)
        } else {
            format!("{}-{part}", turn_anchor(at))
        }
    };

    if turn.role == "user" {
        return vec![Entry::new(
            key(0),
            T::Said {
                role: Role::User,
                body: turn.text.clone(),
                images: pictures(turn),
            },
        )];
    }

    let mut out: Vec<T> = Vec::new();
    let mut run: Vec<ToolCall> = Vec::new();
    for (i, step) in turn.steps.iter().enumerate() {
        match step {
            // Something said mid-turn closes the run before it: that is what makes text the
            // divider rather than one more thing in the list.
            AgentStep::Message { text } => {
                if !run.is_empty() {
                    out.push(T::Did(std::mem::take(&mut run)));
                }
                if !text.trim().is_empty() {
                    out.push(T::Said {
                        role: Role::Agent,
                        body: text.clone(),
                        images: Vec::new(),
                    });
                }
            }
            // Thinking is the agent's private work, which is exactly what folds. It joins the
            // run rather than interrupting it, so it never breaks a sequence in two.
            AgentStep::Thinking { text } => {
                run.push(ToolCall::new("thinking").param("text", text.clone()));
            }
            AgentStep::Tool { name, input, status, output } => {
                let mut call = ToolCall::new(name.clone())
                    .state(match status {
                        AgentToolStatus::Running => ToolState::Running,
                        AgentToolStatus::Ok => ToolState::Ok,
                        AgentToolStatus::Error => ToolState::Failed,
                        AgentToolStatus::Unanswered => ToolState::Unanswered,
                    })
                    // A call is addressed by its place in `turn.steps`, not by its place in the
                    // run it ended up in: the run is a rendering decision that a mid-turn message
                    // can change, while the step index is what the snapshot itself agrees to.
                    .anchor(step_anchor(at, i));
                for (key, value) in tool_params(input) {
                    call = call.param(key, value);
                }
                if !output.is_empty() {
                    call = call.result(output.clone());
                }
                run.push(call);
            }
        }
    }
    if !run.is_empty() {
        out.push(T::Did(run));
    }
    // The turn's final message is not a step; it is what the turn came back with.
    if !turn.text.trim().is_empty() {
        out.push(T::Said {
            role: Role::Agent,
            body: turn.text.clone(),
            images: Vec::new(),
        });
    }
    out.into_iter()
        .enumerate()
        .map(|(part, t)| Entry::new(key(part), t))
        .collect()
}

/// Split a snapshot's turns into the settled transcript and the one turn still being written.
///
/// Everything that changes between two polls of a live run is confined to the last turn: a step
/// appended, a tool call answered, the streaming text grown, the metrics landing when it ends.
/// Drawing that one turn apart is what lets the rest be keyed and left alone — see
/// [`adi_ui::Entry`] for what that saves, and [`Chat`](adi_ui::Chat)'s `live` prop for the other
/// half of the arrangement.
///
/// Queued messages are in the snapshot but not in either list: they are drawn as the feed's lead,
/// because they have been typed rather than said. They still consume a turn index, which is why
/// this enumerates before it filters.
fn feed_entries(watch: AgentsWatch, live: bool) -> Vec<adi_ui::Entry> {
    // `with` rather than `get`: a snapshot holds every turn, every step and every tool result, and
    // this runs once per subscriber per poll. Cloning the transcript to look at it is the kind of
    // cost that does not show up anywhere except the profile.
    watch.peek.with(|peek| {
        let Some(turns) = peek.as_ref().map(|p| p.turns.as_slice()) else {
            return Vec::new();
        };
        // The still-being-written turn is the last one actually said. A finished run has one too;
        // it simply never changes again, and a card that is rebuilt but identical costs nothing.
        let last = turns.iter().rposition(|t| !t.queued);
        turns
            .iter()
            .enumerate()
            .filter(|(at, t)| !t.queued && (Some(*at) == last) == live)
            .flat_map(|(at, t)| feed_turn(at, t))
            .collect()
    })
}

/// A turn's attached images, as the transcript draws them: a URL to fetch each by, and its name.
///
/// The bytes are never in the snapshot — a chat is polled once a second, and one that inlined its
/// screenshots would re-send every one of them every tick. The address is stable and the content
/// behind it never changes, so the browser fetches each exactly once.
fn pictures(turn: &AgentTurn) -> Vec<adi_ui::Image> {
    turn.images
        .iter()
        .map(|image| adi_ui::Image {
            url: crate::attach::url_of(&image.id),
            name: image.name.clone(),
        })
        .collect()
}

/// A message still waiting its turn: your own bubble, dashed and dimmed — said, but not yet asked —
/// carrying an × that takes it back before the agent ever sees it. The bubble itself is
/// [`adi_ui::Queued`], which is the same shape as the sent messages above it in the feed.
fn queued_bubble(state: State, watch: AgentsWatch, turn: AgentTurn, place: usize) -> AnyView {
    view! {
        <adi_ui::Queued
            body=turn.text.clone()
            images=pictures(&turn)
            on_unqueue=Callback::new(move |()| unqueue_message(state, watch, place))
        />
    }
    .into_any()
}

/// The placeholder shown inside the (persistent) chat container while the transcript is still empty —
/// before the first turn lands, or for a finished run that produced nothing. Renders nothing once any
/// turn exists, so it never sits among the bubbles.
fn chat_placeholder(watch: AgentsWatch) -> Option<AnyView> {
    // `with`, because the two facts this needs are a length and a flag, and `get` would copy the
    // whole transcript once a second to read them.
    let msg = watch.peek.with(|peek| match peek {
        Some(p) if !p.turns.is_empty() => None,
        None => Some("Loading…"),
        Some(p) if p.running => Some("Working…"),
        Some(_) => Some("No output."),
    })?;
    Some(view! { <adi_ui::Empty class="adi-ui-type">{msg}</adi_ui::Empty> }.into_any())
}

/// What a turn is called in the DOM, so the analytics rail can scroll to one.
fn turn_anchor(turn: usize) -> String {
    format!("adi-turn-{turn}")
}

/// What one step of a turn is called in the DOM. Keyed by the step's place in `turn.steps` — its
/// original order, not the reversed order it is drawn in — because that is the index the rail carries
/// and the one the transcript on the server agrees to.
fn step_anchor(turn: usize, step: usize) -> String {
    format!("adi-step-{turn}-{step}")
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
// arrive as something the agent reads differently from what was meant.
//
// So both composers — the one that starts a conversation and the one that answers in it — are the
// same control: `adi_ui::Composer`, which grows with what is written, sends on Enter, and shows
// whether it can send. They are the same box because they are the same act; the screen you are on
// when you say something to an agent is not a reason for the saying to look different.

/// What the composers say about themselves on hover. Enter sending (rather than breaking the line)
/// is the convention every chat box uses, and the one the muscle memory here already had; the
/// modifier is how you earn a newline instead.
const COMPOSER_HINT: &str = "Enter sends · Shift-Enter for a new line";

/// What a composer says instead of offering a paperclip, when this conversation cannot be shown an
/// image. Said before anything is pasted, because a picture the send would have dropped is one
/// somebody has already decided to send.
///
/// Rare, now that every real engine can be shown one — either in the request body or by being told
/// where the file is. What is left is a live terminal, which is typed into rather than sent to, and
/// a simulated run, which has a person in the model's seat and nothing to show a picture to.
const IMAGES_REFUSED: &str = "this one can't be shown an image — a terminal session takes typing, \
                              and a simulated run has no model to show it to";

/// The reply box: says the next thing into the selected conversation. It never locks you out while
/// the agent is working — one turn runs at a time, so a message sent mid-answer is *queued* (the
/// button says so), and is picked up either by the turn in flight, at its next round, or by the one
/// that starts when this answer lands. Beside it, while an answer is streaming, a Stop that cuts the
/// turn short and drops anything lined up behind it.
fn reply_bar(state: State, watch: AgentsWatch) -> impl IntoView {
    let answering = move || watch.peek.get().is_some_and(|p| p.running);
    // What the open conversation's own backend can take — the snapshot's capability profile, not
    // the agent's current settings, because a conversation is answered by whatever started it.
    let takes_images =
        Signal::derive(move || watch.peek.get().is_some_and(|p| p.caps.images));
    let attach = crate::attach::attaching(
        state,
        watch.reply_files,
        takes_images,
        Signal::derive(|| IMAGES_REFUSED.to_string()),
    );
    view! {
        <div class="adi-ui-type p-3 pb-0">
            <adi_ui::Composer
                value=watch.reply
                // Answering is not busy: a message typed while the agent works is *queued*,
                // which is a thing you are allowed to do and the placeholder says so.
                busy=false
                placeholder=""
                attr:title=COMPOSER_HINT
                mic=move || crate::voice::mic(watch.reply)
                attach=attach
                on_send=Callback::new(move |message: String| {
                    let images = crate::attach::ready_ids(watch.reply_files);
                    watch.reply.set(String::new());
                    crate::attach::clear(watch.reply_files);
                    send_reply(state, watch, with_context(watch, message), images);
                })
                // Only while a turn is actually in flight: between turns there is nothing to cut
                // short, and the conversation itself is not a thing you stop — you just stop
                // typing into it.
                stoppable=Signal::derive(answering)
                on_stop=Callback::new(move |()| {
                    let Some(run_id) = watch.run_id.get_untracked() else { return };
                    stop_one_run(state, watch, run_id);
                })
            />
            <div class="mt-1 px-1 text-mini text-meta">
                {move || if answering() { "queued — the agent is answering" } else { "" }}
            </div>
        </div>
    }
}

/// What this conversation is *for*: its open goals, each with the two ways out, and a box to set
/// one.
///
/// Sits above the composer rather than in the transcript, because a goal is not something that was
/// said — it is a standing condition on the whole conversation, and it outlives every turn under
/// it. Closed goals are not drawn: what a chat already met is history, and the transcript carries
/// the turn that met it.
fn goal_bar(state: State, watch: AgentsWatch) -> AnyView {
    view! {
        <div class="adi-ui-type px-3 pt-2">
            {move || {
                if watch.goal_editor.get() {
                    return goal_editor(state, watch);
                }
                let open: Vec<AgentGoal> = watch
                    .goals
                    .get()
                    .into_iter()
                    .filter(|g| g.state == "open")
                    .collect();
                // Nothing set — and nothing set is the normal case, so it costs one quiet line.
                if open.is_empty() {
                    return goal_link(watch);
                }
                open.into_iter().map(|goal| goal_row(state, watch, goal)).collect::<Vec<_>>()
                    .into_any()
            }}
        </div>
    }
    .into_any()
}

/// The closed state: one text link, and no more of the screen than that.
///
/// Most conversations never have a goal. A permanently open text box for something rarely used
/// takes more room than the composer it sits above, and reads as a field somebody forgot to fill in
/// rather than an option they can take.
fn goal_link(watch: AgentsWatch) -> AnyView {
    view! {
        <adi_ui::Button
            variant=adi_ui::ButtonVariant::Link
            size=adi_ui::ButtonSize::Small
            class="px-0"
            attr:title="Set what would make this chat done. It is put back to the agent every time \
                        the chat falls quiet, until it is met or given up on."
            on:click=move |_| open_goal_editor(watch, None)
        >
            "+ Set a goal"
        </adi_ui::Button>
    }
    .into_any()
}

/// The open editor: the box, Save, Cancel, and the one line explaining what a goal does.
///
/// The explanation lives here rather than in the collapsed bar because this is the moment somebody
/// is deciding what to type — the rest of the time it is a sentence taking up a row to tell you
/// about a feature you are not using.
fn goal_editor(state: State, watch: AgentsWatch) -> AnyView {
    let save = move || {
        let text = watch.goal_input.get_untracked();
        if text.trim().is_empty() {
            return;
        }
        set_goal(state, watch, text, watch.goal_editing.get_untracked());
    };
    view! {
        <div class="mb-1 flex items-center gap-2">
            <adi_ui::Input
                value=watch.goal_input
                width=adi_ui::InputWidth::Wide
                placeholder="what would make this chat done"
                disabled=Signal::derive(move || watch.goal_busy.get())
                // Enter saves and Escape closes, because this opened under the cursor and asking
                // for the mouse back to dismiss a one-line box is the annoying half of a popover.
                on:keydown=move |ev: leptos::ev::KeyboardEvent| {
                    match ev.key().as_str() {
                        "Enter" => save(),
                        "Escape" => close_goal_editor(watch),
                        _ => {}
                    }
                }
            />
            <adi_ui::Button
                size=adi_ui::ButtonSize::Small
                disabled=Signal::derive(move || {
                    watch.goal_busy.get() || watch.goal_input.get().trim().is_empty()
                })
                on:click=move |_| save()
            >
                "Save"
            </adi_ui::Button>
            <adi_ui::Button
                variant=adi_ui::ButtonVariant::Ghost
                size=adi_ui::ButtonSize::Small
                on:click=move |_| close_goal_editor(watch)
            >
                "Cancel"
            </adi_ui::Button>
        </div>
        <div class="mb-1 text-mini text-meta">
            "Put back to the agent every time this chat falls quiet, until it is met or given up on."
        </div>
    }
    .into_any()
}

/// One open goal, on one line: what it says, and the two ways it ends.
///
/// The text itself is the edit control — a goal is a sentence somebody wrote, and the obvious thing
/// to do with a sentence you disagree with is click it. That also keeps the row down to its two
/// real actions.
///
/// The nudge count appears from the second one on. It is the only visible sign of a run circling a
/// goal — nothing closes one on its behalf — and "asked 1×" on every goal would make the number
/// furniture rather than a signal.
fn goal_row(state: State, watch: AgentsWatch, goal: AgentGoal) -> AnyView {
    let (met_id, gave_id, edit_id) = (goal.id.clone(), goal.id.clone(), goal.id.clone());
    let text = goal.text.clone();
    view! {
        <div class="mb-1 flex items-center gap-2 text-mini">
            <span class="shrink-0 text-meta" title="This chat has a goal">"Goal"</span>
            <adi_ui::Button
                variant=adi_ui::ButtonVariant::Link
                size=adi_ui::ButtonSize::Small
                class="min-w-0 flex-1 justify-start truncate px-0 text-left"
                attr:title=format!(
                    "{text} — click to reword{}",
                    if goal.set_by == "agent" { " (the agent set this itself)" } else { "" },
                )
                on:click=move |_| open_goal_editor(watch, Some((edit_id.clone(), text.clone())))
            >
                {goal.text.clone()}
            </adi_ui::Button>
            {(goal.set_by == "agent").then(|| view! {
                <span class="shrink-0 text-meta" title="The agent set this goal for itself">"self-set"</span>
            })}
            {(goal.nudges > 1).then(|| view! {
                <span class="shrink-0 text-meta">{format!("asked {}×", goal.nudges)}</span>
            })}
            <adi_ui::Button
                variant=adi_ui::ButtonVariant::Ghost
                size=adi_ui::ButtonSize::Small
                disabled=Signal::derive(move || watch.goal_busy.get())
                attr:title="Close this goal as met"
                on:click=move |_| close_goal(state, watch, met_id.clone(), "met")
            >
                "Met"
            </adi_ui::Button>
            <adi_ui::Button
                variant=adi_ui::ButtonVariant::Danger
                size=adi_ui::ButtonSize::Small
                disabled=Signal::derive(move || watch.goal_busy.get())
                attr:title="Stop working toward this goal, and stop being asked about it"
                on:click=move |_| close_goal(state, watch, gave_id.clone(), "given_up")
            >
                "Give up"
            </adi_ui::Button>
        </div>
    }
    .into_any()
}

/// What this conversation is waiting on the *world* for: every wake it has registered, and the one
/// way out of each.
///
/// Sits under the goal bar, and is a **view of the store** rather than a list this tab owns — the
/// awaits ride the conversation snapshot, so one registered from inside a turn appears within a
/// poll and one that fires disappears the same way, without anybody having to refresh. That is also
/// why there is no "register" control here: an await is a note a run leaves *itself*, and a wake
/// nobody asked for is not one to hand out from a browser.
///
/// Nothing at all when there are none, which is the ordinary case. Unlike a goal there is nothing
/// to offer in that state — no line, no link — because there is nothing a person would want to do
/// here except to a wake that already exists.
fn awaits_bar(state: State, watch: AgentsWatch) -> AnyView {
    view! {
        <div class="adi-ui-type">
            {move || {
                // Narrowed to the awaits alone, so the rows are rebuilt when a wake is registered
                // or fires — not once a second, under a hand reaching for "Stop waiting".
                let awaits = watch
                    .peek
                    .with(|p| p.as_ref().map(|p| p.awaits.clone()).unwrap_or_default());
                if awaits.is_empty() {
                    return ().into_any();
                }
                view! {
                    <div class="px-3 pt-2">
                        {awaits
                            .into_iter()
                            .map(|a| await_row(state, watch, a))
                            .collect::<Vec<_>>()}
                    </div>
                }
                .into_any()
            }}
        </div>
    }
    .into_any()
}

/// One registered wake, on one line: what would wake the chat, why the run said it wanted waking,
/// and the one control — stop waiting for it.
///
/// The note leads and the condition follows in the tooltip. The condition is the machine's sentence
/// (`on adi.agents.run.finished carrying run_id=…, if the check passes`) and the note is the run's
/// own — and a reader scanning a chat wants to know what it is waiting *for* long before they want
/// to know what event carries it. A wake with no note falls back to the condition, because a row
/// that said only "waiting" would be a row that says nothing.
fn await_row(state: State, watch: AgentsWatch, a: AgentAwait) -> AnyView {
    let id = a.id.clone();
    let said = if a.note.trim().is_empty() {
        a.summary.clone()
    } else {
        a.note.clone()
    };
    // Both halves in the tooltip whichever one the row printed — the same sentence the rail and the
    // All chats table hang on this conversation — and the check verbatim under them. A wake that is
    // not firing is nearly always a check that keeps saying "not yet", and the command is the whole
    // of what a person can act on; this is the one place with room to show it.
    let hint = match &a.check {
        Some(check) => format!("{}\n\nchecks: {check}", awaiting_hint(std::slice::from_ref(&a))),
        None => awaiting_hint(std::slice::from_ref(&a)),
    };
    view! {
        <div class="mb-1 flex items-center gap-2 text-mini">
            <span class="shrink-0 text-await" title="This chat is waiting on something">
                "Awaiting"
            </span>
            <span class="min-w-0 flex-1 truncate text-secondary" title=hint>{said}</span>
            {countdown(watch, &a)}
            <adi_ui::Button
                variant=adi_ui::ButtonVariant::Ghost
                size=adi_ui::ButtonSize::Small
                disabled=Signal::derive(move || watch.await_busy.get())
                attr:title="Drop this wake. The chat stops waiting for it and stays where it is — \
                            nothing is cancelled at the other end."
                on:click=move |_| ignore_await(state, watch, id.clone())
            >
                "Stop waiting"
            </adi_ui::Button>
        </div>
    }
    .into_any()
}

/// How long until a wake's deadline, or nothing at all for one that is only watching for an event.
///
/// Recomputed off the poll rather than on a timer of its own, exactly as the question card's
/// deadline is: the snapshot already lands about once a second, which is finer than a countdown in
/// minutes can show, and it costs one text node instead of a second clock.
fn countdown(watch: AgentsWatch, a: &AgentAwait) -> Option<AnyView> {
    // Seconds, not milliseconds: an await's clock is whole seconds all the way down to the record
    // on disk, and reading it as millis would put every deadline half a century in the past.
    let at = a.at?;
    let every = a.every;
    let text = Signal::derive(move || {
        // Tracked for its cadence alone — nothing below reads the snapshot.
        watch.peek.track();
        let now = (js_sys::Date::now() / 1000.0) as u64;
        match at.saturating_sub(now) {
            0 => "due now".to_string(),
            left => match every {
                Some(_) => format!("looks again in {}", short_duration(left * 1000)),
                None => format!("in {}", short_duration(left * 1000)),
            },
        }
    });
    Some(view! { <span class="shrink-0 text-meta">{text}</span> }.into_any())
}

/// Stop waiting on one wake.
///
/// The bar is not updated from the answer even though the endpoint returns the remainder: the
/// awaits on screen come from the snapshot, and writing a second copy of them here would be a list
/// that could disagree with the poll for a second. The row goes when the next snapshot lands, which
/// is the same second either way.
fn ignore_await(state: State, watch: AgentsWatch, id: String) {
    let (Some(name), Some(run_id)) = (watch.name.get_untracked(), watch.run_id.get_untracked())
    else {
        return;
    };
    watch.await_busy.set(true);
    spawn_local(async move {
        let dropped = fetch::ignore_agent_await(name, run_id, id).await;
        watch.await_busy.set(false);
        if let Err(e) = dropped {
            state.flash.set(Some(Flash::err(e)));
        }
    });
}

/// Open the editor on a new goal (`None`) or on one being reworded, seeded with its current text.
fn open_goal_editor(watch: AgentsWatch, editing: Option<(String, String)>) {
    match editing {
        Some((id, text)) => {
            watch.goal_editing.set(Some(id));
            watch.goal_input.set(text);
        }
        None => {
            watch.goal_editing.set(None);
            watch.goal_input.set(String::new());
        }
    }
    watch.goal_editor.set(true);
}

/// Close the editor and drop the draft — Cancel, Escape, and a successful save all end here.
fn close_goal_editor(watch: AgentsWatch) {
    watch.goal_editor.set(false);
    watch.goal_editing.set(None);
    watch.goal_input.set(String::new());
}

/// Load the open conversation's goals. Called when a conversation is opened and after each write —
/// not from the poll, which has no reason to carry a list that only changes when somebody changes
/// it.
fn load_goals(watch: AgentsWatch) {
    let (Some(name), Some(run_id)) = (watch.name.get_untracked(), watch.run_id.get_untracked())
    else {
        return;
    };
    // Cleared up front when the conversation changed, so the previous chat's goals are never on
    // screen under this one's title while the fetch is in flight.
    if watch.goals_of.get_untracked().as_deref() != Some(run_id.as_str()) {
        watch.goals.set(Vec::new());
        watch.goal_input.set(String::new());
    }
    spawn_local(async move {
        let Ok(goals) = fetch::agent_goals(name.clone(), run_id.clone()).await else {
            return;
        };
        // Only if the view is still on this conversation — the same guard every other fetch here
        // takes, because a slow answer must not land under a chat somebody has since opened.
        if watch.run_id.get_untracked().as_deref() != Some(run_id.as_str()) {
            return;
        }
        watch.goals.set(goals.goals);
        watch.goals_of.set(Some(run_id));
    });
}

/// Write the editor's text — a new goal, or a rewording when `editing` names one.
///
/// The editor is closed only on success. A goal the server refused is still in the box, which is
/// where somebody can fix it.
fn set_goal(state: State, watch: AgentsWatch, text: String, editing: Option<String>) {
    let (Some(name), Some(run_id)) = (watch.name.get_untracked(), watch.run_id.get_untracked())
    else {
        return;
    };
    watch.goal_busy.set(true);
    spawn_local(async move {
        let saved = fetch::set_agent_goal(name, run_id, text, editing).await;
        watch.goal_busy.set(false);
        match saved {
            Ok(goals) => {
                close_goal_editor(watch);
                watch.goals.set(goals.goals);
            }
            Err(e) => state.flash.set(Some(Flash::err(e))),
        }
    });
}

/// Close a goal from the chat. `as_` is `met` or anything else, which the endpoint reads as giving
/// up — the two are named rather than a boolean, because "not met" is not what giving up means.
fn close_goal(state: State, watch: AgentsWatch, goal: String, as_: &'static str) {
    watch.goal_busy.set(true);
    spawn_local(async move {
        let closed = fetch::close_agent_goal(goal, as_.to_string(), String::new()).await;
        watch.goal_busy.set(false);
        match closed {
            Ok(goals) => watch.goals.set(goals.goals),
            Err(e) => state.flash.set(Some(Flash::err(e))),
        }
    });
}

/// The open conversation's question, if it is waiting on one — [`adi_ui::Ask`] wired to the answer
/// endpoint.
///
/// Drawn from the poll's own snapshot, so it appears within a second of the run asking and vanishes
/// within a second of anybody answering, wherever they answered from: the card is a view of a
/// stored question, never a thing this tab owns.
///
/// `ask` is passed in rather than read from the snapshot here, and that is not a style point: the
/// card holds the selections being made in it, so it must be rebuilt only when the *question*
/// changes. Reading the whole snapshot rebuilt it on every poll, which threw those selections away
/// once a second while the run behind the question kept working.
fn question_card(state: State, watch: AgentsWatch, ask: Option<AgentAsk>) -> Option<AnyView> {
    let ask = ask?;
    let ask_id = ask.id.clone();
    let questions: Vec<adi_ui::AskQuestion> = ask
        .questions
        .iter()
        .map(|q| adi_ui::AskQuestion {
            header: q.header.clone(),
            question: q.question.clone(),
            options: q
                .options
                .iter()
                .map(|o| adi_ui::AskOption {
                    label: o.label.clone(),
                    description: o.description.clone(),
                })
                .collect(),
            multi_select: q.multi_select,
        })
        .collect();
    // Recomputed on every poll rather than on a timer of its own: the snapshot already arrives
    // about once a second, which is finer than a countdown in minutes can show.
    let deadline = ask.deadline;
    let deadline_note = Signal::derive(move || {
        // Tracked for its cadence alone — nothing here reads the snapshot. It is what keeps this
        // ticking now that the card around it is rebuilt only when the question changes, and it
        // costs one text node per poll rather than the card and the answers held in it.
        watch.peek.track();
        match deadline {
            None => String::new(),
            Some(at) => match at.saturating_sub(js_sys::Date::now() as u64) {
                0 => "taking its own default now".to_string(),
                left => format!("takes its own default in {}", short_duration(left)),
            },
        }
    });
    // No wrapper of its own: this is the transcript's `lead`, so the padding, the type scale and
    // the gap between it and the turn below it all come from the `Chat` it sits inside.
    Some(
        view! {
            <adi_ui::Ask
                note=ask.note.clone()
                questions=questions
                deadline_note=deadline_note
                busy=watch.answering
                on_answer=Callback::new(move |replies: Vec<String>| {
                    send_answer(state, watch, ask_id.clone(), replies);
                })
            />
        }
        .into_any(),
    )
}

/// Answer the open conversation's question, applying the returned snapshot at once so the card goes
/// and the answer's turn appears in the same round-trip.
///
/// A 404 here is not a failure to report as one: it means the question was settled while this card
/// sat open — somebody else answered, or its deadline took the run's own default. The poll is
/// resumed either way, which is what clears the card.
fn send_answer(state: State, watch: AgentsWatch, ask: String, replies: Vec<String>) {
    let Some(name) = watch.name.get_untracked() else {
        return;
    };
    let Some(run_id) = watch.run_id.get_untracked() else {
        return;
    };
    watch.answering.set(true);
    spawn_local(async move {
        let answered = fetch::answer_run(name.clone(), run_id.clone(), ask, replies).await;
        // Only apply if the view is still on this same conversation.
        if watch.name.get_untracked().as_deref() != Some(name.as_str())
            || watch.run_id.get_untracked().as_deref() != Some(run_id.as_str())
        {
            return;
        }
        watch.answering.set(false);
        match answered {
            Ok(peek) => {
                watch.peek.set(Some(peek));
                poll_watch(watch);
            }
            Err(e) => {
                state.flash.set(Some(Flash::err(e)));
                poll_watch(watch);
            }
        }
    });
}

/// Milliseconds as the coarsest unit that still says something: `40s`, `12m`, `3h`, `2d`.
fn short_duration(ms: u64) -> String {
    let secs = ms / 1000;
    match secs {
        0..=59 => format!("{secs}s"),
        60..=3599 => format!("{}m", secs / 60),
        3600..=86_399 => format!("{}h", secs / 3600),
        _ => format!("{}d", secs / 86_400),
    }
}

/// Say the reply box's message into the conversation, applying the returned snapshot at once (so the
/// message — asked or queued — and any streaming answer appear immediately) and resuming the poll.
/// Errors go to flash.
fn send_reply(state: State, watch: AgentsWatch, message: String, images: Vec<String>) {
    let Some(name) = watch.name.get_untracked() else {
        return;
    };
    let Some(run_id) = watch.run_id.get_untracked() else {
        return;
    };
    spawn_local(async move {
        match fetch::reply_to_run(name.clone(), run_id.clone(), message, images).await {
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
    table: TableState,
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
    // The selected row tints itself; `Row`'s `class` takes it, so this still goes through the
    // shared row rather than a hand-built `<tr>`.
    let row_class = if is_selected { "bg-selected" } else { "" };
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
    let r = r.clone();
    view! {
        <TableRow
            state=table
            class=row_class
            cell=move |col| run_cell(col, &r, answerable)
            actions=view! {
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
                        state, watch, watch.name.get_untracked().unwrap_or_default(),
                        del_id.clone(), del_title.clone(),
                    )>"Delete"</button>
            }
            .into_any()
        />
    }
    .into_any()
}

/// How many conversations, across every agent, are stopped waiting on a person.
///
/// Read off the cross-agent index the rail is already drawn from rather than from
/// `/api/agents/questions` — that endpoint exists for the CLI and for whatever forwards a question
/// to a phone, and fetching it here as well would be a second count of the same thing, arriving on
/// its own schedule and free to disagree with the rail underneath it.
fn waiting_on_you(state: State) -> usize {
    state
        .all_chats
        .get()
        .map(|all| {
            all.agents
                .iter()
                .flat_map(|a| &a.runs)
                .filter(|r| r.pending_question.is_some())
                .count()
        })
        .unwrap_or(0)
}

/// The composer that starts a new run/conversation: the same box the reply bar is, plus an
/// optional directory to run it in. A message is required — the send button is out until one is
/// typed. Sending launches it and opens its detail: a streaming log for a one-shot run, or the
/// chat for an answerable conversation you then reply to.
///
/// The directory box is the answer to "this agent, but against *that* target". Left blank — the
/// normal case — the run starts where the agent is defined to. It applies to the launch only; a
/// conversation then keeps the directory it started in for every reply. It sits *under* the
/// composer, in the mono the design system sets a path in: it is a value, and a rarely-used one,
/// so it takes a second line rather than a third of the first.
fn run_bar(state: State, watch: AgentsWatch) -> impl IntoView {
    let placeholder = Signal::derive(move || {
        if watch.answerable.get() {
            "start a conversation — your first message".to_string()
        } else {
            "task for a new run — e.g. review the latest commit and summarize it".to_string()
        }
    });
    // A launch typed here is a human asking, so a full cap is an override to offer rather than a
    // refusal to hand back. The send button is the reply box's arrow now and cannot say "anyway",
    // so the line under the composer is what says it — before Enter is ever pressed.
    let at_limit = move || {
        watch
            .name
            .get()
            .is_some_and(|name| at_run_limit(state.agents.get().as_ref(), &name))
    };
    let start = move |message: String| {
        let Some(name) = watch.name.get_untracked() else {
            return;
        };
        let dir = watch.run_dir.get_untracked();
        let dir = dir.trim();
        let working_dir = (!dir.is_empty()).then(|| dir.to_string());
        let force = at_run_limit(state.agents.get_untracked().as_ref(), &name);
        let images = crate::attach::ready_ids(watch.input_files);
        watch.input.set(String::new());
        crate::attach::clear(watch.input_files);
        launch_agent(
            state,
            watch,
            name,
            with_context(watch, message),
            working_dir,
            force,
            images,
        );
    };
    // Asked of the agent this composer is pointed at, before any conversation exists — which is
    // the only thing that can answer here, and the reason the capability rides on the listing as
    // well as on a snapshot.
    let takes_images = Signal::derive(move || {
        let Some(name) = watch.name.get() else {
            return false;
        };
        state.agents.get().is_some_and(|s| {
            s.agents
                .iter()
                .find(|a| a.name == name)
                .is_some_and(|a| a.caps.images)
        })
    });
    let attach = crate::attach::attaching(
        state,
        watch.input_files,
        takes_images,
        Signal::derive(|| IMAGES_REFUSED.to_string()),
    );
    view! {
        <div class="adi-runbar adi-ui-type">
            <adi_ui::Composer
                value=watch.input
                busy=false
                placeholder=placeholder
                attr:title=COMPOSER_HINT
                mic=move || crate::voice::mic(watch.input)
                attach=attach
                on_send=Callback::new(start)
            />
            <adi_ui::Input
                value=watch.run_dir
                width=adi_ui::InputWidth::Wide
                placeholder="run here (optional) — /path/to/target"
                attr:title="Run this launch in a directory other than the agent's own. Blank = as defined."
            />
            <div class="px-1 text-mini text-meta">
                {move || if at_limit() { "the agent is at its run cap — this starts it anyway" } else { "" }}
            </div>
        </div>
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
// The app *is* the chat: every agent's sessions on the left, the open conversation in the centre,
// the dashboards list on the right. Built on the same live-view machinery as the Agents page, so a
// pty agent shows a live terminal + input and a headless one shows its conversations.
//
// The rail is one list, newest activity first, whichever agent a session belongs to — so a click
// reaches any of them, not only the root. The picker above it says which agent "+ New" starts on.

/// The three-pane chat home. `watch` is expected to already point at an agent — the root one, until
/// the picker moves it (the caller sets the name + interactive flag and drives the 1s poll).
/// `state.agents` feeds the picker, `state.all_chats` the sessions rail, and `state.dashboards` the
/// right one.
pub(crate) fn chat_home_view(state: State, watch: AgentsWatch) -> AnyView {
    // Here rather than in the rail, which is rebuilt on every poll: this screen is built once (the
    // caller reads only `meta` and `reconfiguring`), so the listener is installed once and torn
    // down when the wizard takes the screen.
    install_session_hotkeys(state, watch);
    view! {
        <div class="adi-chome">
            // Narrow viewports only (the stylesheet hides it above the breakpoint): the two rails
            // have no column of their own down there, so this is the only way to reach them.
            <div class="adi-chome__mobilebar">
                <button class="adi-chome__drawer-btn" type="button"
                    aria-label="Show sessions"
                    on:click=move |_| toggle_drawer(state, ChatDrawer::Sessions)>
                    <span class="adi-chome__drawer-icon" aria-hidden="true">"\u{2630}"</span>
                    <span>"Sessions"</span>
                    // Down here the rail is behind this button, so the one thing it says that
                    // cannot wait for somebody to open it is how many chats are stopped on you.
                    {move || {
                        let n = waiting_on_you(state);
                        (n > 0).then(|| view! {
                            <span class="adi-ui-type">
                                <adi_ui::Badge tone=adi_ui::BadgeTone::Accent mono=true>
                                    {n.to_string()}
                                </adi_ui::Badge>
                            </span>
                        })
                    }}
                </button>
                <span class="adi-spacer"></span>
                <button class="adi-chome__drawer-btn" type="button"
                    aria-label=move || format!("Show {}", right_rail_title(watch).to_lowercase())
                    on:click=move |_| toggle_drawer(state, ChatDrawer::Right)>
                    <span>{move || right_rail_title(watch)}</span>
                </button>
            </div>

            // One scrim for whichever drawer is open. Rendered only while one is, so it never sits
            // over the chat on a wide viewport where the drawers do not exist.
            {move || {
                state.chat_drawer.get().map(|_| view! {
                    <div class="adi-chome__scrim"
                        on:click=move |_| state.chat_drawer.set(None)></div>
                })
            }}

            // The wrapper keeps the drawer behaviour (`is-open` is what slides it in on a
            // narrow viewport); the rail inside it is `adi-ui`'s, so it draws its own island,
            // its own title and its own scrolling body.
            <aside class="adi-chome__side adi-chome__side--left adi-chome__side--flush adi-ui-type"
                class:is-open=move || state.chat_drawer.get() == Some(ChatDrawer::Sessions)>
                <adi_ui::Rail
                    title="Sessions"
                    actions=move || {
                        view! {
                            {move || chat_starred_button(state)}
                            {move || chat_new_button(state, watch)}
                            <button class="adi-chome__drawer-close" type="button"
                                aria-label="Close"
                                on:click=move |_| state.chat_drawer.set(None)>"\u{2715}"</button>
                        }
                        .into_any()
                    }
                >
                    {move || chat_rail(state, watch)}
                </adi_ui::Rail>
            </aside>

            <main class="adi-chome__chat">
                {move || chat_center(state, watch)}
            </main>

            <aside class="adi-chome__side adi-chome__side--right adi-chome__side--flush adi-ui-type"
                class:is-open=move || state.chat_drawer.get() == Some(ChatDrawer::Right)>
                // Who is running this conversation, where, and how — its own island above the
                // rail, because it is what the conversation *is* rather than what it has come
                // to. Absent while no conversation is open, which is when the rail below is the
                // dashboards and there is nothing for this to be about.
                {move || chat_agent_panel(state, watch)}

                // Rebuilt when the title changes, which is the same moment the body does:
                // this rail is either the dashboards or the open conversation's counts, and
                // it swaps both at once.
                {move || view! {
                <adi_ui::Rail
                    title=right_rail_title(watch).to_string()
                    actions=move || {
                        view! {
                            // The dashboards head controls belong to the dashboards rail, and
                            // only to it: there is nothing to refresh or manage about a
                            // conversation's counts.
                            {move || (!showing_analytics(watch)).then(|| view! {
                                {chat_fleet_refresh(state)}
                                <a class="adi-chome__side-link"
                                    href="/extended/dashboards">"Manage"</a>
                            })}
                            <button class="adi-chome__drawer-close" type="button"
                                aria-label="Close"
                                on:click=move |_| state.chat_drawer.set(None)>"\u{2715}"</button>
                        }
                        .into_any()
                    }
                >
                    {move || if showing_analytics(watch) {
                        chat_analytics(state, watch)
                    } else {
                        chat_dashboards(state)
                    }}
                </adi_ui::Rail>
                }}
            </aside>

            // The rail's right-click menu, drawn once here rather than per row: only one is ever
            // open, and it is `position: fixed` at the pointer, so where it sits in the tree is
            // immaterial — outside the scrolling rail keeps it from being clipped by it.
            {move || chat_session_menu(state, watch)}
        </div>
    }
    .into_any()
}

/// Whether the right rail is currently the open conversation's analytics rather than the dashboards.
///
/// A conversation, not the chat screen, is what the counts are *of* — so the rail only becomes
/// analytics once one is open. The compose screen and a terminal agent (which has a live pane, not a
/// transcript) keep the dashboards, which is the thing worth having in front of you there.
fn showing_analytics(watch: AgentsWatch) -> bool {
    !watch.interactive.get() && watch.run_id.get().is_some()
}

/// What the right rail is called right now — its head, and the narrow-viewport button that summons
/// it. One function so the two can never disagree about which rail is behind them.
fn right_rail_title(watch: AgentsWatch) -> &'static str {
    if showing_analytics(watch) {
        "Chat Analytics"
    } else {
        "Apps"
    }
}

/// One tool call the rail has something to say about, and where in the feed it is.
#[derive(Clone)]
struct StepRef {
    anchor: String,
    tool: String,
    /// The call's arguments, cut to a line — what distinguishes this failure from the next one.
    arg: String,
}

/// What a conversation adds up to, counted once per render from the transcript the centre pane is
/// already showing.
#[derive(Default)]
struct ChatStats {
    you: usize,
    agent: usize,
    queued: usize,
    tools: usize,
    thinking: usize,
    /// The tool calls that failed, and the ones still going — kept as references rather than counts,
    /// because a count of failures is a number and a list of them is somewhere to go.
    failed: Vec<StepRef>,
    running: Vec<StepRef>,
    /// Turns the engine reported as failed outright, as anchors into the feed.
    errored: Vec<String>,
    /// Turns that failed and left *nothing* behind — no step, no text — so the transcript draws
    /// no bubble for them and there is nowhere to send a reader. Counted rather than linked,
    /// because the alternative is a jump that lands on nothing, which is what the rail used to
    /// offer. They are still failures and still belong in the total.
    errored_silent: usize,
    /// Tools blocked by permission, worst first.
    blocked: Vec<(String, usize)>,
    /// Every tool used: name, calls, and how many of those failed. Most-used first.
    by_tool: Vec<(String, usize, usize)>,
    tokens: u64,
    cost_micro: u64,
    /// Time the agent spent answering, summed over turns that reported it.
    work_ms: u64,
    /// When the conversation's first and last settled turns landed.
    first_at: u64,
    last_at: u64,
}

/// Add up a transcript.
///
/// Turn indices are the enumeration order of `turns`, queued messages included — exactly what
/// [`feed_turn`] keys and anchors each bubble by — so every anchor built here addresses an element
/// that is actually on screen. The two must be counted the same way or the rail's links land
/// nowhere, which is what they did while the transcript emitted no ids at all.
/// Queued messages are counted apart from the totals: they have been typed, not asked, and folding
/// them in would report a conversation longer than the one the agent has actually had.
fn collect_stats(turns: &[AgentTurn]) -> ChatStats {
    let mut s = ChatStats::default();
    let mut tools: Vec<(String, usize, usize)> = Vec::new();
    let mut blocked: Vec<(String, usize)> = Vec::new();
    let bump = |list: &mut Vec<(String, usize)>, name: &str| match list
        .iter_mut()
        .find(|(n, _)| n == name)
    {
        Some((_, n)) => *n += 1,
        None => list.push((name.to_string(), 1)),
    };

    for (t, turn) in turns.iter().enumerate() {
        if turn.queued {
            s.queued += 1;
            continue;
        }
        if turn.role == "user" {
            s.you += 1;
        } else {
            s.agent += 1;
        }
        if turn.at > 0 {
            if s.first_at == 0 {
                s.first_at = turn.at;
            }
            s.last_at = turn.at;
        }
        if let Some(m) = &turn.metrics {
            s.tokens += m.input_tokens.unwrap_or(0) + m.output_tokens.unwrap_or(0);
            s.cost_micro += m.cost_micro_usd.unwrap_or(0);
            s.work_ms += m.duration_ms.unwrap_or(0);
            if m.is_error {
                // Asked of `feed_turn` itself rather than re-derived here: the question is
                // literally "does this turn draw anything", and a second opinion on it would be
                // a link that goes dead the day the two answers drift apart.
                if feed_turn(t, turn).is_empty() {
                    s.errored_silent += 1;
                } else {
                    s.errored.push(turn_anchor(t));
                }
            }
            for name in &m.permission_denials {
                bump(&mut blocked, name);
            }
        }
        for (i, step) in turn.steps.iter().enumerate() {
            match step {
                AgentStep::Thinking { .. } => s.thinking += 1,
                AgentStep::Tool {
                    name,
                    input,
                    status,
                    ..
                } => {
                    s.tools += 1;
                    match tools.iter_mut().find(|(n, _, _)| n == name) {
                        Some((_, calls, _)) => *calls += 1,
                        None => tools.push((name.clone(), 1, 0)),
                    }
                    if *status == AgentToolStatus::Error
                        && let Some((_, _, bad)) = tools.iter_mut().find(|(n, _, _)| n == name)
                    {
                        *bad += 1;
                    }
                    let step_ref = || StepRef {
                        anchor: step_anchor(t, i),
                        tool: name.clone(),
                        arg: truncate_task(input),
                    };
                    match status {
                        AgentToolStatus::Error => s.failed.push(step_ref()),
                        AgentToolStatus::Running => s.running.push(step_ref()),
                        // Never answered is not a failure to link to: the call is the last line
                        // of an interrupted run, and the rail already says the run is over.
                        AgentToolStatus::Ok | AgentToolStatus::Unanswered => {}
                    }
                }
                AgentStep::Message { .. } => {}
            }
        }
    }

    tools.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    blocked.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    s.by_tool = tools;
    s.blocked = blocked;
    s
}

/// **Who** is running this conversation, **where**, and **how** — the block above Chat Analytics in
/// the right rail.
///
/// Its own island rather than a band inside the analytics rail, because it is not analytics: those
/// counts are what this conversation has added up to, and this is what it *is*. The two answer
/// different questions and only one of them changes as you talk.
///
/// Three readings, in the order they get asked: which agent this is and what it is doing; where the
/// conversation runs and how long it has been going; and what the agent was configured with — the
/// settings that explain behaviour you are looking at, without going to another page to find them.
///
/// Everything here is already in hand — the watch's signals and the listings the shell polls — so
/// the block costs no endpoint of its own. It renders before the first turn lands, too, which is
/// exactly when "which agent am I talking to, and where" is worth answering and when the counts
/// below it have nothing to show.
///
/// The state word is ranked the way the sessions rail ranks it — a question outranks a turn in
/// flight, and a wake yields to both (see [`chat_session_row`]) — so one conversation is never
/// described two ways by two surfaces looking at it.
fn chat_agent_panel(state: State, watch: AgentsWatch) -> Option<AnyView> {
    let name = watch.name.get()?;
    let run_id = watch.run_id.get()?;
    // The agent's own history when the per-agent poll has filled it, the cross-agent listing
    // otherwise — the same precedence, and the same fallback, as the sessions rail beside this
    // one (see [`session_bands`]). On the chat screen only the second of the two is fetched,
    // and taking the first alone would leave this card timeless there.
    let run = watch
        .runs
        .get()
        .into_iter()
        .find(|r| r.run_id == run_id)
        .or_else(|| {
            state.all_chats.get().and_then(|all| {
                all.agents
                    .into_iter()
                    .find(|a| a.name == name)
                    .and_then(|a| a.runs.into_iter().find(|r| r.run_id == run_id))
            })
        });
    let peek = watch.peek.get();
    let answerable = watch.answerable.get();
    let def = state
        .agents
        .get()
        .and_then(|a| a.agents.into_iter().find(|d| d.name == name));

    // The peek is a second behind nothing — it is the socket the centre pane is drawing from — so
    // it decides "is it running" when the listing hasn't caught up to a conversation this young.
    let running = peek.as_ref().is_some_and(|p| p.running);
    let waiting = peek.as_ref().is_some_and(|p| p.pending_question.is_some())
        || run.as_ref().is_some_and(|r| r.pending_question.is_some());
    let word = match (waiting, &run) {
        (true, _) => "\u{25CF} waiting on you",
        (false, Some(r)) => run_status(answerable, r),
        (false, None) if running => "\u{25CF} running",
        (false, None) => "idle",
    };
    let tone = match (waiting, running) {
        (true, _) => "waiting",
        (_, true) => "working",
        _ => "quiet",
    };

    // What the agent is *made of*, on one line: the backend that runs it and the model it runs on.
    // Both are the agent's definition rather than the run's, so they are absent for a conversation
    // whose agent has since been deleted — the run stays readable, it just can't say what ran it.
    let made_of = def.as_ref().map(|d| {
        let model = d
            .arguments
            .get("model")
            .and_then(|m| m.as_str())
            .unwrap_or_default();
        if model.is_empty() {
            d.backend.clone()
        } else {
            format!("{} \u{00b7} {model}", d.backend)
        }
    });
    let project = def
        .as_ref()
        .and_then(|d| d.project.clone())
        .map(|id| crate::pages::hive::project_path(&projects_of(state), &id));
    let (started, last) = run
        .as_ref()
        .map_or((0, 0), |r| (r.started_at, r.last_activity));
    // The conversation's own directory, which is not always the agent's: a run can be pointed
    // somewhere else at launch, and it keeps that directory for every reply after.
    let cwd = peek.as_ref().map(|p| p.cwd.clone()).unwrap_or_default();

    Some(
        view! {
            <section class="adi-chome__about island bg-panel">
                <header class="adi-chome__about-head">
                    // The rail's own title classes, so the two headers in this column are the
                    // same size and weight rather than nearly so.
                    <h2 class="m-0 text-sub font-semibold text-ink">"Agent"</h2>
                    <span class="adi-spacer"></span>
                    <a class="adi-chome__side-link" href="/extended/agents"
                        title="define and configure agents">"Settings"</a>
                </header>
                <div class="adi-chome__about-body">
                    <div class="adi-chome__agent">
                        <span class="adi-chome__agent-name adi-mono">{name}</span>
                        <span class="adi-chome__agent-state" data-state=tone>{word}</span>
                    </div>
                    {made_of.map(|m| view! { <div class="adi-chome__group-note">{m}</div> })}
                    {project.map(|p| view! {
                        <div class="adi-chiprow">
                            <span class="adi-chip adi-mono"
                                title="the project this agent is filed under">{p}</span>
                        </div>
                    })}

                    <div class="adi-chome__about-sec">
                        <div class="adi-chome__about-sec-head">"Chat"</div>
                        <div class="adi-chome__about-grid">
                            {(!cwd.is_empty())
                                .then(|| meta_row("Dir", short_path(&cwd), cwd.clone(), true))}
                            // Its own island: the ages are read off the clock rather than off
                            // the data, so without the ticker "started 3m ago" would still say
                            // 3m long after it wasn't.
                            {(started > 0).then(|| view! {
                                <span class="adi-chome__meta-key">"Started"</span>
                                <span class="adi-chome__meta-val">
                                    {move || {
                                        state.secs_since.track();
                                        if last > started {
                                            format!("{} \u{00b7} last said {}",
                                                run_age(started), run_age(last))
                                        } else {
                                            run_age(started)
                                        }
                                    }}
                                </span>
                            })}
                            {meta_row("Run", run_id.clone(), run_id, true)}
                        </div>
                    </div>

                    {def.map(|d| chat_agent_settings(&d))}
                </div>
            </section>
        }
        .into_any(),
    )
}

/// The settings that explain the behaviour in front of you: how freely the agent may act, what it
/// carries, and what it remembers. Read off the definition, so it is the same answer the Agents
/// page would give.
///
/// Only what is *set* is drawn. An agent with no secrets attached has nothing to say about secrets,
/// and a row reading "Secrets 0" would take a line to say so in a rail this narrow — the same rule
/// the analytics rail below follows when it names only the exceptions.
fn chat_agent_settings(d: &AgentDto) -> AnyView {
    let arg = |key: &str| {
        d.arguments
            .get(key)
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string()
    };
    // Two backends spell the same idea differently, and an agent carries whichever its own one
    // reads — so the row is drawn from the first that is set rather than from a single key that is
    // right for `claude-sdk` and blank for everything else.
    let permissions = [arg("permission_mode"), arg("approval_policy"), arg("sandbox")]
        .into_iter()
        .find(|v| !v.is_empty());
    /// `1 tool`, `41 tools` — a count that reads as a phrase, since this row is a sentence of
    /// them rather than a column of numbers.
    fn many(n: usize, noun: &str) -> String {
        match n {
            1 => format!("1 {noun}"),
            n => format!("{n} {noun}s"),
        }
    }
    let mut carries: Vec<String> = Vec::new();
    if !d.bin_tools.is_empty() {
        carries.push(many(d.bin_tools.len(), "tool"));
    }
    if !d.knowledge.is_empty() {
        carries.push(many(d.knowledge.len(), "knowledge base"));
    }
    if !d.secrets.is_empty() {
        carries.push(many(d.secrets.len(), "secret"));
    }
    if d.memory {
        carries.push("memory".to_string());
    }
    if permissions.is_none() && carries.is_empty() && !d.unattended {
        return ().into_any();
    }
    view! {
        <div class="adi-chome__about-sec">
            <div class="adi-chome__about-sec-head">"Settings"</div>
            <div class="adi-chome__about-grid">
                {permissions.map(|p| meta_row("Permissions", p.clone(), p, false))}
                {(!carries.is_empty()).then(|| {
                    let all = carries.join(" \u{00b7} ");
                    meta_row("Carries", all.clone(), all, false)
                })}
                // Unattended changes what the agent *does* — its Ask refuses rather than stopping
                // the work on a question nobody is there to answer — so it is said outright rather
                // than left to be inferred from a missing row.
                {d.unattended.then(|| meta_row(
                    "Runs".to_string(),
                    "unattended".to_string(),
                    "the Ask tool refuses; the run decides and says what it assumed".to_string(),
                    false,
                ))}
            </div>
        </div>
    }
    .into_any()
}

/// One `key   value` pair of the block — two cells of the grid around it, rather than a row of its
/// own, so every key column in a section is the width of its longest key and the values line up
/// without a magic number to keep in step with the labels.
///
/// `hover` is the whole of a value the rail is too narrow to show. `ident` marks a value that must
/// not wrap — a path or a run id, which is one string and reads as nonsense broken over two lines;
/// everything else wraps rather than hiding its tail behind an ellipsis nobody thinks to hover.
fn meta_row(key: impl Into<String>, value: String, hover: impl Into<String>, ident: bool) -> AnyView {
    view! {
        <span class="adi-chome__meta-key">{key.into()}</span>
        <span class="adi-chome__meta-val adi-mono" class:adi-chome__meta-val--id=ident
            title=hover.into()>{value}</span>
    }
    .into_any()
}

/// A path cut to its last few segments — `…/mono/projects/bugbounty` — for a rail about a third the
/// width of the paths it shows.
///
/// The tail is what is kept because the tail is what distinguishes: every directory on this machine
/// starts with the same home, and an ellipsis at the end would leave a column of rows that all read
/// `/Users/someone/…`. The whole path is a hover away.
fn short_path(path: &str) -> String {
    const KEEP: usize = 3;
    const FITS: usize = 24;
    let segments: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    if path.len() <= FITS || segments.len() <= KEEP {
        return path.to_string();
    }
    format!("\u{2026}/{}", segments[segments.len() - KEEP..].join("/"))
}

/// The projects listing, or an empty one while it is still loading — what a name path is resolved
/// against.
fn projects_of(state: State) -> Vec<adi_webapp_api::types::Project> {
    state
        .projects
        .get()
        .map(|p| p.projects)
        .unwrap_or_default()
}

/// What the open conversation has added up to so far — how much was said, how much work the agent
/// did to say it, and what went wrong on the way.
///
/// The counts come off [`AgentPeek::turns`], the transcript the centre pane already polls each
/// second, so the panel is live without an endpoint or a poll of its own. What it costs to *itemize*
/// the context — the repeated-text section at the bottom — is the one thing here that is fetched, and
/// only when asked for.
///
/// Every exception it names is a link. A failed tool call forty bubbles down a newest-first feed is,
/// in practice, invisible; a rail that could only say "2 failed" would be reporting a problem while
/// withholding its location.
fn chat_analytics(state: State, watch: AgentsWatch) -> AnyView {
    let turns = watch.peek.get().map(|p| p.turns).unwrap_or_default();
    if turns.is_empty() {
        return view! {
            <div class="adi-chome__empty">
                {if watch.peek.get().is_some() { "Nothing said yet." } else { "Loading\u{2026}" }}
            </div>
        }
        .into_any();
    }
    let s = collect_stats(&turns);

    let mut msg_sub = format!("{} you \u{b7} {} agent", s.you, s.agent);
    if s.queued > 0 {
        msg_sub.push_str(&format!(" \u{b7} {} queued", s.queued));
    }
    // Only the exceptions are named: a run of tool calls that all worked has nothing to add here,
    // and saying "0 failed" every time would make the line that does matter easy to read past.
    let mut tool_sub: Vec<String> = Vec::new();
    if !s.running.is_empty() {
        tool_sub.push(format!("{} running", s.running.len()));
    }
    if !s.failed.is_empty() {
        tool_sub.push(format!("{} failed", s.failed.len()));
    }
    if s.thinking > 0 {
        tool_sub.push(format!("{} thinking", s.thinking));
    }

    // The spend tile appears only for a backend that reports telemetry. A conversation on an engine
    // that reports none would otherwise show a confident `$0`, which is a claim, not a blank.
    let spend = (s.cost_micro > 0 || s.tokens > 0 || s.work_ms > 0).then(|| {
        let value = if s.cost_micro > 0 {
            fmt_cost(s.cost_micro)
        } else {
            format!("{} tok", fmt_count(s.tokens))
        };
        let mut sub: Vec<String> = Vec::new();
        if s.cost_micro > 0 && s.tokens > 0 {
            sub.push(format!("{} tok", fmt_count(s.tokens)));
        }
        if s.work_ms > 0 {
            sub.push(format!("{} working", fmt_duration(s.work_ms)));
        }
        // Wall clock beside working time is the comparison worth drawing: it separates a slow
        // conversation from one that is merely old.
        if s.last_at > s.first_at {
            sub.push(format!("over {}", fmt_duration(s.last_at - s.first_at)));
        }
        text_tile("Spend", value, sub.join(" \u{b7} "))
    });

    view! {
        {review_action(state, watch)}
        {stat_tile("Messages", s.you + s.agent, msg_sub)}
        {stat_tile("Tool calls", s.tools, tool_sub.join(" \u{b7} "))}
        {spend}
        {(!s.failed.is_empty()).then(|| jump_list("Failed", "\u{2717}", "error", s.failed.clone()))}
        {(!s.running.is_empty()).then(|| jump_list("Running", "\u{27F3}", "running", s.running.clone()))}
        {(!s.errored.is_empty() || s.errored_silent > 0)
            .then(|| errored_list(s.errored.clone(), s.errored_silent))}
        {(!s.blocked.is_empty()).then(|| blocked_list(&s.blocked))}
        {(!s.by_tool.is_empty()).then(|| tool_breakdown(&s.by_tool))}
        {chat_token_report(state, watch)}
    }
    .into_any()
}

/// The rail's one *action*, at the top of it because everything below is what it reads.
///
/// The counts in this rail say what a conversation cost and what went wrong in it; none of them says
/// what to do about any of it. This hands the whole session — its configuration and system prompt,
/// the tool-by-tool trace, the failures, the repeats, and how this agent behaves across its other
/// sessions — to the root agent, and asks that question.
///
/// The answer is not rendered here. It arrives as a conversation with `adi-agent`, which the screen
/// jumps to: a review you can argue with, and tell to go and apply the part you agreed with, beats a
/// report card in a panel.
fn review_action(state: State, watch: AgentsWatch) -> AnyView {
    view! {
        <div class="adi-chome__group">
            <button class="adi-chome__analyze adi-chome__analyze--lead" type="button"
                disabled=move || watch.review_busy.get()
                title="Hand this conversation to adi-agent and ask how the workflow should have gone"
                on:click=move |_| start_review(state, watch)>
                {move || if watch.review_busy.get() {
                    "Handing it over\u{2026}"
                } else {
                    "Analyze this chat"
                }}
            </button>
            <div class="adi-chome__group-note">
                "adi-agent reads the whole session \u{2014} prompt, tools, failures, repeats \u{2014} \
                 and answers with what to change: the workflow, what to harden, what wants a tool."
            </div>
        </div>
    }
    .into_any()
}

/// Write the dossier and start the reviewer on it, then go and watch.
///
/// The screen moves to the review's own conversation on success. Nothing about *this* chat changes —
/// a review is a second conversation about the first, and leaving the reader where they were would
/// hide the thing they just asked for behind a rail they would have to go and find.
fn start_review(state: State, watch: AgentsWatch) {
    let (Some(name), Some(run_id)) = (watch.name.get(), watch.run_id.get()) else {
        return;
    };
    if watch.review_busy.get() {
        return;
    }
    watch.review_busy.set(true);
    spawn_local(async move {
        let result = fetch::review_run(name, run_id).await;
        watch.review_busy.set(false);
        match result {
            Ok(started) => {
                // The dossier's path is worth saying: it is a file on disk that outlives the flash,
                // and the one way to read the evidence without reading the review.
                state.flash.set(Some(Flash::ok(format!(
                    "\u{201C}{}\u{201D} is reviewing it \u{2014} evidence in {}",
                    started.reviewer, started.dossier
                ))));
                if started.run_id.is_empty() {
                    // An interactive reviewer keeps no run history, so there is no conversation to
                    // select — only its live pane to open.
                    open_watch(watch, started.reviewer, true);
                } else {
                    open_session(watch, &started.reviewer, &started.run_id);
                }
            }
            Err(e) => state.flash.set(Some(Flash::err(e))),
        }
    });
}

/// One count in the analytics rail: the number, what it counts, and a quiet breakdown under it when
/// there is one worth reading (an empty `sub` renders nothing, not an empty line).
fn stat_tile(label: &'static str, value: usize, sub: String) -> AnyView {
    text_tile(label, value.to_string(), sub)
}

/// The same tile for a value that is not a plain count — money, a duration, a token total.
fn text_tile(label: &'static str, value: String, sub: String) -> AnyView {
    view! {
        <div class="adi-chome__stat">
            <span class="adi-chome__stat-label">{label}</span>
            <span class="adi-chome__stat-val">{value}</span>
            {(!sub.is_empty()).then(|| view! {
                <span class="adi-chome__stat-sub">{sub}</span>
            })}
        </div>
    }
    .into_any()
}

/// A section of the rail whose rows go somewhere: the failed calls, the running ones. The count is in
/// the heading because the rows below it are the same information spelled out, and a reader who only
/// wants the number should not have to count.
fn jump_list(label: &'static str, badge: &'static str, status: &'static str, steps: Vec<StepRef>) -> AnyView {
    let n = steps.len();
    view! {
        <div class="adi-chome__group" data-status=status>
            <div class="adi-chome__group-head">{format!("{label} ({n})")}</div>
            {steps.into_iter().map(|s| {
                let anchor = s.anchor.clone();
                let title = format!("show this call in the transcript: {}", s.arg);
                view! {
                    <button class="adi-chome__jump" type="button" title=title
                        on:click=move |_| jump_to(&anchor)>
                        <span class="adi-chome__jump-badge">{badge}</span>
                        <span class="adi-chome__jump-name adi-mono">{s.tool}</span>
                        <span class="adi-chome__jump-arg adi-mono">{s.arg}</span>
                    </button>
                }
            }).collect_view()}
        </div>
    }
    .into_any()
}

/// Turns the engine gave up on. Distinct from a failed tool call, and worth its own line: a tool that
/// failed is something the agent saw and could work around, a turn that errored is one it never
/// finished.
///
/// `silent` are the ones that finished with nothing at all — no step and no word — which the
/// transcript therefore draws no bubble for. They are counted in the head and named on a line that
/// is deliberately not a button: there is nothing to jump to, and a link that scrolls nowhere reads
/// as the app being broken rather than as the turn being empty.
fn errored_list(anchors: Vec<String>, silent: usize) -> AnyView {
    let n = anchors.len() + silent;
    view! {
        <div class="adi-chome__group" data-status="error">
            <div class="adi-chome__group-head">{format!("Failed turns ({n})")}</div>
            {anchors.into_iter().enumerate().map(|(i, anchor)| view! {
                <button class="adi-chome__jump" type="button"
                    title="show this turn in the transcript"
                    on:click=move |_| jump_to(&anchor)>
                    <span class="adi-chome__jump-badge">"\u{26A0}"</span>
                    <span class="adi-chome__jump-name">{format!("turn {}", i + 1)}</span>
                </button>
            }).collect_view()}
            {(silent > 0).then(|| view! {
                <div class="adi-chome__jump adi-chome__jump--flat"
                    title="this turn produced no output to show">
                    <span class="adi-chome__jump-badge">"\u{26A0}"</span>
                    <span class="adi-chome__jump-name">
                        {if silent == 1 {
                            "1 turn, no output".to_string()
                        } else {
                            format!("{silent} turns, no output")
                        }}
                    </span>
                </div>
            })}
        </div>
    }
    .into_any()
}

/// Tools the agent tried to use and was not allowed to.
///
/// The engine reports these as names on a turn's metrics, not as steps, so there is no call in the
/// feed to point at — the rail names them and stops there. Worth surfacing anyway, and often the
/// answer to "why is the result wrong": an agent that was refused a write did not decide against it.
fn blocked_list(blocked: &[(String, usize)]) -> AnyView {
    let total: usize = blocked.iter().map(|(_, n)| n).sum();
    let names = blocked
        .iter()
        .map(|(name, n)| {
            if *n > 1 {
                format!("{name} \u{d7}{n}")
            } else {
                name.clone()
            }
        })
        .collect::<Vec<_>>()
        .join(" \u{b7} ");
    view! {
        <div class="adi-chome__group" data-status="blocked">
            <div class="adi-chome__group-head">{format!("Blocked ({total})")}</div>
            <div class="adi-chome__group-note adi-mono">{names}</div>
        </div>
    }
    .into_any()
}

/// Which tools did the work, most-used first — the shape of the run in a few lines. Whether a
/// conversation was a search, a refactor, or a build loop is legible here without reading any of it.
fn tool_breakdown(by_tool: &[(String, usize, usize)]) -> AnyView {
    view! {
        <div class="adi-chome__group">
            <div class="adi-chome__group-head">"Tools"</div>
            {by_tool.iter().map(|(name, calls, bad)| view! {
                <div class="adi-chome__toolrow" data-bad=(*bad > 0).then_some("1")>
                    <span class="adi-chome__toolrow-name adi-mono">{name.clone()}</span>
                    <span class="adi-spacer"></span>
                    {(*bad > 0).then(|| view! {
                        <span class="adi-chome__toolrow-bad">{format!("{bad} \u{2717}")}</span>
                    })}
                    <span class="adi-chome__toolrow-n adi-mono">{*calls}</span>
                </div>
            }).collect_view()}
        </div>
    }
    .into_any()
}

/// The context itemization at the foot of the rail: what this conversation's tokens went on, and
/// which runs of text it sent more than once.
///
/// Behind a button rather than loaded with the rest. Every other number here is arithmetic over a
/// transcript the page already holds; this one is a tokenizer pass over the whole conversation on the
/// server, and hanging that off a one-second poll would spend a core per open chat to answer a
/// question nobody asked each second.
fn chat_token_report(state: State, watch: AgentsWatch) -> AnyView {
    let run_id = watch.run_id.get();
    // A report belongs to the conversation it was taken from. Switching chats must show the button
    // again rather than the previous chat's numbers under a new heading.
    let ready = watch
        .tokens
        .get()
        .filter(|_| watch.tokens_of.get() == run_id && run_id.is_some());
    let busy = watch.tokens_busy.get();
    let error = watch.tokens_error.get();

    let body = match ready {
        None => view! {
            <button class="adi-chome__analyze" type="button" disabled=busy
                on:click=move |_| load_token_report(state, watch)>
                {if busy { "Reading the transcript\u{2026}" } else { "Itemize the context" }}
            </button>
            {(!error.is_empty()).then(|| view! {
                <div class="adi-chome__group-note">{error}</div>
            })}
        }
        .into_any(),
        Some(t) => token_report_view(state, watch, &t),
    };

    view! {
        <div class="adi-chome__group">
            <div class="adi-chome__group-head">"Context"</div>
            {body}
        </div>
    }
    .into_any()
}

/// A landed report: the total, where it went, and what was paid for twice.
fn token_report_view(state: State, watch: AgentsWatch, t: &AgentTokens) -> AnyView {
    let total = t.total;
    let split = t
        .by_source
        .iter()
        .filter(|s| total > 0 && s.tokens * 100 / total >= 1)
        .map(|s| format!("{} {}%", source_label(s.source), s.tokens * 100 / total))
        .collect::<Vec<_>>()
        .join(" \u{b7} ");
    // The share is the number that makes this actionable — "26k wasted" means nothing without the
    // total it is a fraction of.
    let share = if total > 0 { t.wasted * 100 / total } else { 0 };
    let repeats: Vec<AgentRepeat> = t.repeats.clone();
    let near: Vec<AgentNearDup> = t.near_duplicates.clone();

    view! {
        <div class="adi-chome__ctx">
            <span class="adi-chome__ctx-total adi-mono">{format!("\u{2248}{} tok", fmt_count(total as u64))}</span>
            // An OpenAI BPE counting a conversation that may have been had with any provider: close,
            // and honest about being an estimate rather than the provider's own accounting.
            <span class="adi-chome__ctx-enc adi-mono" title="estimated with a real BPE; not the provider's own count">
                {t.encoding.clone()}
            </span>
        </div>
        {(!split.is_empty()).then(|| view! {
            <div class="adi-chome__group-note">{split}</div>
        })}
        {t.truncated.then(|| view! {
            <div class="adi-chome__group-note">"long conversation \u{2014} only the recent end was read"</div>
        })}
        {(!repeats.is_empty()).then(|| view! {
            <div class="adi-chome__group-head">
                {format!("Sent twice \u{2014} {} tok ({share}%)", fmt_count(t.wasted as u64))}
            </div>
            {repeats.into_iter().map(repeat_row).collect_view()}
        })}
        {(!near.is_empty()).then(|| view! {
            <div class="adi-chome__group-head">{format!("Nearly the same ({})", near.len())}</div>
            {near.into_iter().map(near_dup_row).collect_view()}
        })}
        {(repeats_and_near_empty(t)).then(|| view! {
            <div class="adi-chome__group-note">"nothing was sent twice."</div>
        })}
        <button class="adi-chome__analyze" type="button" disabled=move || watch.tokens_busy.get()
            on:click=move |_| load_token_report(state, watch)>"Recount"</button>
    }
    .into_any()
}

/// Whether a report found nothing at all — which is a result, and has to be said, or an empty section
/// reads as one that failed to load.
fn repeats_and_near_empty(t: &AgentTokens) -> bool {
    t.repeats.is_empty() && t.near_duplicates.is_empty()
}

/// One repeated run: what it cost, how often it was sent, what it was, and what to do about it.
fn repeat_row(r: AgentRepeat) -> AnyView {
    let full = r.preview.clone();
    view! {
        <div class="adi-chome__rep" data-shape=shape_attr(r.shape)>
            <div class="adi-chome__rep-head">
                <span class="adi-chome__rep-cost adi-mono">{format!("{} tok", fmt_count(r.wasted as u64))}</span>
                <span class="adi-chome__rep-count adi-mono">{format!("{}\u{d7}", r.count)}</span>
                <span class="adi-spacer"></span>
                <span class="adi-chome__rep-shape">{shape_attr(r.shape)}</span>
            </div>
            <div class="adi-chome__rep-text adi-mono" title=full>{r.preview}</div>
            {(!r.hint.is_empty()).then(|| view! {
                <div class="adi-chome__rep-hint">{r.hint}</div>
            })}
        </div>
    }
    .into_any()
}

/// One group of near-identical sends — the same file read again after an edit, most often.
fn near_dup_row(g: AgentNearDup) -> AnyView {
    let full = g.preview.clone();
    view! {
        <div class="adi-chome__rep" data-shape="near">
            <div class="adi-chome__rep-head">
                <span class="adi-chome__rep-cost adi-mono">{format!("{} tok", fmt_count(g.wasted as u64))}</span>
                <span class="adi-chome__rep-count adi-mono">{format!("{}\u{d7}", g.count)}</span>
                <span class="adi-spacer"></span>
                <span class="adi-chome__rep-shape">{format!("\u{2248}{} tok each", fmt_count(g.tokens as u64))}</span>
            </div>
            <div class="adi-chome__rep-text adi-mono" title=full>{g.preview}</div>
        </div>
    }
    .into_any()
}

/// The word for a repeat's shape, used both as the label and as the styling hook.
fn shape_attr(s: AgentRepeatShape) -> &'static str {
    match s {
        AgentRepeatShape::Path => "path",
        AgentRepeatShape::Url => "url",
        AgentRepeatShape::Literal => "literal",
        AgentRepeatShape::Block => "block",
        AgentRepeatShape::Phrase => "text",
    }
}

/// The word for where a token came from.
fn source_label(s: AgentTokenSource) -> &'static str {
    match s {
        AgentTokenSource::User => "you",
        AgentTokenSource::Agent => "agent",
        AgentTokenSource::Thinking => "thinking",
        AgentTokenSource::ToolInput => "tool input",
        AgentTokenSource::ToolOutput => "tool output",
    }
}

/// Ask the server to itemize the open conversation's context.
fn load_token_report(state: State, watch: AgentsWatch) {
    let (Some(name), Some(run_id)) = (watch.name.get(), watch.run_id.get()) else {
        return;
    };
    if watch.tokens_busy.get() {
        return;
    }
    watch.tokens_busy.set(true);
    watch.tokens_error.set(String::new());
    spawn_local(async move {
        let result = fetch::run_tokens(name, run_id.clone()).await;
        watch.tokens_busy.set(false);
        match result {
            Ok(report) => {
                // Stamped with the run it describes, so a reply that lands after the reader has moved
                // to another conversation is shown against that one's title, not this one's.
                watch.tokens_of.set(Some(run_id));
                watch.tokens.set(Some(report));
            }
            Err(e) => {
                watch.tokens_error.set(e.clone());
                state.flash.set(Some(Flash::err(e)));
            }
        }
    });
}

/// Scroll the transcript to the element the rail is pointing at.
///
/// A step is one call *inside* a folded run, so the `<details>` around it is opened on the way:
/// arriving at a collapsed summary would make the link a gesture that appears to do nothing. It is
/// the ancestor that is opened rather than the target itself, because a run holds several calls
/// and only one of them is the one being pointed at. A turn anchor has no run above it, finds
/// nothing, and is simply scrolled to.
///
/// Silent when the element is gone — a run that has moved on between render and click is a race,
/// not an error to report.
fn jump_to(anchor: &str) {
    let Some(el) = web_sys::window()
        .and_then(|w| w.document())
        .and_then(|d| d.get_element_by_id(anchor))
    else {
        return;
    };
    if let Ok(Some(run)) = el.closest("details") {
        let _ = run.set_attribute("open", "");
    }
    el.scroll_into_view();
}

/// Open `which` as a drawer, or close it if it is the one already open.
///
/// Toggling rather than only opening is what makes the same button dismiss what it summoned, which
/// is the gesture a person tries first — the scrim and the ✕ are the other two.
fn toggle_drawer(state: State, which: ChatDrawer) {
    state.chat_drawer.update(|open| {
        *open = if *open == Some(which) { None } else { Some(which) };
    });
}

/// The agent chooser — which agent this screen is a chat with. It sits where a new chat is *started*
/// (the composer's own title, and the Start affordance a terminal agent shows in its place) rather
/// than over the sessions list, the way a model chooser sits with the composer it governs: the one
/// thing to settle before typing is who the message goes to.
///
/// **Starred agents only** — a fleet grows a long tail of one-off and machine-made agents, and this
/// is a short list to choose a chat from, not a register of everything installed. The agent already
/// on screen is always an option whether or not it is starred, so the control can't misreport what
/// the centre pane is showing; the Agents page is where anything else is reached (and starred).
///
/// The root agent leads, then the list's own name order, and one that is live right now carries a
/// dot. Choosing one repoints the conversation in the centre and the agent "+ New" starts a session
/// on; the rail lists every agent's sessions either way, so it only moves in that a different row of
/// it is now the open one.
fn chat_agent_picker(state: State, watch: AgentsWatch) -> AnyView {
    let Some(list) = state.agents.get() else {
        return view! { <span class="adi-chome__pickhint">"Loading agents…"</span> }.into_any();
    };
    let current = watch.name.get().unwrap_or_default();
    let mut agents: Vec<_> = list
        .agents
        .into_iter()
        .filter(|a| a.starred || a.name == current)
        .collect();
    if agents.is_empty() {
        return view! { <span class="adi-chome__pickhint">"No starred agents"</span> }.into_any();
    }
    // The root agent leads — it's the one the app is set up around. A stable sort, so the rest keep
    // the order the list arrived in (by name) behind it.
    agents.sort_by_key(|a| a.name != ROOT_AGENT);
    view! {
        <span class="adi-chome__agentpick">
            <select class="adi-chome__agentsel"
                title="which agent this chat runs on — starred agents only"
                prop:value=current.clone()
                on:change=move |ev| switch_agent(state, watch, event_target_value(&ev))>
                {agents.into_iter().map(|a| {
                    let selected = a.name == current;
                    // An option carries no markup, so the live dot rides in its text — worth
                    // spotting in a collapsed select, which is all you see of the other agents
                    // until you open it.
                    let label = if a.running {
                        format!("\u{25CF} {}", a.name)
                    } else {
                        a.name.clone()
                    };
                    view! { <option value=a.name selected=selected>{label}</option> }
                }).collect::<Vec<_>>()}
            </select>
            // The chevron the native control gives up under `appearance: none`, laid over the room
            // the select reserves for it.
            <span class="adi-chome__agentcaret" aria-hidden="true">"\u{25BE}"</span>
        </span>
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

/// When a session last moved, which is the time the whole rail reads and sorts by — a chat's "when"
/// is when it last said something, not when it opened. An older server sends no `last_activity`; its
/// runs then fall back to their start, which is what the field meant before it existed.
fn last_touch(r: &AgentRunInfo) -> u64 {
    r.last_activity.max(r.started_at)
}

/// Cut the *watched* agent's own run list to the page the rail is showing, by the rule the backend
/// pages the cross-agent index with: the newest [`State::rail_limit`], plus every session that is
/// running, blocked on a person, or starred, whatever its age.
///
/// The three exemptions have to match `newest` in `handlers/agents.rs` exactly. They are the same
/// rule applied to two lists, and a star that survived the server's cut only to be dropped by this
/// one would go missing from the rail of the agent you are actually on — which is the one agent
/// whose history is whole here, and so the only place the difference would show.
///
/// The rail takes the watched agent's conversations from `/api/agents/runs` rather than from the
/// index, because that list moves the instant a chat is deleted or hidden — and that endpoint
/// answers with the agent's *whole* history. Without this the agent you are actually on would be
/// the one agent paging did nothing for, which on this machine is the one with nearly every
/// session. `Load more` widens both, because it widens `rail_limit`.
///
/// Nothing is re-sorted: the endpoint answers newest first, so position is age.
fn paged(runs: Vec<AgentRunInfo>, state: State) -> Vec<AgentRunInfo> {
    let limit = state.rail_limit.get();
    if runs.len() <= limit {
        return runs;
    }
    runs.into_iter()
        .enumerate()
        .filter(|(i, r)| *i < limit || r.running || r.pending_question.is_some() || r.starred)
        .map(|(_, r)| r)
        .collect()
}

/// The rail's **starred only** toggle, in the Sessions head beside "+ New".
///
/// A fleet grows a long tail of one-off and machine-made agents, and their chats land in the same
/// flat list as the handful of agents actually worked with. This narrows the rail to the agents
/// starred on the Agents page — the same shortlist the agent picker already draws from, now
/// answering "show me only the sessions I care about" as well as "which agent does + New start".
///
/// Off by default: the rail opens on every conversation, so nothing a person had is missing until
/// they ask for it to be. The glyph fills and takes the accent while it is on, because a list showing
/// less than everything has to say so — and it is the only thing here that says it. Clicking it on
/// narrows the rail for as long as the page is open; the setting is not stored, so a reload comes
/// back to the full list.
fn chat_starred_button(state: State) -> AnyView {
    let on = state.starred_only.get();
    let (glyph, hint) = if on {
        ("\u{2605}", "showing starred agents only \u{2014} click to show every session")
    } else {
        ("\u{2606}", "show only sessions from starred agents")
    };
    view! {
        <button class="adi-chome__starred" class:is-on=on type="button"
            title=hint aria-label=hint aria-pressed=on.to_string()
            on:click=move |_| state.starred_only.update(|v| *v = !*v)>{glyph}</button>
    }
    .into_any()
}

/// Which agents the rail may list, or `None` when it may list them all.
///
/// `Some` only while [`State::starred_only`] is on *and* the agents list has arrived — without it
/// there is nothing to say which agents are starred, and narrowing on an empty answer would blank
/// the rail for a beat on every load. The watched agent is always in the set, so the filter can
/// never hide the conversation the centre pane is showing (the rule [`chat_agent_picker`] follows).
fn starred_agents(state: State, watched: &str) -> Option<std::collections::HashSet<String>> {
    if !state.starred_only.get() {
        return None;
    }
    let list = state.agents.get()?;
    let mut keep: std::collections::HashSet<String> = list
        .agents
        .into_iter()
        .filter(|a| a.starred)
        .map(|a| a.name)
        .collect();
    if !watched.is_empty() {
        keep.insert(watched.to_string());
    }
    Some(keep)
}

/// The whole left rail: every agent's sessions in one flat list, most recently updated first, and
/// the collapsed Hidden band under it.
///
/// One list rather than a band per agent, because "what was I just doing" is how a person looks for
/// a chat — so the agent a session belongs to is a line under its title rather than a heading to
/// scroll past. The picker above still names the agent "+ New" starts a session on; it no longer
/// decides what the rail shows.
///
/// A session the user has hidden ([`set_session_hidden`]) is left out of the list and rides only in
/// the band beneath it. Hiding is a rail preference, so it stops here: the Agents page's history
/// tables still list everything, which is what a workbench is for.
fn chat_rail(state: State, watch: AgentsWatch) -> AnyView {
    view! {
        {chat_all_sessions(state, watch)}
        {chat_load_more(state)}
        {chat_hidden_sessions(state, watch)}
    }
    .into_any()
}

/// The rail's **Load more**, under the last session and above the Hidden band: another
/// [`SESSION_PAGE`] sessions from the backend, and how many are left behind it.
///
/// `None` — no button at all — once the index says everything is here, which on a fresh machine is
/// from the first load. A control that does nothing is worse than no control, and "you have seen
/// them all" is what its absence says.
///
/// It widens the *request*, not a slice of something already in hand: [`State::rail_limit`] is what
/// the subscription's path is built from, so pressing this re-subscribes and the next page arrives
/// on its own. That is also why there is no spinner — the rail keeps every row it had and grows
/// when the answer lands, rather than emptying while it waits.
fn chat_load_more(state: State) -> Option<AnyView> {
    let all = state.all_chats.get()?;
    let shown: usize = all.agents.iter().map(|a| a.runs.len()).sum();
    let more = all.total.saturating_sub(shown);
    if more == 0 {
        return None;
    }
    // The last press says how many are left rather than repeating the page size back — "load 30
    // more · 30 older" is the same number twice, and the end of the list is worth saying plainly.
    let label = if more <= SESSION_PAGE {
        format!("Load the last {more}")
    } else {
        format!("Load {SESSION_PAGE} more \u{00b7} {more} older")
    };
    Some(
        view! {
            <button class="adi-chome__divider adi-chome__divider--toggle" type="button"
                title="ask the backend for the next page of sessions"
                on:click=move |_| state.rail_limit.update(|n| *n += SESSION_PAGE)>{label}</button>
        }
        .into_any(),
    )
}

/// One row of the rail. The list spans every agent, so a row has to carry which agent it belongs to
/// — there is no group heading above it to say so.
///
/// `Clone` because the rail lists rows through a keyed `For`, which owns its items.
#[derive(Clone)]
struct SessionRow {
    agent: String,
    /// The conversation, or `None` for an interactive agent's live pty session — which has no run
    /// id, being the agent's single session rather than one of many.
    run: Option<AgentRunInfo>,
    /// Unix millis this session last moved; what the rail sorts on.
    when: u64,
    running: bool,
    /// Kept deliberately: what puts the row in the **Starred** band and draws the ★ on it. Always
    /// false for a pty row, which has no record to carry a mark.
    starred: bool,
    /// Which number opens this row, counted down the whole rail rather than within its band —
    /// `None` past the ninth. Filled in by [`session_bands`] once the bands are settled, because
    /// until then there is no "third row" to be.
    hotkey: Option<usize>,
}

/// How many rows of the rail answer to a number.
///
/// Nine, because ⌘0 is not a tenth on any keyboard, and a row you have to count ten deep to reach
/// is one to search for rather than to number.
const HOTKEYS: usize = 9;

/// Which modifier a row prints, which is not always the one you would expect.
///
/// ⌘ only in the installed app. In a browser tab ⌘1…⌘8 belongs to the tab switcher and never
/// reaches the page at all, so printing ⌘ there would advertise a shortcut that cannot work — the
/// row says ⌃ instead, which does. [`install_session_hotkeys`] answers to both either way; this
/// only decides which one is worth the row's space. `display_override` puts `minimal-ui` behind
/// `standalone`, so both count as installed.
fn hotkey_glyph() -> &'static str {
    let installed = ["(display-mode: standalone)", "(display-mode: minimal-ui)"]
        .iter()
        .any(|q| {
            window()
                .match_media(q)
                .ok()
                .flatten()
                .is_some_and(|m| m.matches())
        });
    if installed { "\u{2318}" } else { "\u{2303}" }
}

/// Every visible session, whichever agent it belongs to, in the five bands the rail reads them in:
/// blocked on you, then running, then awaiting a wake, then starred, then the rest — each newest
/// activity first. The `bool` says whether the ★ *agent* filter is on, which is the difference
/// between the rail's two emptinesses.
///
/// The watched agent's conversations come from `watch.runs` when it has any — that list is updated
/// the moment a chat is deleted or hidden, so the rail doesn't go on showing a row that has just
/// gone — and from the cross-agent index otherwise. A pty agent keeps no run history, so it
/// contributes one row for its live session, sorted as though it moved just now: it is active by
/// definition and has no older timestamp to be placed by. That row shows while the session runs, or
/// while its agent is the one on screen — otherwise there is nothing there to open.
///
/// Split out of the view because [`install_session_hotkeys`] needs the same list, and needs it at
/// the moment a key is struck rather than the moment the rail was last drawn. One function, so the
/// number printed on a row and the row that number opens cannot drift apart.
fn session_bands(state: State, watch: AgentsWatch) -> ([Vec<SessionRow>; 5], bool) {
    let all = state.all_chats.get();
    let watched = watch.name.get().unwrap_or_default();
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
    let now = js_sys::Date::now() as u64;
    // `None` unless the head's ★ is on, in which case only these agents' sessions are listed.
    let keep = starred_agents(state, &watched);

    let mut rows: Vec<SessionRow> = Vec::new();
    let mut listed_watched = false;
    for ar in all.iter().flat_map(|a| a.agents.iter()) {
        if keep.as_ref().is_some_and(|k| !k.contains(&ar.name)) {
            continue;
        }
        let is_watched = ar.name == watched;
        listed_watched |= is_watched;
        if ar.interactive {
            let running = live.contains(&ar.name);
            if running || is_watched {
                rows.push(SessionRow {
                    agent: ar.name.clone(),
                    run: None,
                    when: now,
                    running,
                    starred: false,
                    hotkey: None,
                });
            }
            continue;
        }
        let runs = if is_watched {
            let own = watch.runs.get();
            if own.is_empty() { ar.runs.clone() } else { paged(own, state) }
        } else {
            ar.runs.clone()
        };
        rows.extend(runs.into_iter().filter(|r| !r.hidden).map(|r| SessionRow {
            agent: ar.name.clone(),
            when: last_touch(&r),
            running: r.running,
            starred: r.starred,
            run: Some(r),
            hotkey: None,
        }));
    }
    // The cross-agent index hasn't arrived yet (or doesn't carry this agent): the watch alone still
    // knows what is on screen, which is the one thing the rail must never be missing.
    if !listed_watched && !watched.is_empty() {
        if watch.interactive.get() {
            rows.push(SessionRow {
                agent: watched.clone(),
                run: None,
                when: now,
                running: watch.peek.get().is_some_and(|p| p.running),
                starred: false,
                hotkey: None,
            });
        } else {
            let own = paged(watch.runs.get(), state);
            rows.extend(own.into_iter().filter(|r| !r.hidden).map(|r| SessionRow {
                agent: watched.clone(),
                when: last_touch(&r),
                running: r.running,
                starred: r.starred,
                run: Some(r),
                hotkey: None,
            }));
        }
    }
    // Most recently updated first. A stable sort, so sessions that last moved at the same instant —
    // the live pty rows, all stamped "now" — keep the order the index listed them in.
    rows.sort_by(|a, b| b.when.cmp(&a.when));
    // Two bands, as in the playground: what is working right now, then everything else.
    // The counts are what the band heading is for — "how many are going" is the question the
    // rail is scanned for.
    // Three bands, not two. A conversation stopped on a question is neither working nor finished,
    // and it is the only kind you have to *do* something about — so it goes first, above what is
    // merely in progress. This is the whole of the "needs you" inbox: the rail is already the
    // cross-agent index, and a second surface fed by a second request would only be a copy of it
    // that could disagree.
    let (mut waiting, rows): (Vec<SessionRow>, Vec<SessionRow>) = rows
        .into_iter()
        .partition(|r| r.run.as_ref().is_some_and(|run| run.pending_question.is_some()));
    let (mut running, rows): (Vec<SessionRow>, Vec<SessionRow>) =
        rows.into_iter().partition(|r| r.running);
    // Then the ones that are coming back on their own. A conversation with a wake registered has
    // stopped, so it would otherwise fall into "Recent" and read as finished — which is the one
    // thing it is not. It sits below "Running now" because nothing is happening in it this second,
    // and above everything else because something is going to.
    //
    // Running wins the tie: a run working *and* holding a wake for what it launched is best found
    // where you look for what is working now, and the await is the smaller half of what it is doing.
    let (mut awaiting, rows): (Vec<SessionRow>, Vec<SessionRow>) = rows
        .into_iter()
        .partition(|r| r.run.as_ref().is_some_and(|run| !run.awaits.is_empty()));
    // Five, and the starred band comes *after* the three live ones rather than at the top. Waiting,
    // running and awaiting are states the conversation is in right now and will leave on its own; a
    // star is a standing instruction from a person. A starred chat that is working is still best
    // found under "Running now" — that is where you look for it today — so the band collects only
    // the ones the recency ordering would otherwise have carried away, which is the whole reason to
    // mark one.
    let (mut starred, mut rest): (Vec<SessionRow>, Vec<SessionRow>) =
        rows.into_iter().partition(|r| r.starred);
    // Numbered straight down the rail and across the band headings, not restarted per band: ⌘1 is
    // the row at the very top of the list whatever band it happens to be in today, which is the
    // only rule a hand can learn. Numbering within bands would move ⌘1 to a different session
    // every time the last question got answered.
    for (i, row) in waiting
        .iter_mut()
        .chain(running.iter_mut())
        .chain(awaiting.iter_mut())
        .chain(starred.iter_mut())
        .chain(rest.iter_mut())
        .take(HOTKEYS)
        .enumerate()
    {
        row.hotkey = Some(i + 1);
    }
    (
        [waiting, running, awaiting, starred, rest],
        keep.is_some(),
    )
}

/// The rail's session list: the five bands, or the one line that says why there are none.
fn chat_all_sessions(state: State, watch: AgentsWatch) -> AnyView {
    let ([waiting, running, awaiting, kept, rest], filtered) = session_bands(state, watch);
    if waiting.is_empty()
        && running.is_empty()
        && awaiting.is_empty()
        && kept.is_empty()
        && rest.is_empty()
    {
        // Which of the two emptinesses this is: nothing to show, or nothing left after the filter —
        // said apart, so the ★ never reads as "you have no chats".
        let msg = if filtered {
            "No chats from starred agents — star one on the Agents page, or turn ★ off."
        } else {
            "No chats yet — press New to start one."
        };
        return view! { <div class="adi-chome__empty">{msg}</div> }.into_any();
    }
    // Keyed, and that is not tidiness: a row's click handler is bound when the row is
    // *built*, so a plain list that is rebuilt with a different shape — which is exactly what
    // the ★ does — leaves handlers patched onto rows they no longer belong to, and a click
    // opens the session that used to be in that slot. `For` keys by identity, so a row and
    // its handler move together or not at all.
    //
    // Both bands are always emitted for the same reason: a band that comes and goes shifts
    // every slot after it.
    let band = move |label: &'static str, rows: Vec<SessionRow>| {
        let n = rows.len();
        let any = n > 0;
        let rows = StoredValue::new(rows);
        view! {
            <Show when=move || any>
                <adi_ui::RailGroup label=label count=n>
                    <For
                        // Stored, so the closure can hand out a fresh copy on every read
                        // instead of moving the one it has.
                        each=move || rows.get_value()
                        key=|row: &SessionRow| {
                            format!(
                                "{}:{}",
                                row.agent,
                                row.run.as_ref().map_or("", |r| r.run_id.as_str()),
                            )
                        }
                        let:row
                    >
                        {chat_session_row(state, watch, row)}
                    </For>
                </adi_ui::RailGroup>
            </Show>
        }
        .into_any()
    };
    vec![
        band("Waiting on you", waiting),
        band("Running now", running),
        band("Awaiting", awaiting),
        band("Starred", kept),
        band("Recent", rest),
    ]
    .into_any()
}

/// One session in the rail: its task, then the agent it belongs to and when it last moved. Clicking
/// opens it — repointing the whole screen when it belongs to another agent, and only selecting the
/// conversation when it is already the picked one, so a click on a chat of the agent on screen
/// doesn't tear the centre pane down and rebuild it. Right-clicking offers to hide or star it, and a
/// star and a delete ride the row's right edge. The first nine rows also carry the number that opens
/// them.
fn chat_session_row(state: State, watch: AgentsWatch, item: SessionRow) -> AnyView {
    let SessionRow { agent, run, when, running, starred, hotkey } = item;
    let on_this_agent = watch.name.get().as_deref() == Some(agent.as_str());
    let waiting = run.as_ref().is_some_and(|r| r.pending_question.is_some());
    // What it is waiting on the world for. The row says it with the dot and the band it is under,
    // and the rest goes in the tooltip — the meta line's parts are all `shrink-0` inside an
    // `overflow-hidden`, so a third one does not shrink to fit the rail, it clips mid-word.
    let awaits = run.as_ref().map(|r| r.awaits.len()).unwrap_or(0);
    // The tooltip is where there is room for the sentence, and it is the same sentence the All
    // chats table hangs on its status cell — two surfaces showing one conversation should not
    // describe it two ways.
    let await_hint = run
        .as_ref()
        .map(|r| awaiting_hint(&r.awaits))
        .filter(|hint| !hint.is_empty())
        .map(|hint| format!("\n\n{hint}"))
        .unwrap_or_default();
    let (title, sub, run_id) = match run {
        Some(r) => {
            let t = truncate_task(&r.message);
            let t = if t.trim().is_empty() { "New chat".to_string() } else { t };
            (t, format!("{agent} \u{00b7} {}", run_age(when)), r.run_id)
        }
        None => (
            if running { "Live session" } else { "No live session" }.to_string(),
            format!("{agent} \u{00b7} interactive terminal"),
            String::new(),
        ),
    };
    // A pty session has no run id, so the agent being watched is the whole of "this row is open".
    let is_sel = on_this_agent
        && (run_id.is_empty() && watch.interactive.get()
            || !run_id.is_empty() && watch.run_id.get().as_deref() == Some(run_id.as_str()));
    // The tooltip names both, whatever the row prints: whichever of the two the browser keeps for
    // itself, the other one is the way in, and here there is room to say so.
    let hint = match hotkey {
        Some(n) => {
            format!("open this session with {agent} \u{2014} \u{2318}{n} or Ctrl+{n}{await_hint}")
        }
        None => format!("open this session with {agent}{await_hint}"),
    };
    let menu = SessionRef::of(&agent, &run_id, &title, false, starred);
    // Only a conversation can be deleted: a pty agent's live session is started and stopped from the
    // centre pane, and keeps no transcript to take with it.
    let del = (!run_id.is_empty()).then(|| {
        let (del_agent, del_id, del_title) = (agent.clone(), run_id.clone(), title.clone());
        view! {
            <button class="adi-chome__session-del" type="button"
                title="delete this chat and its transcript"
                on:click=move |_| delete_one_run(
                    state, watch, del_agent.clone(), del_id.clone(), del_title.clone(),
                )>"\u{2715}"</button>
        }
    });
    // The star toggle, on the same edge and by the same rule — a pty row has no record to mark.
    //
    // Unlike the delete beside it, it does not wait for a hover once the chat *is* starred: the
    // mark is the row's state as much as its control, and a keep that only showed under the cursor
    // would be a keep you had to go looking for to confirm.
    let star = (!run_id.is_empty()).then(|| {
        let (star_agent, star_id) = (agent.clone(), run_id.clone());
        let (glyph, hint) = if starred {
            ("\u{2605}", "unstar this chat")
        } else {
            ("\u{2606}", "star this chat \u{2014} it stays in the rail and outlives the session cap")
        };
        view! {
            <button class="adi-chome__session-star" class:is-on=starred type="button"
                title=hint aria-label=hint aria-pressed=starred.to_string()
                on:click=move |_| set_session_starred(
                    state, watch, star_agent.clone(), star_id.clone(), !starred,
                )>{glyph}</button>
        }
    });
    // The row itself is `adi-ui`; the delete control is laid over it rather than inside,
    // because the row is one hit target and a button inside a button is not a thing a
    // browser will do. It appears on hover, where it cannot be hit by accident.
    // Waiting outranks working. A conversation with a question up is stopped on *you*, and the
    // one thing the rail exists to answer is which of forty rows needs you — `running: false`
    // alone cannot say it, because finished and blocked-on-you look identical from there.
    //
    // Awaiting comes last of the three for the opposite reason: it is the only live state that asks
    // nothing of anybody, so it yields to both a question and a turn in flight. A run working and
    // holding a wake for what it launched is a working run.
    let state_of = if waiting {
        adi_ui::SessionState::Waiting
    } else if running {
        adi_ui::SessionState::Working
    } else if awaits > 0 {
        adi_ui::SessionState::Awaiting
    } else {
        adi_ui::SessionState::Done
    };
    let sub = sub.clone();
    // A shortcut nobody can see is a shortcut nobody uses, so the number rides the row it opens.
    // One modifier and one digit, never "⌘1 or Ctrl+1": the row has to stay readable at a glance,
    // and the long form is already in the tooltip.
    //
    // It fades out under the cursor because the delete control lands in the same corner — and a
    // hand already on the mouse has no use for a keyboard shortcut anyway.
    let cap = hotkey.map(|n| {
        view! {
            <span class="ml-auto transition-opacity group-hover:opacity-0">
                <adi_ui::Kbd>{format!("{}{n}", hotkey_glyph())}</adi_ui::Kbd>
            </span>
        }
    });
    view! {
        // Right-click still offers the row's menu, and it is on the wrapper so it covers
        // the whole row including the delete control's corner.
        <div
            class="group relative"
            on:contextmenu=move |ev: web_sys::MouseEvent| menu.open(state, &ev)
        >
            <adi_ui::SessionItem
                title=title.clone()
                state=state_of
                agent=sub
                // The only coloured words in the row, and spent on the one thing a rail is
                // scanned for. What it wants, not how much of it there is.
                //
                // An await gets none, though it is the one state here that could have used one: it
                // asks for nothing, so there is nothing for the row to say it wants. Its dot is
                // blue and its band is named, which between them is the whole of the news.
                alert=if waiting { "your answer" } else { "" }
                selected=is_sel
                attr:title=hint
                on:click=move |_| {
                    if run_id.is_empty() {
                        point_watch(watch, agent.clone(), true);
                    } else {
                        open_session(watch, &agent, &run_id);
                    }
                    // Picking a session is the drawer's whole purpose, so it gets out of the
                    // way — otherwise the chat you just chose opens behind the list you chose
                    // it from. Inert on a wide viewport, where nothing is ever open.
                    state.chat_drawer.set(None);
                }
            >
                {cap}
            </adi_ui::SessionItem>
            // Two controls in the same corner, each in its own anchor rather than in one flex row:
            // both buttons are `position: absolute` against the anchor they are given, so a row of
            // them would stack rather than sit side by side. The star is the outer of the two, and
            // keeps its place whether the delete beside it is showing or not.
            <div class=if starred {
                "absolute top-1 right-7 transition-opacity"
            } else {
                "absolute top-1 right-7 opacity-0 transition-opacity \
                 group-hover:opacity-100 focus-within:opacity-100"
            }>
                {star}
            </div>
            <div class="absolute top-1 right-1 opacity-0 transition-opacity \
                        group-hover:opacity-100 focus-within:opacity-100">
                {del}
            </div>
        </div>
    }
    .into_any()
}

/// Which session a row's right-click menu would act on, packaged so a row can hand it to the menu
/// without threading four fields (and four clones) through each `on:contextmenu` closure.
#[derive(Clone)]
struct SessionRef {
    agent: String,
    /// Empty for an interactive agent's live pty session — it is not a run, keeps no per-run record,
    /// and so has nothing to hide, which is why such a row opens no menu at all.
    run_id: String,
    title: String,
    hidden: bool,
    starred: bool,
}

impl SessionRef {
    fn of(agent: &str, run_id: &str, title: &str, hidden: bool, starred: bool) -> Self {
        Self {
            agent: agent.to_string(),
            run_id: run_id.to_string(),
            title: title.to_string(),
            hidden,
            starred,
        }
    }

    /// Open the rail's menu over this row, anchored at the pointer. A row with no run behind it
    /// declines — the browser's own menu shows instead of an empty one of ours.
    fn open(&self, state: State, ev: &web_sys::MouseEvent) {
        if self.run_id.is_empty() {
            return;
        }
        ev.prevent_default();
        state.session_menu.set(Some(SessionMenu {
            agent: self.agent.clone(),
            run_id: self.run_id.clone(),
            title: self.title.clone(),
            hidden: self.hidden,
            starred: self.starred,
            x: ev.client_x(),
            y: ev.client_y(),
        }));
    }
}

/// The rail's right-click menu on a session: which chat it is, then Star (or Unstar) and Hide (or,
/// for one already put away, Unhide). A full-viewport scrim behind it makes the next click a
/// dismiss, as on the store tree's menu.
///
/// Star comes first because it is the one of the two that is reached from here rather than from the
/// row — the row's own edge carries a star, but the Hidden band's rows have no edge control for it
/// and this menu is the whole of their way to one.
fn chat_session_menu(state: State, watch: AgentsWatch) -> Option<AnyView> {
    let menu = state.session_menu.get()?;
    let SessionMenu { agent, run_id, title, hidden, starred, x, y } = menu;
    let hide_label = if hidden { "Unhide" } else { "Hide" };
    let star_label = if starred { "Unstar" } else { "Star" };
    let head = format!("{title} \u{00b7} {agent}");
    let (star_agent, star_id) = (agent.clone(), run_id.clone());
    Some(
        view! {
            <div class="adi-menu__scrim"
                on:click=move |_| state.session_menu.set(None)
                on:contextmenu=move |ev: web_sys::MouseEvent| {
                    ev.prevent_default();
                    state.session_menu.set(None);
                }></div>
            <div class="adi-menu" style=format!("left:{x}px; top:{y}px")>
                <div class="adi-menu__head" title=head.clone()>{head.clone()}</div>
                <button class="adi-menu__item" type="button"
                    on:click=move |_| set_session_starred(
                        state, watch, star_agent.clone(), star_id.clone(), !starred,
                    )>{star_label}</button>
                <button class="adi-menu__item" type="button"
                    on:click=move |_| set_session_hidden(
                        state, watch, agent.clone(), run_id.clone(), !hidden,
                    )>{hide_label}</button>
            </div>
        }
        .into_any(),
    )
}

/// Hide a session from the rail, or bring it back. See [`settle_session_flag`] for what happens to
/// the answer.
///
/// Nothing is stopped and nothing is deleted — a hidden run keeps working and keeps its transcript.
/// The one thing that does move is the centre pane: hiding the conversation on screen closes it,
/// since a chat still open after being put away is a puzzle about where it went.
fn set_session_hidden(
    state: State,
    watch: AgentsWatch,
    agent: String,
    run_id: String,
    hidden: bool,
) {
    state.session_menu.set(None);
    if run_id.is_empty() {
        return;
    }
    if hidden
        && watch.name.get_untracked().as_deref() == Some(agent.as_str())
        && watch.run_id.get_untracked().as_deref() == Some(run_id.as_str())
    {
        close_run_view(watch);
    }
    spawn_local(async move {
        let flagged = fetch::hide_run(agent.clone(), run_id, hidden).await;
        settle_session_flag(state, watch, &agent, flagged).await;
    });
}

/// Star a conversation, or unstar it — the other flag on the same row, through the same refresh.
///
/// Nothing moves but the mark: the chat stays open, stays in the rail, and keeps its place in
/// whichever band it was in. What it buys is out of sight from here — a starred conversation is
/// exempt from the per-agent session cap, so this is also how a chat is kept past the fifty newest
/// rather than swept the next time its agent is run.
fn set_session_starred(
    state: State,
    watch: AgentsWatch,
    agent: String,
    run_id: String,
    starred: bool,
) {
    state.session_menu.set(None);
    if run_id.is_empty() {
        return;
    }
    spawn_local(async move {
        let flagged = fetch::star_run(agent.clone(), run_id, starred).await;
        settle_session_flag(state, watch, &agent, flagged).await;
    });
}

/// What both of the rail's flag toggles do with their answer.
///
/// The endpoint replies with that agent's fresh history, so the row settles into its new band at
/// once instead of at the socket's next tick — and the cross-agent index the Starred, Recent and
/// Hidden bands read is re-fetched for the same reason. Refetching it **at the rail's current
/// page**, not without a limit: an unlimited answer would widen the rail to the whole index until
/// the socket's next answer narrowed it back.
async fn settle_session_flag(
    state: State,
    watch: AgentsWatch,
    agent: &str,
    flagged: Result<AgentRuns, String>,
) {
    match flagged {
        Ok(runs) => {
            if watch.name.get_untracked().as_deref() == Some(agent) {
                watch.runs.set(runs.runs);
            }
            let limit = Some(state.rail_limit.get_untracked());
            if let Ok(all) = fetch::all_agent_runs(limit).await {
                state.all_chats.set(Some(all));
            }
        }
        Err(e) => state.flash.set(Some(Flash::err(e))),
    }
}

/// Bind ⌘1…⌘9 to the first nine rows of the sessions rail, in the order the rail reads them.
///
/// The row is looked up when the key is struck, not when the rail was drawn. The rail redraws
/// whenever anything moves, and a list captured at draw time would go on opening whatever *used* to
/// be third after a run finished and the bands resorted under it — the same trap the keyed `For` in
/// [`chat_all_sessions`] exists to avoid, arrived at from the other side.
///
/// Ctrl as well as ⌘, and not for symmetry with other platforms: Chrome and Safari spend ⌘1…⌘8 on
/// "switch to tab N" and never hand them to the page in an ordinary tab. ⌘ is the shortcut in the
/// installed app, where there are no tabs to switch to; Ctrl is the way in from a browser tab, where
/// there are. Only one of the two ever needs to work on a given screen.
fn install_session_hotkeys(state: State, watch: AgentsWatch) {
    let handle = window_event_listener(leptos::ev::keydown, move |ev| {
        if !(ev.meta_key() || ev.ctrl_key()) || ev.alt_key() || ev.shift_key() {
            return;
        }
        // `code`, not `key`: with a modifier held, layouts that put a symbol on the number row
        // report *that* — and which chat opens must not depend on the keyboard.
        let code = ev.code();
        let Some(n) = code
            .strip_prefix("Digit")
            .or_else(|| code.strip_prefix("Numpad"))
            .and_then(|d| d.parse::<usize>().ok())
            .filter(|n| (1..=HOTKEYS).contains(n))
        else {
            return;
        };
        // Nothing is claimed unless there really is a row there, so ⌘4 on a three-chat rail stays
        // the browser's to handle rather than being swallowed into a no-op.
        let Some(row) = session_bands(state, watch).0.into_iter().flatten().nth(n - 1) else {
            return;
        };
        ev.prevent_default();
        match row.run {
            Some(run) => open_session(watch, &row.agent, &run.run_id),
            // A pty agent's row *is* the agent — there is no run to select, only a screen to point.
            None => point_watch(watch, row.agent, true),
        }
        // As with a click: on a narrow viewport the rail is a drawer laid over the chat, and the
        // chat you just picked would open behind the list you picked it from.
        state.chat_drawer.set(None);
    });
    on_cleanup(move || handle.remove());
}

/// Open one session from anywhere in the rail: repoint the whole screen when it belongs to another
/// agent, or only select the conversation when it is already the picked one — so a click on a chat of
/// the agent already on screen doesn't tear the centre pane down and rebuild it.
fn open_session(watch: AgentsWatch, agent: &str, run_id: &str) {
    if watch.name.get_untracked().as_deref() == Some(agent) {
        select_run(watch, run_id.to_string());
    } else {
        point_conversation(watch, agent.to_string(), run_id.to_string(), false);
    }
}

/// The rail's **Hidden** band: every session put away with Hide, whichever agent it belongs to,
/// newest first. `None` when nothing is hidden, so a rail no one has hidden anything in never
/// mentions the idea.
///
/// Collapsed behind its own count, because this band is the way *back* to a session rather than
/// something to read: a click opens the chat as any other row does, and its right-click menu offers
/// Unhide — as does the ↩ that rides the row's right edge.
fn chat_hidden_sessions(state: State, watch: AgentsWatch) -> Option<AnyView> {
    let all = state.all_chats.get()?;
    // The ★ narrows this band with the list above it — one filter over the whole rail, or unhiding
    // would offer chats the rail has just been told not to show.
    let keep = starred_agents(state, &watch.name.get().unwrap_or_default());
    let mut rows: Vec<(String, AgentRunInfo)> = all
        .agents
        .iter()
        .filter(|ar| keep.as_ref().is_none_or(|k| k.contains(&ar.name)))
        .flat_map(|ar| {
            ar.runs
                .iter()
                .filter(|r| r.hidden)
                .map(|r| (ar.name.clone(), r.clone()))
        })
        .collect();
    if rows.is_empty() {
        return None;
    }
    rows.sort_by_key(|(_, r)| std::cmp::Reverse(last_touch(r)));
    let open = state.show_hidden.get();
    let label = format!(
        "{} Hidden \u{00b7} {}",
        if open { "\u{25be}" } else { "\u{25b8}" },
        rows.len()
    );
    let body = open.then(|| {
        rows.into_iter()
            .map(|(agent, r)| chat_hidden_row(state, watch, &agent, &r))
            .collect::<Vec<_>>()
    });
    Some(
        view! {
            <button class="adi-chome__divider adi-chome__divider--toggle" type="button"
                title="sessions hidden from the rail"
                aria-expanded=open.to_string()
                on:click=move |_| state.show_hidden.update(|v| *v = !*v)>{label}</button>
            {body}
        }
        .into_any(),
    )
}

/// One hidden session: the same strip as any other agent's, dimmed under the Hidden band, with an
/// unhide (↩) at its right edge in place of the delete — putting a chat back is the one thing this
/// band is for, so it doesn't hide behind the right-click menu.
fn chat_hidden_row(state: State, watch: AgentsWatch, agent: &str, r: &AgentRunInfo) -> AnyView {
    let title = truncate_task(&r.message);
    let title = if title.trim().is_empty() { "New chat".to_string() } else { title };
    let sub = format!("{agent} \u{00b7} {}", run_age(last_touch(r)));
    let dot = if r.running { "adi-chome__dot adi-chome__dot--on" } else { "adi-chome__dot" };
    let hint = format!("open this hidden chat with {agent}");
    let menu = SessionRef::of(agent, &r.run_id, &title, true, r.starred);
    let (open_name, open_id) = (agent.to_string(), r.run_id.clone());
    let (show_name, show_id) = (agent.to_string(), r.run_id.clone());
    view! {
        <div class="adi-chome__sessionrow">
            <button class="adi-chome__session adi-chome__session--hidden"
                type="button" title=hint
                on:click=move |_| open_session(watch, &open_name, &open_id)
                on:contextmenu=move |ev: web_sys::MouseEvent| menu.open(state, &ev)>
                <span class=dot></span>
                <span class="adi-chome__session-main">
                    <span class="adi-chome__session-title">{title}</span>
                    <span class="adi-chome__session-when">{sub}</span>
                </span>
            </button>
            <button class="adi-chome__session-unhide" type="button"
                title="bring this chat back into the rail"
                on:click=move |_| set_session_hidden(
                    state, watch, show_name.clone(), show_id.clone(), false,
                )>"\u{21a9}"</button>
        </div>
    }
    .into_any()
}

/// The sessions rail's "New" action: for a pty agent it (re)starts the live session; for a headless
/// one it clears the selection so the centre shows the new-conversation composer. Either way it acts
/// on the *picked* agent — the one the rail's picker names, which is what the picker is still for
/// now that the list below it spans every agent.
fn chat_new_button(state: State, watch: AgentsWatch) -> AnyView {
    if watch.interactive.get() {
        let name = watch.name.get().unwrap_or_default();
        view! {
            <button class="adi-btn adi-btn--ghost adi-chome__new" type="button"
                title="start a fresh session"
                on:click=move |_| run_now(state, name.clone(),
                    at_run_limit(state.agents.get_untracked().as_ref(), &name))>"+ New"</button>
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

/// The centre pane: a pty agent's live terminal (+ input), or a headless agent's selected
/// conversation (or the new-conversation composer when nothing is selected).
fn chat_center(state: State, watch: AgentsWatch) -> AnyView {
    let Some(name) = watch.name.get() else {
        return view! { <div class="adi-chome__center-empty">"Connecting…"</div> }.into_any();
    };
    if watch.interactive.get() {
        chat_center_pty(state, watch, name)
    } else {
        chat_center_headless(state, watch)
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
                                // The same chooser the composer carries: a terminal agent has no
                                // composer to hang it under, and this is where its session starts.
                                <p class="adi-chome__pickline">
                                    "Session with "
                                    {move || chat_agent_picker(state, watch)}
                                </p>
                                <button class="adi-btn adi-btn--primary" type="button"
                                    on:click=move |_| run_now(state, name.clone(),
                                        at_run_limit(state.agents.get_untracked().as_ref(), &name))>
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

/// The headless centre: the selected conversation's transcript (+ reply), or — when no session is
/// selected — the home screen: a composer to start a new one, and under it whatever is stopped
/// waiting on you ([`chat_inbox`]). The composer's title *is* the agent chooser — with several
/// agents to pick between, which one a new chat goes to is both the thing worth saying out loud
/// and the thing worth being able to change right there.
fn chat_center_headless(state: State, watch: AgentsWatch) -> AnyView {
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
                                {move || chat_agent_picker(state, watch)}
                            </h2>
                        </div>
                        {run_bar(state, watch)}
                        // Its own closure, and that is not style: this arm holds the composer, and
                        // reading the session list out here would put every session update in the
                        // dependencies of the tree the message is being typed into.
                        {move || chat_inbox(state, watch)}
                    </div>
                }
                .into_any(),
            }}
        </div>
    }
    .into_any()
}

/// How many conversations the inbox lists before it stops and says how many are left.
///
/// Six: this sits under the composer, which is the thing you came to this screen to use, and a list
/// that grew without limit would push it off the top of the pane. The rail beside it holds the
/// rest — and lists them under the same heading, in the same order.
const INBOX_ROWS: usize = 6;

/// Under the composer: **the conversations stopped waiting on you**, and nothing else.
///
/// The rail beside it already lists every session, banded — running, awaiting, starred, recent — so
/// anything else here would be a second copy of the rail, drawn wider, on the one screen where
/// there is nothing to compare it against. The exception is this band, because it is not a list but
/// an *inbox*: those conversations have stopped until a person answers them, and that is the one
/// thing worth putting in front of the person rather than off to the side of them.
///
/// Drawn from [`session_bands`], the same list the rail is built from, so the two cannot disagree
/// about a session; and the rows are the rail's own ([`chat_session_row`]), so a session opens,
/// stars, deletes and right-clicks here exactly as it does over there.
///
/// `None` when nothing is waiting — which is the normal state of a machine, and then the composer
/// keeps the pane to itself. Nothing asking is not news, and a panel saying so every day is how a
/// panel stops being read on the day it has something to say.
fn chat_inbox(state: State, watch: AgentsWatch) -> Option<AnyView> {
    // Only the first band. The other four are the rail's business.
    let ([waiting, ..], _filtered) = session_bands(state, watch);
    if waiting.is_empty() {
        return None;
    }
    let total = waiting.len();
    let more = total.saturating_sub(INBOX_ROWS);
    // Keyed, for the reason the rail's list is: a click handler is bound when its row is *built*,
    // so a positional rebuild — which is what this list gaining or losing a row is — would leave
    // the handler of the session that used to be in that slot on the one now drawn there.
    let shown = StoredValue::new(waiting.into_iter().take(INBOX_ROWS).collect::<Vec<_>>());
    Some(
        view! {
            <div class="adi-chome__inbox adi-ui-type">
                <adi_ui::RailGroup label="Waiting on you" count=total>
                    <For
                        each=move || shown.get_value()
                        key=|row: &SessionRow| {
                            format!(
                                "{}:{}",
                                row.agent,
                                row.run.as_ref().map_or("", |r| r.run_id.as_str()),
                            )
                        }
                        let:row
                    >
                        {chat_session_row(state, watch, row)}
                    </For>
                </adi_ui::RailGroup>
                {(more > 0).then(|| view! {
                    <p class="adi-chome__inbox-more">
                        {format!("+{more} more in the sessions rail")}
                    </p>
                })}
            </div>
        }
        .into_any(),
    )
}

/// The dashboards rail: **what this machine runs, then what its fleet does.**
///
/// The local half is grouped by the project a dashboard is filed under; the fleet half is grouped
/// by node ([`chat_fleet_groups`]). One rail rather than two because "which dashboards can I open?"
/// is one question — where a dashboard happens to run is a property of the row, not a reason to go
/// looking somewhere else for it.
///
/// The empty note is shown only when *both* halves are empty, so a machine that runs nothing itself
/// but has a node full of dashboards does not claim there are none.
fn chat_dashboards(state: State) -> AnyView {
    let loading = state.dashboards.get().is_none();
    let mut out = chat_local_groups(state);
    let fleet = chat_fleet_groups(state);
    let empty = out.is_empty() && fleet.is_empty();
    out.extend(fleet);
    if empty {
        return view! {
            <div class="adi-chome__empty">
                {if loading {
                    "Loading…"
                } else {
                    "No dashboards yet — ask your agent to build one."
                }}
            </div>
        }
        .into_any();
    }
    out.into_any()
}

/// The rail's local half: every live dashboard on *this* machine, grouped by the project it's filed
/// under (a header per project that has at least one, then an "Ungrouped" bucket), each a link to
/// open it when its frontend is up. Project names come from `state.projects`; an unknown/archived
/// project id folds into Ungrouped.
///
/// Empty while the listing is still loading, and empty when there is nothing to list — the caller
/// is what tells those two apart, because it also knows what the fleet said.
fn chat_local_groups(state: State) -> Vec<AnyView> {
    let Some(ds) = state.dashboards.get() else {
        return Vec::new();
    };
    let live: Vec<Dashboard> = ds.dashboards.into_iter().filter(|d| !d.is_archived()).collect();
    if live.is_empty() {
        return Vec::new();
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
        let n = items.len();
        let rows: Vec<AnyView> = items
            .into_iter()
            .map(|d| {
                placed.insert(d.id.clone());
                chat_dash_item(d)
            })
            .collect();
        out.push(
            view! { <adi_ui::RailGroup label=p.name.clone() count=n>{rows}</adi_ui::RailGroup> }
                .into_any(),
        );
    }
    // Whatever's left — unfiled, or filed under a project that no longer exists — trails as one
    // Ungrouped bucket. Only labelled when it sits alongside real groups.
    let rest: Vec<&Dashboard> = live.iter().filter(|d| !placed.contains(&d.id)).collect();
    if !rest.is_empty() {
        let n = rest.len();
        let rows: Vec<AnyView> = rest.into_iter().map(chat_dash_item).collect();
        // Only labelled when it sits alongside real groups; on its own it is just the list.
        let label = if out.is_empty() { "" } else { "Ungrouped" };
        out.push(
            view! { <adi_ui::RailGroup label=label count=n>{rows}</adi_ui::RailGroup> }.into_any(),
        );
    }
    out
}

/// One dashboard row in the rail: a link to its running frontend, or a dimmed row when it's down.
///
/// The address comes from [`open_url`](crate::pages::dashboards::open_url), the same rule the
/// Dashboards page uses: the dashboard's own host when it has one, loopback only as the fallback.
/// A dashboard is one origin now, so a `127.0.0.1:<port>` link bypasses the front door and the
/// page's `/api` calls stop routing — it renders, and only then falls over.
fn chat_dash_item(d: &Dashboard) -> AnyView {
    // An address is the whole of "is this up?": `open_url` returns one only for a dashboard
    // whose frontend is running. With one, the row is a link and opens in its own tab —
    // a dashboard is its own origin. Without one, it is a dead row with a red dot and
    // nothing to click.
    let (state, href) = match crate::pages::dashboards::open_url(d) {
        Some(href) => (adi_ui::AppState::Live, href),
        None => (adi_ui::AppState::Offline, String::new()),
    };
    view! {
        <adi_ui::AppItem
            title=d.name.clone()
            state=state
            href=href
            blank=true
            // Empty machine: a local dashboard runs right here, and the row says so itself.
            machine=""
        />
    }
    .into_any()
}

// ---- the fleet's half of the rail ---------------------------------------------------------
// What the *other* machines run, asked of each node's own control panel over the mesh
// (`docs/fleet.md` §5, and `adi-app/src/viewer.rs` for why it is the panel that is asked). Three
// things can be true of a node and each wants something different from the reader: it is locked
// (this machine has no password for it), it refused (a sentence saying why), or it answered.

/// One group per paired node, under a rule that says the rest of the rail is somewhere else.
///
/// That rule is load-bearing rather than decorative: a node's header and a project's are the same
/// band, so without it `TEREMEC` and `BUGBOUNTY` read as two projects, and the one thing this half
/// of the rail exists to say — these run on another machine — would be the one thing it does not.
///
/// Empty until the first listing arrives, and empty forever on a machine paired with nobody, which
/// is what keeps the whole half invisible for anyone not running a fleet.
fn chat_fleet_groups(state: State) -> Vec<AnyView> {
    let Some(fleet) = state.fleet_dashboards.get() else {
        return Vec::new();
    };
    if fleet.nodes.is_empty() {
        return Vec::new();
    }
    let mut out = vec![
        view! { <div class="mt-4 border-t border-divider pt-1"></div> }.into_any(),
    ];
    out.extend(fleet.nodes.iter().map(|n| chat_node_group(state, n)));
    out
}

/// One node: its header, then whatever it had to say — a password prompt, a refusal, or its
/// dashboards.
fn chat_node_group(state: State, node: &NodeDashboards) -> AnyView {
    let name = node.node.clone();
    let unlock = state.fleet_unlock;
    let (locked, opening) = (node.locked, unlock.node.get() == node.node);
    let open_on = node.node.clone();
    let forget = node.node.clone();

    // The header's trailing control is the one thing that can be done to the node from here:
    // hand it a password, or take one back.
    let action = if locked {
        let label = if opening { "Cancel" } else { "Unlock" };
        view! {
            <button class="adi-chome__group-act" type="button"
                title="This machine has no password for this node, so it cannot ask what it runs."
                on:click=move |_| if unlock.node.get_untracked() == open_on {
                    unlock.close();
                } else {
                    unlock.open(&open_on);
                }>
                {label}
            </button>
        }
        .into_any()
    } else {
        view! {
            <button class="adi-chome__group-act" type="button"
                title="Forget this node's password here. Nothing on the node changes — this \
                       machine just stops being able to ask it anything."
                on:click=move |_| apply_fleet_dashboards(state, fetch::forget_node(forget.clone()))>
                "Lock"
            </button>
        }
        .into_any()
    };

    // Beside it, the node itself. Its dashboards are each one origin under this node, and the node
    // is one more: its control panel, which is the only service pairing grants unasked (§8). So this
    // one link is live even while every row under it is an ask — and even while the row is locked,
    // where the browser asks for the password this machine does not hold.
    let panel_host = node.app_host();
    // …whenever there is an address for it from here. The petname in it is this machine's own
    // (§2), so read through a node it names a third machine and the mesh routes one hop only.
    let panel = crate::origin::service_url(&panel_host).map(|href| {
        view! {
            <a class="adi-chome__group-act" href=href
                target="_blank" rel="noreferrer"
                title=format!("This node's own control panel, over the mesh: {panel_host}")>
                "Panel"
            </a>
        }
    });

    let body = if locked && opening {
        chat_unlock_form(state)
    } else if let Some(error) = node.error.clone() {
        view! { <div class="adi-chome__nodeerr">{error}</div> }.into_any()
    } else if locked {
        ().into_any()
    } else if node.dashboards.is_empty() {
        view! { <div class="adi-chome__nodeerr">"No dashboards on this node."</div> }.into_any()
    } else {
        node.dashboards
            .iter()
            .map(|d| chat_node_dash_item(state, &node.node, d))
            .collect::<Vec<_>>()
            .into_any()
    };

    // A node is a band like a project is, so it is the same heading — with the node's own
    // controls riding its right edge where a count would otherwise sit.
    let n = node.dashboards.len();
    view! {
        <div class="relative">
            <adi_ui::RailGroup label=name count=n>{body}</adi_ui::RailGroup>
            <div class="absolute top-3 right-2.5 flex items-center gap-2">{panel}{action}</div>
        </div>
    }
    .into_any()
}

/// The inline "give this machine the node's password" form.
///
/// A field and not a `prompt()`, for the reason the transfer panel gives: this is the one input on
/// the page that must not be readable `type="text"`, and the browser must never offer it back from
/// autofill — it is not this machine's secret to remember, only to hold.
fn chat_unlock_form(state: State) -> AnyView {
    let unlock = state.fleet_unlock;
    view! {
        <form class="adi-chome__unlock" on:submit=move |ev| {
            ev.prevent_default();
            submit_unlock(state);
        }>
            <input class="adi-input adi-chome__unlock-field" type="password"
                autocomplete="new-password" placeholder="node password"
                prop:value=move || unlock.password.get()
                on:input=move |ev| unlock.password.set(event_target_value(&ev)) />
            <button class="adi-btn adi-btn--primary adi-chome__unlock-go" type="submit"
                prop:disabled=move || unlock.busy.get()>
                {move || if unlock.busy.get() { "Asking\u{2026}" } else { "Unlock" }}
            </button>
        </form>
        {move || unlock.error.get().map(|e| view! {
            <div class="adi-chome__nodeerr">{e}</div>
        })}
        <div class="adi-chome__nodehint">
            "Printed once, on the node, when it joined this fleet. Kept here encrypted so the rail
             can ask again without asking you."
        </div>
    }
    .into_any()
}

/// Check the password against the node and, if it takes, keep it — the listing that comes back is
/// the node's dashboards, so the form closes onto the rows it just unlocked.
fn submit_unlock(state: State) {
    let unlock = state.fleet_unlock;
    let (node, password) = (unlock.node.get(), unlock.password.get());
    if node.is_empty() || password.is_empty() {
        return;
    }
    unlock.busy.set(true);
    unlock.error.set(None);
    spawn_local(async move {
        match fetch::unlock_node(node, password).await {
            Ok(f) => {
                state.fleet_dashboards.set(Some(f));
                unlock.close();
            }
            Err(e) => unlock.error.set(Some(e)),
        }
        unlock.busy.set(false);
    });
}

/// One of a node's dashboards.
///
/// Three shapes, because there are three genuinely different situations and only one of them is a
/// link. **Allowed and up** opens on its own origin at `<service>.<node>.n.adi` — the same address
/// a transfer reports, and the only one under which the page's `/api` calls route (§4). **Up but
/// not granted** is the ordinary state of a dashboard this machine did not put there: pairing hands
/// out `http:app` and nothing else (§8), so the row asks the node for the grant rather than
/// offering a link that would answer *not authorized*. **Down** is dimmed and says so — the failure
/// is the node's to fix, not a row that should vanish.
///
/// A fourth, when this panel is itself being read through a node: the address the listing built is
/// a name from *this* machine's registry, so from over the mesh it points at a third machine and
/// there is nothing to click.
fn chat_node_dash_item(state: State, node: &str, d: &NodeDashboard) -> AnyView {
    let name = d.name.clone();
    let machine = node.to_string();

    // Down on its node: the failure is the node's to fix, so the row stays and says so with
    // its dot rather than vanishing.
    if !d.running {
        return view! {
            <adi_ui::AppItem
                title=name
                state=adi_ui::AppState::Offline
                machine=machine
                attr:title="not running on this node"
            />
        }
        .into_any();
    }

    match (
        d.allowed,
        d.url.as_deref().and_then(crate::origin::mapped_url),
        d.service.clone(),
    ) {
        // Allowed and up: a link, on its own origin at `<service>.<node>.n.adi` — the only
        // address under which the page's own `/api` calls route.
        (true, Some(href), _) => view! {
            <adi_ui::AppItem
                title=name
                state=adi_ui::AppState::Live
                machine=machine
                href=href
                blank=true
                attr:title=d.description.clone().unwrap_or_else(|| d.name.clone())
            />
        }
        .into_any(),
        // Up, but this machine was never granted it — pairing hands out `http:app` and
        // nothing else. Offering a link would answer *not authorized*, so the row asks the
        // node instead, behind the dot: it is a guest's view until the grant lands.
        (false, _, Some(service)) => {
            let (ask_node, ask_service) = (node.to_string(), service.clone());
            view! {
                <adi_ui::AppItem
                    title=name
                    state=adi_ui::AppState::ViewOnly
                    machine=machine
                    attr:title=format!(
                        "This node has not granted this machine http:{service} yet. Ask it to.",
                    )
                    action=move || {
                        let (n, sv) = (ask_node.clone(), ask_service.clone());
                        view! {
                            <adi_ui::Button
                                size=adi_ui::ButtonSize::Small
                                on:click=move |_| apply_fleet_dashboards(
                                    state,
                                    fetch::allow_node_service(n.clone(), sv.clone()),
                                )
                            >
                                "Ask for access"
                            </adi_ui::Button>
                        }
                        .into_any()
                    }
                />
            }
            .into_any()
        }
        // Running and granted, but not reachable from where this page is being read: either the
        // node gave it no routable name at all, or the name it gave is one only the machine
        // serving this panel can resolve.
        _ => {
            let why = if d.url.is_some() {
                "on this node's own fleet — not reachable from where you are reading this panel"
            } else {
                "no host on the node — it has no address the mesh could route to"
            };
            view! {
                <adi_ui::AppItem
                    title=name
                    state=adi_ui::AppState::ViewOnly
                    machine=machine
                    attr:title=why
                />
            }
            .into_any()
        }
    }
}

/// The rail's refresh: ask every node again. Shown only once a fleet is known to exist, so a
/// machine paired with nobody sees exactly the rail it saw before any of this.
fn chat_fleet_refresh(state: State) -> AnyView {
    if state
        .fleet_dashboards
        .get()
        .is_none_or(|f| f.nodes.is_empty())
    {
        return ().into_any();
    }
    let busy = state.fleet_dashboards_busy;
    view! {
        <button class="adi-chome__group-act" type="button"
            title="Ask every paired node what it is running"
            prop:disabled=move || busy.get()
            on:click=move |_| refresh_fleet_dashboards(state)>
            {move || if busy.get() { "Asking\u{2026}" } else { "Refresh" }}
        </button>
    }
    .into_any()
}

/// Run one of the fleet-rail mutations: fold the fresh listing in, or flash what went wrong. The
/// endpoints all answer with the whole listing, so a grant or a lock updates the rail in one
/// round-trip rather than leaving it to the next refresh.
fn apply_fleet_dashboards<F>(state: State, fut: F)
where
    F: std::future::Future<Output = Result<FleetDashboards, String>> + 'static,
{
    spawn_local(async move {
        match fut.await {
            Ok(f) => state.fleet_dashboards.set(Some(f)),
            Err(e) => state.flash.set(Some(Flash::err(e))),
        }
    });
}
