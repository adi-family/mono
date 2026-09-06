//! The Agents page run, stop, and live-view actions.
//!
//! An agent definition is a *template*. For interactive (pty) backends a Run starts a session you
//! type into and View watches its pane. For headless (`process` / `harness`) backends each Run is an
//! independent run of the agent's settings (a fresh dialog, never continued): every run keeps its
//! own log, several may be live at once, and the live view is a browsable run history plus a task
//! composer — never a shared, overwritten slot.

use adi_ui::{EmptyRow, Row as TableRow, Table};
use adi_webapp_api::types::{
    AgentAsk, AgentAwait, AgentDto, AgentGoal, AgentNearDup, AgentRepeat, AgentRepeatShape,
    AgentRunInfo, AgentRuns, AgentStep, AgentTokenSource, AgentTokens, AgentToolStatus, AgentTurn,
    AgentsState, AllAgentRuns, Dashboard, FleetDashboards, NodeDashboard, NodeDashboards,
};
use leptos::prelude::*;
use wasm_bindgen_futures::spawn_local;

use crate::fetch;
use crate::launcher::{self, Launcher};
use crate::routing::{Route, agent_form_path, scroll_top};
use crate::state::{
    AgentsWatch, ChatDrawer, Flash, ROOT_AGENT, SESSION_PAGE, SessionFilter, SessionMenu, State,
    refresh_fleet_dashboards,
};
use crate::ui::{
    Key, Sort, TableState, apply_mutation, display_message, field_hint, prompt, sort_rows,
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

/// The auto-title toggle: whether a fresh conversation is retitled from its opening message by a
/// local model once one answers (`adi_agents::AutoTitleSettings`). Lives beside the run cap for the
/// same reason: it is a standing preference about how a chat behaves, not a per-agent setting, so it
/// belongs next to the agents rather than behind a settings screen the panel doesn't otherwise have.
pub(crate) fn auto_title_view(state: State) -> impl IntoView {
    let agents = state.agents;
    let checked = move || agents.get().is_some_and(|a| a.auto_title_enabled);
    view! {
        <label class="adi-field adi-field--check">
            <input type="checkbox"
                prop:checked=checked
                on:change=move |ev| {
                    let enabled = event_target_checked(&ev);
                    let msg = if enabled {
                        "New chats will be renamed from a local model's guess.".to_string()
                    } else {
                        "New chats will keep the title their opening message gives them.".to_string()
                    };
                    apply_agents(state, None, msg, fetch::set_auto_title(enabled));
                } />
            <span class="adi-field__label">"Auto-name new chats"</span>
            {field_hint("Guess a name for a new chat from its opening message, using a local model, once one answers. Off costs nothing — chats keep the title their opening message gives them either way.")}
        </label>
    }
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
        draft.get().unwrap_or_else(|| {
            load.get()
                .map_or_else(String::new, |(_, max)| max.to_string())
        })
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
                    on:click=move |_| open_watch(watch, None, watch_name.clone(), interactive)>"View"</button>
                " "
                <button class="adi-btn adi-btn--link" title=stop_title
                    on:click=move |_| stop_agent(state, watch, stop_name.clone())>"Stop"</button>
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
                        {if full { "Run anyway" } else { "Run" }}
                    </button>
                    " "
                }
                .into_any()
            } else if answerable {
                view! {
                    <button class="adi-btn adi-btn--link" title="start a conversation you can answer"
                        on:click=move |_| open_watch(watch, None, run_name.clone(), false)>"Chat…"</button>
                    " "
                }
                .into_any()
            } else {
                view! {
                    <button class="adi-btn adi-btn--link" title="give it a task and run it headless"
                        on:click=move |_| open_watch(watch, None, run_name.clone(), false)>"Run…"</button>
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
/// human's "run it anyway" past a full concurrency limit. Always this machine — the Agents page has
/// no node concept (`docs/fleet.md` §13).
pub(crate) fn run_now(state: State, name: String, force: bool) {
    run_now_with(state, None, name, force, None, None);
}

/// [`run_now`], carrying the run settings of the composer that asked for it — a pty session started
/// from the chat home is started with the same "run here" and the same overrides a headless one
/// would be. The Agents list's own ▶ Run has no such panel behind it and passes neither: it acts on
/// a row, which is rarely the agent the composer is pointed at.
///
/// `node` is which source the picker above the "Start session" button is pointed at
/// (`docs/fleet.md` §13) — `None` for this machine, which is every call site except the chat home's
/// own pty affordance, since a pty agent only ever runs on the machine that defines it.
fn run_now_with(
    state: State,
    node: Option<String>,
    name: String,
    force: bool,
    working_dir: Option<String>,
    overrides: Option<adi_webapp_api::types::AgentRunOverrides>,
) {
    spawn_local(async move {
        // No task and no attachments: a pty session is typed into after it starts, so there is no
        // message here for a picture to belong to.
        match fetch::run_agent(
            node.as_deref(),
            name,
            String::new(),
            working_dir,
            overrides,
            force,
            Vec::new(),
        )
        .await
        {
            Ok(res) => {
                set_source_agents(state, node.as_deref(), res.state);
                state.flash.set(Some(Flash::ok(res.message)));
            }
            Err(e) => state.flash.set(Some(Flash::err(e))),
        }
    });
}

/// Launch a new headless run of the agent with `message` as its task, then select that run in the
/// panel so its log streams in. Each launch is independent — never a continuation of a prior run.
/// `working_dir` is the run settings' "run here" and `overrides` the rest of that panel; `None` and
/// `None` start the agent exactly as it is defined. `force` launches past a full concurrency limit —
/// the composer sends it once the cap is reached, where the button already reads "Run anyway".
///
/// Launches on `watch.node` — whichever source the composer's picker is pointed at
/// (`docs/fleet.md` §13) — and re-asserts it on the watch once the launch lands, rather than
/// trusting it is still what it was when the request went out: the picker (and so `watch.node`)
/// can move while the request is in flight, and the conversation that just started belongs to the
/// source it was actually sent to, not wherever the picker has since drifted to.
fn launch_agent(
    state: State,
    watch: AgentsWatch,
    name: String,
    message: String,
    working_dir: Option<String>,
    overrides: Option<adi_webapp_api::types::AgentRunOverrides>,
    force: bool,
    images: Vec<String>,
) {
    let node = watch.node.get_untracked();
    spawn_local(async move {
        match fetch::run_agent(
            node.as_deref(),
            name.clone(),
            message,
            working_dir,
            overrides,
            force,
            images,
        )
        .await
        {
            Ok(res) => {
                set_source_agents(state, node.as_deref(), res.state);
                state.flash.set(Some(Flash::ok(res.message)));
                watch.peek.set(None);
                watch.log.set(String::new());
                if !res.run_id.is_empty() {
                    watch.run_id.set(Some(res.run_id));
                }
                watch.node.set(node);
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
pub(crate) fn stop_agent(state: State, watch: AgentsWatch, name: String) {
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
///
/// Always the *open* conversation, so its source is `watch.node` — a row's own Stop is reached
/// through the chat pane's controls once the row is open, not from the rail row directly.
fn stop_one_run(state: State, watch: AgentsWatch, run_id: String) {
    let Some(name) = watch.name.get_untracked() else {
        return;
    };
    if run_id.is_empty() {
        return;
    }
    let node = watch.node.get_untracked();
    spawn_local(async move {
        match fetch::stop_run(node.as_deref(), name.clone(), run_id).await {
            Ok(runs) => {
                if watch.name.get_untracked().as_deref() == Some(name.as_str()) {
                    watch.runs.set(runs.runs);
                    poll_watch(watch);
                }
                refresh_source_agents(state, node.as_deref());
            }
            Err(e) => state.flash.set(Some(Flash::err(e))),
        }
    });
}

/// Refresh one source's own agent list after a mutation settles its running flag — this machine's
/// [`State::agents`] for `None`, or one node's slice of [`State::rail_node_agents`]
/// (`docs/fleet.md` §13). Fire-and-forget: a stale running flag for a second or two is not worth a
/// caller waiting on.
fn refresh_source_agents(state: State, node: Option<&str>) {
    match node {
        None => spawn_local(async move {
            if let Ok(st) = fetch::agents().await {
                state.agents.set(Some(st));
            }
        }),
        Some(node) => crate::state::refresh_rail_node(state, node.to_string()),
    }
}

/// Fold one source's freshly answered `AgentsState` in directly, rather than asking again — this
/// machine's [`State::agents`] for `None`, or one node's slice of [`State::rail_node_agents`]
/// (`docs/fleet.md` §13). What a launch on that source hands back already *is* its post-launch agent
/// list, so [`refresh_source_agents`]'s round trip would only repeat the answer just received.
fn set_source_agents(state: State, node: Option<&str>, agents: AgentsState) {
    match node {
        None => state.agents.set(Some(agents)),
        Some(node) => {
            state.rail_node_agents.update(|m| {
                m.insert(node.to_string(), agents);
            });
        }
    }
}

/// Delete one run — for a harness agent, the whole conversation, transcript and all — behind an
/// explicit confirmation, since nothing here is recoverable. A live run is stopped by the server
/// first. If the deleted run is the one on screen, its detail view closes rather than polling a
/// conversation that no longer exists.
///
/// `agent` is named rather than taken from the watch, because the sessions rail lists every agent's
/// chats at once: the row's own agent is the one to delete from, not whichever one is on screen.
/// `node` is the row's own source (`docs/fleet.md` §13) — `None` for this machine.
fn delete_one_run(
    state: State,
    watch: AgentsWatch,
    node: Option<String>,
    agent: String,
    run_id: String,
    title: String,
) {
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
    // the run ids of two different agents are no reason to close what is open. Matched on the row's
    // own source too: two sources can each have an agent of the same name, and a run id is only
    // unique on the machine that minted it.
    if watch.name.get_untracked().as_deref() == Some(agent.as_str())
        && watch.run_id.get_untracked().as_deref() == Some(run_id.as_str())
        && watch.node.get_untracked() == node
    {
        close_run_view(watch);
    }
    spawn_local(async move {
        match fetch::delete_run(node.as_deref(), agent.clone(), run_id).await {
            Ok(runs) => {
                if watch.name.get_untracked().as_deref() == Some(agent.as_str())
                    && watch.node.get_untracked() == node
                {
                    watch.runs.set(runs.runs);
                }
                // The agent list carries a running flag that a deleted live run may have settled.
                refresh_source_agents(state, node.as_deref());
            }
            Err(e) => state.flash.set(Some(Flash::err(e))),
        }
    });
}

/// Point the live view at an agent: drop everything the one before it left behind (its snapshot, log
/// tail, run selection and history), remember whether this one is interactive, and fetch the first
/// snapshot — the 1s poll takes over from there. The view moves; nothing on the page scrolls, which
/// is what the chat home wants when its picker switches agents mid-screen.
///
/// `node` is which source this agent is being watched *on* (`docs/fleet.md` §13) — `None` for this
/// machine, which is every call site except the sessions rail's own rows and hotkeys. Set on
/// [`AgentsWatch::node`] before anything is polled, so the very first request the new view makes
/// already goes to the right place.
fn point_watch(watch: AgentsWatch, node: Option<String>, name: String, interactive: bool) {
    watch.peek.set(None);
    watch.log.set(String::new());
    watch.run_id.set(None);
    watch.runs.set(Vec::new());
    // Reset until the first history poll reports whether this backend keeps answerable conversations.
    watch.answerable.set(false);
    watch.reply.set(String::new());
    watch.interactive.set(interactive);
    watch.node.set(node);
    // Before the name is set, so the settings and the agent they belong to land together and the
    // composer is never briefly showing the last agent's directory under this one's name.
    adopt_run_settings(watch, &name);
    watch.name.set(Some(name));
    poll_watch(watch);
}

/// Open the run panel on an agent (View / Run…): point the view at it, then scroll to the panel —
/// on the Agents page it sits below the list, so a click near the bottom would otherwise open it
/// out of sight. Always this machine — the Agents page has no node concept — except when a review's
/// reviewer turns out to be a pty agent, which opens on the reviewed conversation's own source.
pub(crate) fn open_watch(watch: AgentsWatch, node: Option<String>, name: String, interactive: bool) {
    point_watch(watch, node, name, interactive);
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
fn point_conversation(
    watch: AgentsWatch,
    node: Option<String>,
    name: String,
    run_id: String,
    interactive: bool,
) {
    point_watch(watch, node, name, interactive);
    if !run_id.is_empty() {
        watch.run_id.set(Some(run_id));
        poll_watch(watch);
        load_goals(watch);
    }
}

/// Open a specific conversation from the cross-agent "All chats" index: as above, plus a scroll to
/// the panel it opens in, which on the Agents page sits below the index. Always this machine — the
/// index this reads from is local.
pub(crate) fn open_conversation(
    watch: AgentsWatch,
    name: String,
    run_id: String,
    interactive: bool,
) {
    point_conversation(watch, None, name, run_id, interactive);
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
    let node = watch.node.get_untracked();
    if watch.interactive.get_untracked() {
        let for_peek = node.clone();
        spawn_local(async move {
            if let Ok(peek) = fetch::peek_agent(for_peek.as_deref(), name).await
                && watch.name.get_untracked().as_deref() == Some(peek.name.as_str())
                // Checked on the source too, not only the name — two sources can each run an
                // agent of the same name (`docs/fleet.md` §13).
                && watch.node.get_untracked() == for_peek
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
        let for_runs = node.clone();
        spawn_local(async move {
            if let Ok(runs) = fetch::agent_runs(for_runs.as_deref(), name.clone()).await
                && watch.name.get_untracked().as_deref() == Some(name.as_str())
                && watch.node.get_untracked() == for_runs
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
            if let Ok(peek) = fetch::peek_run(node.as_deref(), name.clone(), run_id).await
                && watch.name.get_untracked().as_deref() == Some(name.as_str())
                && watch.run_id.get_untracked().as_deref() == Some(peek.run_id.as_str())
                && watch.node.get_untracked() == node
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
pub(crate) fn all_chats_view(
    state: State,
    watch: AgentsWatch,
    only: Option<Vec<String>>,
) -> AnyView {
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
            "Conversation" => Key::text(display_message(r)),
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
                    "Agent" => view! { <span>{agent.clone()}</span> }.into_any(),
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
        (true, true, _) => "answering",
        (false, true, _) => "running",
        (_, false, false) => "awaiting",
        (true, false, true) => "idle",
        (false, false, true) => "done",
    }
}

/// The `data-state` a status word takes on `.adi-status`, which is what colours its dot: the
/// accent for a turn in flight, nothing for the rest.
fn status_tone(word: &str) -> &'static str {
    match word {
        "answering" | "running" => "running",
        _ => "quiet",
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
        many => format!(
            "waiting on {} wakes. The first: {}",
            many.len(),
            said(&many[0])
        ),
    }
}

/// One run's cell under `col`. `Conversation` and `Task` are the same cell under two headers —
/// which of them a table declares is what the backend's kind decides. Matching the header text —
/// the same key the sort uses — is what lets the user hide and reorder columns without the row
/// builder knowing about it.
fn run_cell(col: &str, r: &AgentRunInfo, answerable: bool) -> AnyView {
    match col {
        // A dot and a word (§6). The wake's sentence rides the tooltip — the same one the rail
        // hangs on this conversation, so two surfaces never describe one state two ways.
        "Status" => {
            let hint = awaiting_hint(&r.awaits);
            let word = run_status(answerable, r);
            view! {
                <span class="adi-status" data-state=status_tone(word) title=hint>
                    <span class="adi-status__led"></span>
                    {word}
                </span>
            }
            .into_any()
        }
        "Conversation" | "Task" => {
            let full = display_message(r).to_string();
            let short = truncate_task(&full);
            view! { <span title=full>{short}</span> }.into_any()
        }
        // "When", and anything the layout offers that this match doesn't name.
        _ => view! {
            <span class="adi-muted" style="white-space:nowrap">{run_age(r.started_at)}</span>
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
                    <code class="adi-mono adi-muted">{attach}</code>
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
            "Conversation" | "Task" => Key::text(display_message(r)),
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
    let title = if answerable { "Chat" } else { "Run" };
    view! {
        <tr class="adi-runlog">
            <td class="adi-runlog__cell whitespace-normal" colspan=span>
                // A code block, framed and titled: obviously a console, not another row.
                <div class="adi-runlog__card">
                    <div class="adi-runlog__bar">
                        <span class="adi-runlog__title">{title}</span>
                        <span class="adi-runlog__run">{run_id}</span>
                        <span class="adi-spacer"></span>
                        // The `tail -f` hint stays available for following the raw log by hand.
                        {move || run_log_status(watch)}
                        <button class="adi-runlog__close" type="button" title="close this"
                            on:click=move |_| close_run_view(watch)>"Close"</button>
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
        watch
            .peek
            .with(|p| p.as_ref().and_then(|p| p.pending_question.clone()))
    });
    // Likewise: the queue changes when you queue or unqueue something, not when the agent works.
    let queued = Memo::new(move |_| {
        watch.peek.with(|p| {
            p.as_ref()
                .map(|p| {
                    p.turns
                        .iter()
                        .filter(|t| t.queued)
                        .cloned()
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default()
        })
    });
    view! {
        // Above the transcript, sharing its left edge: the goal (a standing condition on the
        // conversation rather than a thing said in it), the wakes it is waiting on, and the
        // composer — because the transcript reads newest-first, what you type appears at the
        // top, next to the box you typed it in.
        {answerable.then(|| view! {
            <div class="adi-chat__top">
                {goal_bar(state, watch)}
                {awaits_bar(state, watch)}
                {reply_bar(state, watch)}
            </div>
        })}

        <adi_ui::Chat
            class="adi-chat__transcript min-h-0 flex-1"
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
fn feed_turn(node: Option<&str>, at: usize, turn: &AgentTurn) -> Vec<adi_ui::Entry> {
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
                images: pictures(node, turn),
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
            AgentStep::Tool {
                name,
                input,
                status,
                output,
            } => {
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
    let node = watch.node.get();
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
            .flat_map(|(at, t)| feed_turn(node.as_deref(), at, t))
            .collect()
    })
}

/// A turn's attachments, as the transcript draws them: a URL to fetch each by, its name, and
/// whether it is a picture to show or a file to link to.
///
/// The bytes are never in the snapshot — a chat is polled once a second, and one that inlined its
/// screenshots would re-send every one of them every tick. The address is stable and the content
/// behind it never changes, so the browser fetches each exactly once.
fn pictures(node: Option<&str>, turn: &AgentTurn) -> Vec<adi_ui::Attachment> {
    turn.images
        .iter()
        .map(|image| adi_ui::Attachment {
            url: crate::attach::url_of(node, &image.id),
            name: image.name.clone(),
            kind: crate::attach::kind_of(&image.media_type),
        })
        .collect()
}

/// A message still waiting its turn: your own bubble, dashed and dimmed — said, but not yet asked —
/// carrying an × that takes it back before the agent ever sees it. The bubble itself is
/// [`adi_ui::Queued`], which is the same shape as the sent messages above it in the feed.
fn queued_bubble(state: State, watch: AgentsWatch, turn: AgentTurn, place: usize) -> AnyView {
    let node = watch.node.get_untracked();
    view! {
        <adi_ui::Queued
            body=turn.text.clone()
            images=pictures(node.as_deref(), &turn)
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
    Some(view! { <div class="adi-chat__empty">{msg}</div> }.into_any())
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

/// Micro-dollars as dollars and cents (`$6.78`). Never four decimals: a fraction of a cent is
/// not a number anyone acts on, and a column of them reads as noise (§8).
fn fmt_cost(micro: u64) -> String {
    format!("${:.2}", micro as f64 / 1_000_000.0)
}

/// Milliseconds in the coarsest unit that still says something: `850ms`, `8.6s`, `22 min`,
/// `1h 17m`. Raw seconds past a minute are a sum to do, not a duration to read.
fn fmt_duration(ms: u64) -> String {
    let secs = ms / 1000;
    if ms < 1000 {
        format!("{ms}ms")
    } else if secs < 60 {
        format!("{:.1}s", ms as f64 / 1000.0)
    } else if secs < 3600 {
        format!("{} min", secs / 60)
    } else {
        format!("{}h {}m", secs / 3600, (secs % 3600) / 60)
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

/// What a composer says instead of offering a paperclip, when this conversation can be shown
/// nothing. Said before anything is pasted, because a file the send would have dropped is one
/// somebody has already decided to send.
///
/// Rare, now that every real engine can take one — a picture in the request body or by being told
/// where the file is, and a file always by its path. What is left is a live terminal, which is typed
/// into rather than sent to, and a simulated run, which has a person in the model's seat and nothing
/// to open a file with.
const IMAGES_REFUSED: &str = "this one can't be sent a file — a terminal session takes typing, and \
                              a simulated run has no model to give one to";

/// The reply box: says the next thing into the selected conversation. It never locks you out while
/// the agent is working — one turn runs at a time, so a message sent mid-answer is *queued* (the
/// button says so), and is picked up either by the turn in flight, at its next round, or by the one
/// that starts when this answer lands. Beside it, while an answer is streaming, a Stop that cuts the
/// turn short and drops anything lined up behind it.
fn reply_bar(state: State, watch: AgentsWatch) -> impl IntoView {
    let answering = move || watch.peek.get().is_some_and(|p| p.running);
    // What the open conversation's own backend can take — the snapshot's capability profile, not
    // the agent's current settings, because a conversation is answered by whatever started it.
    let takes_images = Signal::derive(move || watch.peek.get().is_some_and(|p| p.caps.images));
    let attach = crate::attach::attaching(
        state,
        watch.node.get_untracked(),
        watch.reply_files,
        takes_images,
        Signal::derive(|| IMAGES_REFUSED.to_string()),
    );
    let placeholder = Signal::derive(move || {
        watch
            .name
            .get()
            .map_or_else(|| "Write…".to_string(), |name| format!("Write to {name}…"))
    });
    view! {
        <adi_ui::Composer
                value=watch.reply
                // Answering is not busy: a message typed while the agent works is *queued*,
                // which is a thing you are allowed to do and the line under the box says so.
                busy=false
                placeholder=placeholder
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
        <div class="adi-chat__note">
            {move || if answering() { "queued — the agent is answering" } else { "" }}
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
        <div>
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

/// The closed state: one 13px grey link, and no more of the screen than that.
///
/// Most conversations never have a goal. A permanently open text box for something rarely used
/// takes more room than the composer it sits above, and reads as a field somebody forgot to fill in
/// rather than an option they can take.
fn goal_link(watch: AgentsWatch) -> AnyView {
    view! {
        <div class="adi-chat__goal">
            <button class="adi-chat__link" type="button"
                title="Set what would make this chat done. It is put back to the agent every time \
                       the chat falls quiet, until it is met or given up on."
                on:click=move |_| open_goal_editor(watch, None)>
                "+ Set a goal"
            </button>
        </div>
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
        <div class="adi-chat__goal">
            <input class="adi-input adi-input--wide" type="text"
                placeholder="what would make this chat done"
                prop:value=move || watch.goal_input.get()
                prop:disabled=move || watch.goal_busy.get()
                on:input=move |ev| watch.goal_input.set(event_target_value(&ev))
                // Enter saves and Escape closes, because this opened under the cursor and asking
                // for the mouse back to dismiss a one-line box is the annoying half of a popover.
                on:keydown=move |ev: leptos::ev::KeyboardEvent| {
                    match ev.key().as_str() {
                        "Enter" => save(),
                        "Escape" => close_goal_editor(watch),
                        _ => {}
                    }
                } />
            <button class="adi-btn adi-btn--sm" type="button"
                prop:disabled=move || {
                    watch.goal_busy.get() || watch.goal_input.get().trim().is_empty()
                }
                on:click=move |_| save()>"Save"</button>
            <button class="adi-btn adi-btn--quiet adi-btn--sm" type="button"
                on:click=move |_| close_goal_editor(watch)>"Cancel"</button>
        </div>
        <div class="adi-chat__note" style="margin:0 0 8px">
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
        <div class="adi-chat__goal">
            <span title="This chat has a goal">"Goal"</span>
            <button class="adi-chat__goal-text" type="button"
                title=format!(
                    "{text} — click to reword{}",
                    if goal.set_by == "agent" { " (the agent set this itself)" } else { "" },
                )
                on:click=move |_| open_goal_editor(watch, Some((edit_id.clone(), text.clone())))>
                {goal.text.clone()}
            </button>
            {(goal.set_by == "agent").then(|| view! {
                <span title="The agent set this goal for itself">"self-set"</span>
            })}
            {(goal.nudges > 1).then(|| view! {
                <span>{format!("asked {}×", goal.nudges)}</span>
            })}
            <button class="adi-btn adi-btn--quiet adi-btn--sm" type="button"
                prop:disabled=move || watch.goal_busy.get()
                title="Close this goal as met"
                on:click=move |_| close_goal(state, watch, met_id.clone(), "met")>"Met"</button>
            <button class="adi-btn adi-btn--quiet adi-btn--sm adi-btn--danger" type="button"
                prop:disabled=move || watch.goal_busy.get()
                title="Stop working toward this goal, and stop being asked about it"
                on:click=move |_| close_goal(state, watch, gave_id.clone(), "given_up")>
                "Give up"
            </button>
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
        <div>
            {move || {
                // Narrowed to the awaits alone, so the rows are rebuilt when a wake is registered
                // or fires — not once a second, under a hand reaching for "Stop waiting".
                let awaits = watch
                    .peek
                    .with(|p| p.as_ref().map(|p| p.awaits.clone()).unwrap_or_default());
                if awaits.is_empty() {
                    return ().into_any();
                }
                awaits
                    .into_iter()
                    .map(|a| await_row(state, watch, a))
                    .collect::<Vec<_>>()
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
        Some(check) => format!(
            "{}\n\nchecks: {check}",
            awaiting_hint(std::slice::from_ref(&a))
        ),
        None => awaiting_hint(std::slice::from_ref(&a)),
    };
    view! {
        <div class="adi-chat__goal">
            <span title="This chat is waiting on something">"Awaiting"</span>
            <span class="adi-chat__goal-text" style="cursor:default" title=hint>{said}</span>
            {countdown(watch, &a)}
            <button class="adi-btn adi-btn--quiet adi-btn--sm" type="button"
                prop:disabled=move || watch.await_busy.get()
                title="Drop this wake. The chat stops waiting for it and stays where it is — \
                       nothing is cancelled at the other end."
                on:click=move |_| ignore_await(state, watch, id.clone())>
                "Stop waiting"
            </button>
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
    Some(view! { <span>{text}</span> }.into_any())
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
    let node = watch.node.get_untracked();
    watch.await_busy.set(true);
    spawn_local(async move {
        let dropped = fetch::ignore_agent_await(node.as_deref(), name, run_id, id).await;
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
    let node = watch.node.get_untracked();
    spawn_local(async move {
        let Ok(goals) = fetch::agent_goals(node.as_deref(), name.clone(), run_id.clone()).await
        else {
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
    let node = watch.node.get_untracked();
    watch.goal_busy.set(true);
    spawn_local(async move {
        let saved = fetch::set_agent_goal(node.as_deref(), name, run_id, text, editing).await;
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
    let node = watch.node.get_untracked();
    watch.goal_busy.set(true);
    spawn_local(async move {
        let closed =
            fetch::close_agent_goal(node.as_deref(), goal, as_.to_string(), String::new()).await;
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
    let node = watch.node.get_untracked();
    watch.answering.set(true);
    spawn_local(async move {
        let answered =
            fetch::answer_run(node.as_deref(), name.clone(), run_id.clone(), ask, replies).await;
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
    let node = watch.node.get_untracked();
    spawn_local(async move {
        match fetch::reply_to_run(node.as_deref(), name.clone(), run_id.clone(), message, images)
            .await
        {
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
    let node = watch.node.get_untracked();
    spawn_local(async move {
        match fetch::unqueue_from_run(node.as_deref(), name.clone(), run_id.clone(), index).await {
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
    (!attach.is_empty()).then(|| view! { <code class="adi-runlog__cmd">{attach}</code> }.into_any())
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
    let del_title = truncate_task(display_message(r));
    // The selected row tints itself; `Row`'s `class` takes it, so this still goes through the
    // shared row rather than a hand-built `<tr>`.
    let row_class = if is_selected { "bg-active" } else { "" };
    // The action toggles this row's detail drawer: Open reveals the chat/log beneath it, and a
    // second click collapses it. Only the drawer carries an explicit "Close", so there is one
    // thing labelled Close, not two.
    let open_verb = if answerable { "Open" } else { "View" };
    let view_label = if is_selected { "Hide" } else { open_verb };
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
    // Open stays on the row — it is what the row is for. Stop and Delete go in the ⋯ menu (§8):
    // an action word repeated down every row is a column of noise.
    let open = view! {
        <button class="adi-btn adi-btn--link"
            on:click=move |_| if is_selected {
                close_run_view(watch);
            } else {
                select_run(watch, view_id.clone());
            }>{view_label}</button>
    }
    .into_any();
    let mut items: Vec<AnyView> = Vec::new();
    if running {
        items.push(crate::ui::menu_item(state, "Stop", false, move || {
            stop_one_run(state, watch, stop_id.clone());
        }));
    }
    let _ = stop_title;
    items.push(crate::ui::menu_item(state, "Delete", true, move || {
        delete_one_run(
            state,
            watch,
            watch.node.get_untracked(),
            watch.name.get_untracked().unwrap_or_default(),
            del_id.clone(),
            del_title.clone(),
        );
    }));
    let _ = delete_title;
    let actions = crate::ui::row_actions(state, format!("run:{run_id}"), open, items);
    view! {
        <TableRow
            state=table
            class=row_class
            cell=move |col| run_cell(col, &r, answerable)
            actions=actions
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

/// The composer that starts a new run/conversation: the same box the reply bar is, with its run
/// settings behind the gear in the button row. A message is required — the send button is out until
/// one is typed. Sending launches it and opens its detail: a streaming log for a one-shot run, or
/// the chat for an answerable conversation you then reply to.
///
/// The settings are what this launch does *differently* from the agent's definition — the directory
/// it starts in, the model it runs on, how freely it may act. They live behind a button rather than
/// under the box because nearly every launch changes nothing: a permanently visible row of controls
/// would be a form standing in front of the one thing this screen is for, all day, to serve the
/// occasional run that is deliberately unlike the others. The gear says when there is something set
/// (see [`run_settings_button`]), so the state is never invisible — only folded away.
fn run_bar(state: State, watch: AgentsWatch) -> impl IntoView {
    let placeholder = Signal::derive(move || {
        let name = watch.name.get().unwrap_or_default();
        if watch.answerable.get() {
            format!("Write to {name}…")
        } else {
            "A task for a new run — e.g. review the latest commit and summarize it".to_string()
        }
    });
    // A launch typed here is a human asking, so a full cap is an override to offer rather than a
    // refusal to hand back. The send button is the reply box's arrow now and cannot say "anyway",
    // so the line under the composer is what says it — before Enter is ever pressed. Read against
    // whichever source the picker above this box is pointed at (`docs/fleet.md` §13) — the same
    // source `start` below launches on.
    let at_limit = move || {
        let Some(name) = watch.name.get() else {
            return false;
        };
        let agents = match watch.node.get() {
            None => state.agents.get(),
            Some(node) => state.rail_node_agents.get().get(&node).cloned(),
        };
        at_run_limit(agents.as_ref(), &name)
    };
    let start = move |message: String| {
        let Some(name) = watch.name.get_untracked() else {
            return;
        };
        let node = watch.node.get_untracked();
        let agents = match &node {
            None => state.agents.get_untracked(),
            Some(node) => state.rail_node_agents.get_untracked().get(node).cloned(),
        };
        let force = at_run_limit(agents.as_ref(), &name);
        let images = crate::attach::ready_ids(watch.input_files);
        watch.input.set(String::new());
        crate::attach::clear(watch.input_files);
        launch_agent(
            state,
            watch,
            name,
            with_context(watch, message),
            run_dir_of(watch),
            run_overrides_of(state, watch),
            force,
            images,
        );
    };
    // Asked of the agent this composer is pointed at, before any conversation exists — which is
    // the only thing that can answer here, and the reason the capability rides on the listing as
    // well as on a snapshot. That listing is whichever source the picker chose the agent from, not
    // always this machine's — a starred agent on a selected node answers for itself the same way.
    let takes_images = Signal::derive(move || {
        let Some(name) = watch.name.get() else {
            return false;
        };
        let agents = match watch.node.get() {
            None => state.agents.get(),
            Some(node) => state.rail_node_agents.get().get(&node).cloned(),
        };
        agents.is_some_and(|s| {
            s.agents
                .iter()
                .find(|a| a.name == name)
                .is_some_and(|a| a.caps.images)
        })
    });
    let attach = crate::attach::attaching(
        state,
        // Whichever source the picker above this box launches on (`docs/fleet.md` §13) — a picture
        // still refuses there (the forwarder carries JSON, not bytes: `fetch::upload_attachment`),
        // but with the node named in the refusal instead of one silently sent to the wrong machine.
        watch.node.get_untracked(),
        watch.input_files,
        takes_images,
        Signal::derive(|| IMAGES_REFUSED.to_string()),
    );
    view! {
        <div class="adi-chat__runbar">
            // Above the box, not over it: this panel is as tall as the settings the backend takes,
            // and a layer floating over the composer would cover the message being typed on the one
            // screen where the message is the point. Rendered only while open.
            {move || run_settings_panel(state, watch)}
            <adi_ui::Composer
                value=watch.input
                busy=false
                placeholder=placeholder
                attr:title=COMPOSER_HINT
                settings=move || run_settings_button(watch)
                mic=move || crate::voice::mic(watch.input)
                attach=attach
                on_send=Callback::new(start)
            />
            <div class="adi-chat__note">
                {move || if at_limit() { "the agent is at its run cap — this starts it anyway" } else { "" }}
            </div>
        </div>
    }
}

/// The `localStorage` key holding one agent's run settings in this browser.
fn run_settings_key(agent: &str) -> String {
    format!("adi-run-settings.{agent}")
}

/// The value the override map holds for a control set to "whatever the agent says".
///
/// A sentinel rather than the empty string, because on a select the empty string is already a real
/// answer: the schema's own `— default —` option, which *unsets* the agent's value for this run.
/// Inherit and unset are different instructions, and a control that spelled them the same way could
/// not offer both.
const INHERIT: &str = "__adi_inherit__";

/// What this browser starts one agent with — the shape kept under [`run_settings_key`].
///
/// Per browser and per agent, deliberately. Pointing an agent at a target and turning its model up
/// is *this machine's* standing way of running it, not a fact about the agent: the definition is
/// what every machine and every trigger gets, and an override typed here must never quietly become
/// that. The cost of keeping it locally is that another browser does not know about it, which is
/// also the point.
#[derive(Default, serde::Serialize, serde::Deserialize)]
struct StoredRunSettings {
    #[serde(default)]
    dir: String,
    #[serde(default)]
    overrides: std::collections::BTreeMap<String, String>,
}

/// Load `agent`'s stored run settings into the composer — called wherever the watched agent is
/// *changed*, so the panel and its badge describe the agent now in front of you.
///
/// Deliberately not called by the dashboard-agent embed, which sets a directory of its own from the
/// dashboard it was opened on: restoring over it would point that chat somewhere else entirely.
pub(crate) fn adopt_run_settings(watch: AgentsWatch, agent: &str) {
    let stored = crate::ui::storage()
        .and_then(|s| s.get_item(&run_settings_key(agent)).ok().flatten())
        .and_then(|raw| serde_json::from_str::<StoredRunSettings>(&raw).ok())
        .unwrap_or_default();
    watch.run_dir.set(stored.dir);
    watch.run_overrides.set(stored.overrides);
}

/// Write the composer's current settings back, under whichever agent it is pointed at. Called after
/// every edit — there is no Save button, because a panel with one is a panel you can leave in a
/// state it does not act on.
fn save_run_settings(watch: AgentsWatch) {
    let Some(agent) = watch.name.get_untracked() else {
        return;
    };
    let Some(storage) = crate::ui::storage() else {
        return;
    };
    let key = run_settings_key(&agent);
    let settings = StoredRunSettings {
        dir: watch.run_dir.get_untracked(),
        overrides: watch.run_overrides.get_untracked(),
    };
    // Nothing set is nothing kept: an agent run as it is defined should leave no row behind, so the
    // next reader of this store sees only the agents somebody really configured here.
    if settings.dir.trim().is_empty() && settings.overrides.is_empty() {
        let _ = storage.remove_item(&key);
        return;
    }
    if let Ok(json) = serde_json::to_string(&settings) {
        let _ = storage.set_item(&key, &json);
    }
}

/// How many settings this launch differs by — what the gear's badge counts.
fn run_settings_count(watch: AgentsWatch) -> usize {
    usize::from(!watch.run_dir.get().trim().is_empty()) + watch.run_overrides.get().len()
}

/// This launch's directory, or `None` for "as the agent is defined".
fn run_dir_of(watch: AgentsWatch) -> Option<String> {
    let dir = watch.run_dir.get_untracked();
    let dir = dir.trim();
    (!dir.is_empty()).then(|| dir.to_string())
}

/// The composer's overrides as the launch endpoint takes them, or `None` when this run changes
/// nothing.
///
/// The controls hold strings; a field the schema calls numeric becomes a number here, exactly as the
/// agent form does when it saves one. A value that will not parse is sent as it was typed rather
/// than dropped: the launch is then refused with the engine's own message about it, which is the
/// answer, where a silently discarded setting would have run something nobody asked for.
fn run_overrides_of(
    state: State,
    watch: AgentsWatch,
) -> Option<adi_webapp_api::types::AgentRunOverrides> {
    let set = watch.run_overrides.get_untracked();
    if set.is_empty() {
        return None;
    }
    let spec = state.agents.get_untracked().map(|s| s.form);
    let mut out = adi_webapp_api::types::AgentRunOverrides::default();
    for (key, value) in set {
        if key == "unattended" {
            out.unattended = Some(value == "true");
            continue;
        }
        let numeric = spec
            .as_ref()
            .is_some_and(|s| s.fields.iter().any(|f| f.name == key && f.numeric));
        let parsed = match numeric.then(|| value.parse::<f64>()) {
            Some(Ok(number)) => number.into(),
            _ => serde_json::Value::String(value),
        };
        out.arguments.insert(key, parsed);
    }
    Some(out)
}

/// The composer's run-settings button: the gear in the button row, and the count of what is set.
///
/// The count is the whole reason it is a badge and not an icon: settings that persist between
/// launches — and these do, per agent, in this browser — are settings somebody will forget they
/// left on. A number on the button is what makes "this agent is pointed somewhere else" visible from
/// the screen you launch on, rather than one click in.
fn run_settings_button(watch: AgentsWatch) -> AnyView {
    let title = move || match run_settings_count(watch) {
        0 => "Run settings — where this run starts, and what it overrides".to_string(),
        1 => "Run settings — 1 setting overridden for runs started here".to_string(),
        n => format!("Run settings — {n} settings overridden for runs started here"),
    };
    // Bound out here, not inline: `view!` reads a `>` inside an attribute as the end of the tag, so
    // a comparison written in place would be parsed as an empty closure and a stray `0`.
    let is_set = move || run_settings_count(watch) > 0;
    view! {
        <button class="adi-chat__settings" type="button"
            class:is-set=is_set
            aria-expanded=move || watch.run_settings_open.get().to_string()
            title=title
            on:click=move |_| {
                let open = !watch.run_settings_open.get_untracked();
                watch.run_settings_open.set(open);
                // Read on open rather than held live: another tab of the same panel may have
                // written since, and this is the moment somebody is about to trust what it says.
                if open && let Some(name) = watch.name.get_untracked() {
                    adopt_run_settings(watch, &name);
                }
            }>
            <adi_ui::Icon icon=crate::icons::Icon::Sliders.lucide() label="Run settings"/>
            {move || {
                let n = run_settings_count(watch);
                (n > 0).then(|| view! {
                    <span class="adi-chat__settings-count">{n.to_string()}</span>
                })
            }}
        </button>
    }
    .into_any()
}

/// The run-settings panel: where this launch starts, and which of the agent's own settings it
/// replaces.
///
/// Two halves, in the order they get asked. **Run here** is the one that applies to every backend —
/// the answer to "this agent, but against *that* target" — and a conversation keeps the directory it
/// started in for every reply after. Under it, the settings the *chosen backend* actually takes,
/// named and scoped by the same server-owned schema the agent editor renders, so this panel gains a
/// dial the day a backend does and never offers one that engine has never heard of.
///
/// Every control starts on "as defined", and what it says there is the agent's own value: an
/// override is only ever a deliberate departure from something you can see. Nothing here edits the
/// agent — that is [`agent_form_path`], one link away at the foot of the panel.
fn run_settings_panel(state: State, watch: AgentsWatch) -> Option<AnyView> {
    if !watch.run_settings_open.get() {
        return None;
    }
    let name = watch.name.get()?;
    let agents = state.agents.get()?;
    let def = agents.agents.iter().find(|a| a.name == name).cloned();
    let backend = def.as_ref().map(|d| d.backend.clone()).unwrap_or_default();
    let provider = def
        .as_ref()
        .and_then(|d| d.arguments.get("provider"))
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    let fields: Vec<_> = agents
        .form
        .fields
        .iter()
        .filter(|f| f.run_override && super::form::field_applies(f, &backend, &provider))
        .cloned()
        .collect();
    let backends = agents.form.backends.clone();
    let editor = agent_form_path(&name);
    Some(
        view! {
            <section class="adi-chat__runset">
                <header class="adi-chat__runset-head">
                    <h3 class="adi-chat__runset-title">"Run settings"</h3>
                    <span class="adi-chat__runset-for">{name.clone()}</span>
                    <span class="adi-spacer"></span>
                    {move || (run_settings_count(watch) > 0).then(|| view! {
                        <button class="adi-btn adi-btn--link" type="button"
                            title="run this agent exactly as it is defined"
                            on:click=move |_| {
                                watch.run_dir.set(String::new());
                                watch.run_overrides.set(std::collections::BTreeMap::new());
                                save_run_settings(watch);
                            }>"Reset"</button>
                    })}
                    <button class="adi-btn adi-btn--icon-sm" type="button"
                        on:click=move |_| watch.run_settings_open.set(false)>
                        <adi_ui::Icon icon=adi_ui::Lucide::X label="Close"/>
                    </button>
                </header>

                <div class="adi-chat__runset-body">
                    <div class="adi-chat__runset-field adi-chat__runset-field--wide">
                        <label class="adi-chat__runset-label" for="adi-run-dir">"Run here"</label>
                        <input class="adi-input adi-input--wide adi-mono" id="adi-run-dir"
                            placeholder="as defined \u{2014} /path/to/target"
                            prop:value=move || watch.run_dir.get()
                            on:input=move |ev| {
                                watch.run_dir.set(event_target_value(&ev));
                                save_run_settings(watch);
                            }/>
                        <p class="adi-chat__runset-hint">
                            "Where the run starts, instead of the agent's own directory. The \
                             conversation keeps it for every reply."
                        </p>
                    </div>
                    {fields.into_iter()
                        .map(|f| run_override_field(watch, f, def.as_ref(), &backends, &backend))
                        .collect::<Vec<_>>()}
                </div>

                <footer class="adi-chat__runset-foot">
                    <span>
                        "Applies to runs started here, and is kept in this browser \u{2014} the \
                         agent itself is unchanged."
                    </span>
                    <a class="adi-chat__link" href=editor
                        title="change the agent itself, for every run and every machine">"Edit agent"</a>
                </footer>
            </section>
        }
        .into_any(),
    )
}

/// One overridable setting, drawn from the same schema field the agent editor uses.
///
/// The control's "as defined" state is what the agent is currently set to, spelled out — a select
/// leads with it, a text box shows it as its placeholder. That is the whole trick of this panel: you
/// cannot sensibly override what you cannot see, and the alternative (a blank box beside a label)
/// reads as though the agent has nothing set at all.
fn run_override_field(
    watch: AgentsWatch,
    field: adi_webapp_api::types::AgentFormField,
    def: Option<&AgentDto>,
    backends: &[adi_webapp_api::types::AgentBackendOption],
    backend: &str,
) -> AnyView {
    use adi_webapp_api::types::AgentFormFieldKind;

    let name = field.name.clone();
    let defined = defined_value(def, &name);
    let id = format!("adi-run-{}", name.replace('_', "-"));
    let wide = field.wide || matches!(field.kind, AgentFormFieldKind::Textarea);
    // Read once per render rather than per control: the map is one signal, and every control here
    // reads it.
    let set = {
        let name = name.clone();
        move || watch.run_overrides.get().get(&name).cloned()
    };
    let write = {
        let name = name.clone();
        move |value: String| {
            watch.run_overrides.update(|map| {
                if value == INHERIT || value.is_empty() {
                    map.remove(&name);
                } else {
                    map.insert(name.clone(), value);
                }
            });
            save_run_settings(watch);
        }
    };
    // A blank input is "as the agent has it" for a text box, so a *select* has to carry the
    // difference explicitly: `INHERIT` first, then the schema's own options — whose own empty value
    // means "unset it for this run", which is a thing only a select can say.
    let control = match field.kind {
        AgentFormFieldKind::Select | AgentFormFieldKind::Checkbox => {
            let boolean = matches!(field.kind, AgentFormFieldKind::Checkbox);
            let options = if boolean {
                vec![
                    adi_webapp_api::types::AgentFormOption {
                        value: "true".into(),
                        label: "yes".into(),
                    },
                    adi_webapp_api::types::AgentFormOption {
                        value: "false".into(),
                        label: "no".into(),
                    },
                ]
            } else {
                field.options.clone()
            };
            let write = write.clone();
            let (shown, inherited) = (set.clone(), set.clone());
            view! {
                <select class="adi-input" id=id.clone()
                    prop:value=move || shown().unwrap_or_else(|| INHERIT.to_string())
                    on:change=move |ev| write(event_target_value(&ev))>
                    <option value=INHERIT
                        selected=move || inherited().is_none()>{as_defined(&defined)}</option>
                    {options.into_iter().map(|o| {
                        let value = o.value.clone();
                        let chosen = {
                            let set = set.clone();
                            move || set().is_some_and(|v| v == value)
                        };
                        view! {
                            <option value=o.value.clone() selected=chosen>{o.label}</option>
                        }
                    }).collect::<Vec<_>>()}
                </select>
            }
            .into_any()
        }
        AgentFormFieldKind::Textarea => {
            let write = write.clone();
            view! {
                <textarea class="adi-textarea" id=id.clone() rows="2"
                    placeholder=as_defined(&defined)
                    prop:value=move || set().unwrap_or_default()
                    on:input=move |ev| write(event_target_value(&ev))></textarea>
            }
            .into_any()
        }
        _ => {
            // Whatever the schema calls it, it is typed in here: the model picker's chips belong to
            // the editor, where a model is being chosen once, rather than to a panel opened to
            // change one thing about one run.
            let placeholder = if defined.is_empty() {
                super::form::field_placeholder(&field, backends, backend)
            } else {
                as_defined(&defined)
            };
            let numeric = field.numeric;
            let write = write.clone();
            view! {
                <input class="adi-input adi-mono" id=id.clone()
                    type=if numeric { "number" } else { "text" }
                    placeholder=placeholder
                    prop:value=move || set().unwrap_or_default()
                    on:input=move |ev| write(event_target_value(&ev))/>
            }
            .into_any()
        }
    };
    view! {
        <div class="adi-chat__runset-field" class:adi-chat__runset-field--wide=wide>
            <label class="adi-chat__runset-label" for=id>{field.label.clone()}</label>
            {control}
            {(!field.hint.is_empty()).then(|| view! {
                <p class="adi-chat__runset-hint">{field.hint.clone()}</p>
            })}
        </div>
    }
    .into_any()
}

/// What the agent is currently set to for `name`, as a string — the value every control in the panel
/// offers to leave alone. `unattended` is a field of the agent rather than an argument of its
/// backend, and is the one name read from somewhere other than the argument map.
fn defined_value(def: Option<&AgentDto>, name: &str) -> String {
    let Some(def) = def else {
        return String::new();
    };
    if name == "unattended" {
        return if def.unattended { "yes" } else { "no" }.to_string();
    }
    match def.arguments.get(name) {
        Some(serde_json::Value::String(s)) => s.clone(),
        Some(other) => other.to_string(),
        None => String::new(),
    }
}

/// The "leave it alone" label: what the agent says, or that it says nothing.
fn as_defined(defined: &str) -> String {
    if defined.trim().is_empty() {
        "as defined".to_string()
    } else {
        format!("as defined \u{2014} {defined}")
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
pub(crate) fn chat_home_view(state: State, watch: AgentsWatch, l: Launcher) -> AnyView {
    // Here rather than in the rail, which is rebuilt on every poll: this screen is built once (the
    // caller reads only `meta` and `reconfiguring`), so the listener is installed once and torn
    // down when the wizard takes the screen.
    install_session_hotkeys(state, watch);
    view! {
        <div class="adi-chome">
            // Narrow viewports only (the stylesheet hides it above the breakpoint): the two rails
            // have no column of their own down there, so this is the only way to reach them.
            <div class="adi-chome__mobilebar">
                // Down here the sessions column is a drawer, so the brand line inside it cannot
                // be the way into the menu — this bar carries its own.
                {launcher::brand(l, "adi-brand--bar")}
                <button class="adi-chome__drawer-btn" type="button"
                    aria-label="Show sessions"
                    on:click=move |_| toggle_drawer(state, ChatDrawer::Sessions)>
                    <adi_ui::Icon icon=adi_ui::Lucide::Menu/>
                    <span>"Sessions"</span>
                    // Down here the rail is behind this button, so the one thing it says that
                    // cannot wait for somebody to open it is how many chats are stopped on you.
                    {move || {
                        let n = waiting_on_you(state);
                        (n > 0).then(|| view! {
                            <span class="adi-chip" title="chats waiting on you">{n.to_string()}</span>
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
            // narrow viewport); the rail inside it is `adi-ui`'s, and draws its own title and its
            // own scrolling body.
            <aside class="adi-chome__side adi-chome__side--left"
                class:is-open=move || state.chat_drawer.get() == Some(ChatDrawer::Sessions)>
                // What a top bar would have been, in the width it actually needs: the mark, the
                // name and the shortcut, over the rail rather than over the whole viewport.
                {launcher::brand(l, "adi-brand--rail")}
                // "Who else is working right now" — every paired node, active first. Absent on a
                // machine paired with nobody, so it costs the rail nothing there.
                {move || chat_fleet_presence(state)}
                <adi_ui::Rail
                    title="Sessions"
                    actions=move || {
                        view! {
                            {move || chat_session_node(state)}
                            {move || chat_session_filter(state)}
                            {move || chat_new_button(state, watch)}
                            <button class="adi-chome__drawer-close" type="button"
                                on:click=move |_| state.chat_drawer.set(None)>
                                <adi_ui::Icon icon=adi_ui::Lucide::X label="Close"/>
                            </button>
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

            <aside class="adi-chome__side adi-chome__side--right"
                class:is-open=move || state.chat_drawer.get() == Some(ChatDrawer::Right)>
                // Rebuilt when the title changes, which is the same moment the body does: this
                // column is either the dashboards or the open conversation — who is running it,
                // what it has come to, and what can be asked of it — and it swaps whole.
                {move || if showing_analytics(watch) {
                    chat_conversation_panel(state, watch)
                } else {
                    view! {
                        <adi_ui::Rail
                            title="Apps"
                            actions=move || {
                                view! {
                                    {chat_fleet_refresh(state)}
                                    <a class="adi-chat__link" href="/extended/dashboards">"Manage"</a>
                                    <button class="adi-chome__drawer-close" type="button"
                                        on:click=move |_| state.chat_drawer.set(None)>
                                        <adi_ui::Icon icon=adi_ui::Lucide::X label="Close"/>
                                    </button>
                                }
                                .into_any()
                            }
                        >
                            {move || chat_dashboards(state)}
                        </adi_ui::Rail>
                    }
                    .into_any()
                }}

                // The foot of the column, under whichever of the two is showing.
                {chat_foot()}
            </aside>

            // The rail's right-click menu, drawn once here rather than per row: only one is ever
            // open, and it is `position: fixed` at the pointer, so where it sits in the tree is
            // immaterial — outside the scrolling rail keeps it from being clipped by it.
            {move || chat_session_menu(state, watch)}

            // The Sessions head's filter menu, drawn here for the same reasons: one at a time, and
            // it must not be clipped by the rail it hangs off.
            {move || chat_filter_menu(state)}

            // …and the node menu beside it, which is the same control for a different question:
            // not which of these sessions, but whose.
            {move || chat_node_menu(state, watch)}
        </div>
    }
    .into_any()
}

/// Where the donate link goes. Off this machine, so it is written out in full rather than as a
/// path — every other link in this screen is local, and one of them is not.
const DONATE_URL: &str = "https://withadi.dev/mono-donate";

/// Where the docs are.
const DOCS_URL: &str = "https://github.com/adi-family/mono/tree/main/docs";

/// The foot of the right column: two 12px links, under whichever panel is showing. adi runs on
/// this machine and asks for nothing to do it, so this is the one place it asks at all.
///
/// New tabs, not navigations: leaving would take the open conversation off the screen to read a
/// page that has nothing to do with it.
fn chat_foot() -> impl IntoView {
    view! {
        <div class="adi-chat__foot">
            <a href=DONATE_URL target="_blank" rel="noopener noreferrer"
                title="support adi — opens withadi.dev/mono-donate in a new tab">"Donate"</a>
            <a href=DOCS_URL target="_blank" rel="noopener noreferrer">"Docs"</a>
        </div>
    }
}

/// Whether the right rail is currently the open conversation's analytics rather than the dashboards.
///
/// A conversation, not the chat screen, is what the counts are *of* — so the rail only becomes
/// analytics once one is open. The compose screen and a terminal agent (which has a live pane, not a
/// transcript) keep the dashboards, which is the thing worth having in front of you there.
fn showing_analytics(watch: AgentsWatch) -> bool {
    !watch.interactive.get() && watch.run_id.get().is_some()
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
                if feed_turn(None, t, turn).is_empty() {
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
    state.projects.get().map(|p| p.projects).unwrap_or_default()
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
    // Runs on the same source as the conversation being reviewed — the reviewer it launches is a
    // conversation on that machine — so it is carried through to wherever the answer opens.
    let node = watch.node.get_untracked();
    watch.review_busy.set(true);
    spawn_local(async move {
        let result = fetch::review_run(node.as_deref(), name, run_id).await;
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
                    open_watch(watch, node, started.reviewer, true);
                } else {
                    open_session(watch, node, &started.reviewer, &started.run_id);
                }
            }
            Err(e) => state.flash.set(Some(Flash::err(e))),
        }
    });
}

/// Whether a report found nothing at all — which is a result, and has to be said, or an empty section
/// reads as one that failed to load.
fn repeats_and_near_empty(t: &AgentTokens) -> bool {
    t.repeats.is_empty() && t.near_duplicates.is_empty()
}

/// What the right column is called right now — the narrow-viewport button that summons it says
/// so. One function so the button and the column can never disagree.
fn right_rail_title(watch: AgentsWatch) -> &'static str {
    if showing_analytics(watch) {
        "This chat"
    } else {
        "Apps"
    }
}

/// The right column while a conversation is open: what it is, what it has come to, and the two
/// things that can be asked of it — three sections under hairlines (design/examples/chat.html).
fn chat_conversation_panel(state: State, watch: AgentsWatch) -> AnyView {
    view! {
        <div class="adi-chat__panel">
            {move || chat_agent_section(state, watch)}
            {move || chat_stats_section(state, watch)}
            {move || chat_analysis_section(state, watch)}
        </div>
    }
    .into_any()
}

/// **Who** is running this conversation, **where**, and **how** — the first section of the panel.
///
/// Three readings, in the order they get asked: which agent this is and what it is doing; what it
/// is made of; and where the conversation runs, how long it has been going, and what the agent was
/// configured with — the settings that explain behaviour you are looking at, without going to
/// another page to find them.
///
/// Everything here is already in hand — the watch's signals and the listings the shell polls — so
/// the section costs no endpoint of its own. It renders before the first turn lands, too, which is
/// exactly when "which agent am I talking to, and where" is worth answering.
///
/// The state word is ranked the way the sessions rail ranks it — a question outranks a turn in
/// flight, and a wake yields to both (see [`chat_session_row`]) — so one conversation is never
/// described two ways by two surfaces looking at it.
fn chat_agent_section(state: State, watch: AgentsWatch) -> Option<AnyView> {
    let name = watch.name.get()?;
    let run_id = watch.run_id.get()?;
    // The agent's own history when the per-agent poll has filled it, the cross-agent listing
    // otherwise — the same precedence, and the same fallback, as the sessions rail beside this
    // one (see [`session_bands`]). On the chat screen only the second of the two is fetched,
    // and taking the first alone would leave this section timeless there.
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
        (true, _) => "waiting on you",
        (false, Some(r)) => run_status(answerable, r),
        (false, None) if running => "running",
        (false, None) => "idle",
    };
    let tone = match word {
        "waiting on you" => "waiting",
        "answering" | "running" => "working",
        "awaiting" => "awaiting",
        _ => "quiet",
    };

    // What the agent is *made of*: the backend that runs it and the model it runs on. Both are
    // the agent's definition rather than the run's, so they are absent for a conversation whose
    // agent has since been deleted — the run stays readable, it just can't say what ran it.
    let backend = def.as_ref().map(|d| d.backend.clone()).unwrap_or_default();
    let model = def
        .as_ref()
        .and_then(|d| d.arguments.get("model"))
        .and_then(|m| m.as_str())
        .unwrap_or_default()
        .to_string();
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
    // And the rest of what it was launched as. Said here rather than left to be inferred from the
    // settings below, which are the *agent's*: a chat started on another model would otherwise be
    // described, in this very section, by the model it is not running on.
    let overrides = run
        .as_ref()
        .map(|r| r.overrides.clone())
        .filter(|o| !o.is_empty());

    // Settings opens *this* agent's editor rather than the list — the section is already about one
    // agent, so the list was a step the reader had to retrace. A conversation whose agent has since
    // been deleted has no editor to open, so that one still goes to the list.
    //
    // A plain href, not an SPA navigation: this lives in the `Home` mount, and the editor is a
    // route of the `App` shell (see `main`), so reaching it is a real page load either way.
    let (settings_href, settings_title) = match def.as_ref() {
        Some(d) => (agent_form_path(&d.name), "edit this agent's definition"),
        None => (
            Route::Agents.path().to_string(),
            "define and configure agents",
        ),
    };

    Some(
        view! {
            <section class="adi-chat__sec">
                <div class="adi-chat__agent">
                    <span class="adi-chat__agent-name">{name}</span>
                    <span class="adi-chat__state" data-state=tone>{word}</span>
                    <button class="adi-chome__drawer-close" type="button"
                        on:click=move |_| state.chat_drawer.set(None)>
                        <adi_ui::Icon icon=adi_ui::Lucide::X label="Close"/>
                    </button>
                </div>
                <div class="adi-chat__sub">
                    {(!backend.is_empty()).then(|| view! {
                        <code class="adi-mono">{backend.clone()}</code>" · "
                    })}
                    {(!model.is_empty()).then(|| view! {
                        <code class="adi-mono">{model.clone()}</code>" · "
                    })}
                    <a class="adi-chat__link" href=settings_href title=settings_title>"Settings"</a>
                </div>
                <dl class="adi-chat__kv">
                    {(!cwd.is_empty()).then(|| kv("Dir", short_path(&cwd), cwd.clone(), true))}
                    // The ages are read off the clock rather than off the data, so without the
                    // ticker "1h ago" would still say 1h long after it wasn't.
                    {(started > 0).then(|| view! {
                        <dt>"Started"</dt>
                        <dd>
                            {move || {
                                state.secs_since.track();
                                if last > started {
                                    format!("{}, last said {}", run_age(started), run_age(last))
                                } else {
                                    run_age(started)
                                }
                            }}
                        </dd>
                    })}
                    {kv("Run", run_id.clone(), run_id, true)}
                    {overrides.map(|o| kv(
                        "Runs as",
                        o.clone(),
                        format!("this conversation was launched with {o}, over the agent's own \
                                 settings"),
                        true,
                    ))}
                    {def.as_ref().map(chat_agent_settings)}
                    {project.map(|p| kv("Project", p.clone(), p, false))}
                </dl>
            </section>
        }
        .into_any(),
    )
}

/// The settings that explain the behaviour in front of you: how freely the agent may act, and what
/// it carries. Read off the definition, so it is the same answer the Agents page would give.
///
/// Only what is *set* is drawn. An agent with no secrets attached has nothing to say about secrets,
/// and a row reading "Secrets 0" would take a line to say so in a column this narrow.
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
    let permissions = [
        arg("permission_mode"),
        arg("approval_policy"),
        arg("sandbox"),
    ]
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
    view! {
        {permissions.map(|p| kv("Permissions", p.clone(), p, true))}
        {(!carries.is_empty()).then(|| {
            let all = carries.join(", ");
            kv("Carries", all.clone(), all, false)
        })}
        // Unattended changes what the agent *does* — its Ask refuses rather than stopping the
        // work on a question nobody is there to answer — so it is said outright rather than left
        // to be inferred from a missing row.
        {d.unattended.then(|| kv(
            "Runs",
            "unattended".to_string(),
            "the Ask tool refuses; the run decides and says what it assumed".to_string(),
            false,
        ))}
    }
    .into_any()
}

/// One `key   value` pair of the list (§6: keys `--ink-3`, values `--ink-2`, machine values mono).
///
/// `hover` is the whole of a value the column is too narrow to show. `machine` marks a value a
/// machine wrote — a path, a run id, a config value — which is set in mono and never wrapped;
/// everything else wraps rather than hiding its tail behind an ellipsis nobody thinks to hover.
fn kv(key: &'static str, value: String, hover: impl Into<String>, machine: bool) -> AnyView {
    let class = if machine {
        "adi-mono"
    } else {
        "adi-chat__kv-wrap"
    };
    view! {
        <dt>{key}</dt>
        <dd class=class title=hover.into()>{value}</dd>
    }
    .into_any()
}

/// **This chat**: what the open conversation has added up to so far — how much was said, how
/// much work the agent did to say it, which tools did it, and what went wrong on the way.
///
/// The counts come off [`AgentPeek::turns`], the transcript the centre pane already polls each
/// second, so the section is live without an endpoint or a poll of its own.
///
/// Every exception it names is a link. A failed tool call forty entries down a newest-first feed
/// is, in practice, invisible; a panel that could only say "2 failed" would be reporting a problem
/// while withholding its location.
fn chat_stats_section(state: State, watch: AgentsWatch) -> AnyView {
    let _ = state;
    let turns = watch.peek.get().map(|p| p.turns).unwrap_or_default();
    let head = view! { <div class="adi-chat__sec-head"><span>"This chat"</span></div> };
    if turns.is_empty() {
        return view! {
            <section class="adi-chat__sec">
                {head}
                <div class="adi-chat__fine">
                    {if watch.peek.get().is_some() { "Nothing said yet." } else { "Loading\u{2026}" }}
                </div>
            </section>
        }
        .into_any();
    }
    let s = collect_stats(&turns);

    // The third number is what the run cost, or — for a backend that reports tokens but not
    // money — what it took. Absent for one that reports neither: a confident `$0.00` is a claim.
    let third = if s.cost_micro > 0 {
        Some((fmt_cost(s.cost_micro), "spent"))
    } else if s.tokens > 0 {
        Some((fmt_count(s.tokens), "tokens"))
    } else {
        None
    };

    // One line of detail under the numbers. Only the exceptions are named: a run of tool calls
    // that all worked has nothing to add here, and saying "0 failed" every time would make the
    // line that does matter easy to read past.
    let mut fine: Vec<String> = vec![format!("{} you, {} agent", s.you, s.agent)];
    if s.queued > 0 {
        fine.push(format!("{} queued", s.queued));
    }
    if s.cost_micro > 0 && s.tokens > 0 {
        fine.push(format!("{} tokens", fmt_count(s.tokens)));
    }
    if s.work_ms > 0 {
        // Wall clock beside working time is the comparison worth drawing: it separates a slow
        // conversation from one that is merely old.
        if s.last_at > s.first_at {
            fine.push(format!(
                "{} working of {}",
                fmt_duration(s.work_ms),
                fmt_duration(s.last_at - s.first_at)
            ));
        } else {
            fine.push(format!("{} working", fmt_duration(s.work_ms)));
        }
    }
    if s.thinking > 0 {
        fine.push(format!("{} thinking", s.thinking));
    }

    view! {
        <section class="adi-chat__sec">
            {head}
            <div class="adi-chat__stats">
                <div class="adi-chat__stat">
                    <b>{(s.you + s.agent).to_string()}</b><span>"messages"</span>
                </div>
                <div class="adi-chat__stat">
                    <b>{s.tools.to_string()}</b><span>"tool calls"</span>
                </div>
                {third.map(|(value, label)| view! {
                    <div class="adi-chat__stat"><b>{value}</b><span>{label}</span></div>
                })}
            </div>
            <div class="adi-chat__fine">{fine.join(" \u{b7} ")}</div>
            {(!s.by_tool.is_empty()).then(|| tool_breakdown(&s.by_tool))}
            {(!s.failed.is_empty()).then(|| jump_list("Failed", "error", s.failed.clone()))}
            {(!s.running.is_empty()).then(|| jump_list("Running", "running", s.running.clone()))}
            {(!s.errored.is_empty() || s.errored_silent > 0)
                .then(|| errored_list(s.errored.clone(), s.errored_silent))}
            {(!s.blocked.is_empty()).then(|| blocked_list(&s.blocked))}
        </section>
    }
    .into_any()
}

/// **Analysis**: the two things that can be asked of a conversation, and the answer to the second.
///
/// The counts above say what a conversation cost and what went wrong in it; none of them says what
/// to do about any of it. The first button hands the whole session — its configuration and system
/// prompt, the tool-by-tool trace, the failures, the repeats, and how this agent behaves across its
/// other sessions — to the root agent, and asks that question. The answer is not rendered here: it
/// arrives as a conversation with `adi-agent`, which the screen jumps to, because a review you can
/// argue with beats a report card in a panel.
///
/// The second itemizes the context, and stays behind a button rather than loading with the rest:
/// every other number here is arithmetic over a transcript the page already holds, and this one is
/// a tokenizer pass over the whole conversation on the server.
fn chat_analysis_section(state: State, watch: AgentsWatch) -> AnyView {
    view! {
        <section class="adi-chat__sec">
            <div class="adi-chat__sec-head"><span>"Analysis"</span></div>
            <button class="adi-chat__btn-w" type="button"
                disabled=move || watch.review_busy.get()
                title="Hand this conversation to adi-agent and ask how the workflow should have gone"
                on:click=move |_| start_review(state, watch)>
                {move || if watch.review_busy.get() {
                    "Handing it over\u{2026}"
                } else {
                    "Analyze this chat"
                }}
            </button>
            <p class="adi-chat__hint">
                "adi-agent reads the whole session \u{2014} prompt, tools, failures, repeats \u{2014} \
                 and answers with what to change: the workflow, what to harden, what wants a tool."
            </p>
            {chat_token_report(state, watch)}
        </section>
    }
    .into_any()
}

/// Which tools did the work, most-used first — the shape of the run in a few lines. Whether a
/// conversation was a search, a refactor, or a build loop is legible here without reading any of it.
fn tool_breakdown(by_tool: &[(String, usize, usize)]) -> AnyView {
    view! {
        <div class="adi-chat__tools">
            {by_tool.iter().map(|(name, calls, bad)| view! {
                <div class="adi-chat__tool">
                    <span class="adi-chat__tool-name">{name.clone()}</span>
                    {(*bad > 0).then(|| view! {
                        <span class="adi-chat__tool-bad">{format!("{bad} failed")}</span>
                    })}
                    <span class="adi-chat__tool-n">{*calls}</span>
                </div>
            }).collect_view()}
        </div>
    }
    .into_any()
}

/// A band whose rows go somewhere: the failed calls, the running ones. The count is in the heading
/// because the rows below it are the same information spelled out, and a reader who only wants the
/// number should not have to count.
fn jump_list(label: &'static str, status: &'static str, steps: Vec<StepRef>) -> AnyView {
    let n = steps.len();
    view! {
        <div class="adi-chat__band">
            <div class="adi-chat__band-head">{format!("{label} ({n})")}</div>
            {steps.into_iter().map(|s| {
                let anchor = s.anchor.clone();
                let title = format!("show this call in the transcript: {}", s.arg);
                view! {
                    <button class="adi-chat__jump" data-status=status type="button" title=title
                        on:click=move |_| jump_to(&anchor)>
                        <span class="adi-chat__jump-dot" aria-hidden="true"></span>
                        <span class="adi-chat__jump-name">{s.tool}</span>
                        <span class="adi-chat__jump-arg">{s.arg}</span>
                    </button>
                }
            }).collect_view()}
        </div>
    }
    .into_any()
}

/// Turns the engine gave up on. Distinct from a failed tool call, and worth its own band: a tool that
/// failed is something the agent saw and could work around, a turn that errored is one it never
/// finished.
///
/// `silent` are the ones that finished with nothing at all — no step and no word — which the
/// transcript therefore draws nothing for. They are counted in the head and named on a line that
/// is deliberately not a button: there is nothing to jump to, and a link that scrolls nowhere reads
/// as the app being broken rather than as the turn being empty.
fn errored_list(anchors: Vec<String>, silent: usize) -> AnyView {
    let n = anchors.len() + silent;
    view! {
        <div class="adi-chat__band">
            <div class="adi-chat__band-head">{format!("Failed turns ({n})")}</div>
            {anchors.into_iter().enumerate().map(|(i, anchor)| view! {
                <button class="adi-chat__jump" data-status="error" type="button"
                    title="show this turn in the transcript"
                    on:click=move |_| jump_to(&anchor)>
                    <span class="adi-chat__jump-dot" aria-hidden="true"></span>
                    <span class="adi-chat__jump-name">{format!("turn {}", i + 1)}</span>
                </button>
            }).collect_view()}
            {(silent > 0).then(|| view! {
                <div class="adi-chat__jump adi-chat__jump--flat" data-status="error"
                    title="this turn produced no output to show">
                    <span class="adi-chat__jump-dot" aria-hidden="true"></span>
                    <span class="adi-chat__jump-name">
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
/// feed to point at — the band names them and stops there. Worth surfacing anyway, and often the
/// answer to "why is the result wrong": an agent that was refused a write did not decide against it.
fn blocked_list(blocked: &[(String, usize)]) -> AnyView {
    let total: usize = blocked.iter().map(|(_, n)| n).sum();
    view! {
        <div class="adi-chat__band">
            <div class="adi-chat__band-head">{format!("Blocked ({total})")}</div>
            {blocked.iter().map(|(name, n)| view! {
                <div class="adi-chat__jump adi-chat__jump--flat" data-status="blocked">
                    <span class="adi-chat__jump-dot" aria-hidden="true"></span>
                    <span class="adi-chat__jump-name">{name.clone()}</span>
                    {(*n > 1).then(|| view! {
                        <span class="adi-chat__tool-n">{format!("{n}\u{d7}")}</span>
                    })}
                </div>
            }).collect_view()}
        </div>
    }
    .into_any()
}

/// The context itemization: the button, or the report once it has landed.
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

    match ready {
        None => view! {
            <button class="adi-chat__btn-w" type="button" disabled=busy
                on:click=move |_| load_token_report(state, watch)>
                {if busy { "Reading the transcript\u{2026}" } else { "Itemize the context" }}
            </button>
            <p class="adi-chat__hint">
                {if error.is_empty() {
                    "What this conversation's tokens went on, and which runs of text it sent \
                     more than once."
                        .to_string()
                } else {
                    error
                }}
            </p>
        }
        .into_any(),
        Some(t) => token_report_view(state, watch, &t),
    }
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
        <div class="adi-chat__band">
            <div class="adi-chat__band-head">"Context"</div>
            <div class="adi-chat__ctx">
                <b>{format!("\u{2248}{} tokens", fmt_count(total as u64))}</b>
                // An OpenAI BPE counting a conversation that may have been had with any provider:
                // close, and honest about being an estimate rather than the provider's own count.
                <code class="adi-mono" title="estimated with a real BPE; not the provider's own count">
                    {t.encoding.clone()}
                </code>
            </div>
            {(!split.is_empty()).then(|| view! { <div class="adi-chat__fine">{split}</div> })}
            {t.truncated.then(|| view! {
                <div class="adi-chat__fine">"long conversation \u{2014} only the recent end was read"</div>
            })}
        </div>
        {(!repeats.is_empty()).then(|| view! {
            <div class="adi-chat__band">
                <div class="adi-chat__band-head">
                    {format!("Sent twice \u{2014} {} tokens ({share}%)", fmt_count(t.wasted as u64))}
                </div>
                {repeats.into_iter().map(repeat_row).collect_view()}
            </div>
        })}
        {(!near.is_empty()).then(|| view! {
            <div class="adi-chat__band">
                <div class="adi-chat__band-head">{format!("Nearly the same ({})", near.len())}</div>
                {near.into_iter().map(near_dup_row).collect_view()}
            </div>
        })}
        {(repeats_and_near_empty(t)).then(|| view! {
            <div class="adi-chat__fine">"Nothing was sent twice."</div>
        })}
        <div class="adi-chat__band">
            <button class="adi-chat__btn-w" type="button" disabled=move || watch.tokens_busy.get()
                on:click=move |_| load_token_report(state, watch)>"Recount"</button>
        </div>
    }
    .into_any()
}

/// One repeated run: what it cost, how often it was sent, what it was, and what to do about it.
fn repeat_row(r: AgentRepeat) -> AnyView {
    let full = r.preview.clone();
    view! {
        <div class="adi-chat__rep">
            <div class="adi-chat__rep-head">
                <b>{format!("{} tokens", fmt_count(r.wasted as u64))}</b>
                <span>{format!("{}\u{d7}", r.count)}</span>
                <span class="adi-spacer"></span>
                <span>{shape_attr(r.shape)}</span>
            </div>
            <div class="adi-chat__rep-text" title=full>{r.preview}</div>
            {(!r.hint.is_empty()).then(|| view! {
                <div class="adi-chat__rep-hint">{r.hint}</div>
            })}
        </div>
    }
    .into_any()
}

/// One group of near-identical sends — the same file read again after an edit, most often.
fn near_dup_row(g: AgentNearDup) -> AnyView {
    let full = g.preview.clone();
    view! {
        <div class="adi-chat__rep">
            <div class="adi-chat__rep-head">
                <b>{format!("{} tokens", fmt_count(g.wasted as u64))}</b>
                <span>{format!("{}\u{d7}", g.count)}</span>
                <span class="adi-spacer"></span>
                <span>{format!("\u{2248}{} tokens each", fmt_count(g.tokens as u64))}</span>
            </div>
            <div class="adi-chat__rep-text" title=full>{g.preview}</div>
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
    let node = watch.node.get_untracked();
    watch.tokens_busy.set(true);
    watch.tokens_error.set(String::new());
    spawn_local(async move {
        let result = fetch::run_tokens(node.as_deref(), name, run_id.clone()).await;
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
        *open = if *open == Some(which) {
            None
        } else {
            Some(which)
        };
    });
}

/// One option [`chat_agent_picker`] offers: an agent, and which of the rail's currently-selected
/// sources it came from (`docs/fleet.md` §13, multi-select) — `None` for this machine.
struct PickerOption {
    node: Option<String>,
    name: String,
    running: bool,
}

/// The agent chooser — which agent this screen is a chat with, and (with more than one source
/// selected) which of them it starts on. It sits where a new chat is *started* (the composer's own
/// title, and the Start affordance a terminal agent shows in its place) rather than over the
/// sessions list, the way a model chooser sits with the composer it governs: the one thing to settle
/// before typing is who the message goes to, and now, where.
///
/// **Starred agents only** — a fleet grows a long tail of one-off and machine-made agents, and this
/// is a short list to choose a chat from, not a register of everything installed. The agent already
/// on screen is always an option whether or not it is starred, so the control can't misreport what
/// the centre pane is showing; the Agents page is where anything else is reached (and starred).
///
/// Options are drawn from every currently-selected source — this machine's own [`State::agents`],
/// plus one selected node's own slice of [`State::rail_node_agents`] for each node ticked in the
/// rail's node menu — the same sources [`session_bands`] merges into the rail below, read the same
/// way. A remote pty agent is a real option here, not just a rail row: it only ever runs on the
/// machine that defines it, so starting one *is* starting it there. With only this machine selected
/// (the common case, and everything this control offered before multi-select existed) the list is
/// unchanged and carries no source label; ticking anything else besides tags every option with its
/// source, the same rule the rail's own rows use to decide when to print theirs.
///
/// The root agent leads (only when it is local — the one the app is set up around), then each
/// source's own name order, sources in the order the rail merges them; one that is live right now
/// carries a dot. Choosing one repoints the conversation in the centre and the agent "+ New" starts
/// a session on, on whichever source it came from; the rail lists every selected source's sessions
/// either way, so it only moves in that a different row of it is now the open one.
fn chat_agent_picker(state: State, watch: AgentsWatch) -> AnyView {
    let current = watch.name.get().unwrap_or_default();
    let current_node = watch.node.get();
    let multi_source = usize::from(state.session_local.get()) + state.session_nodes.get().len() > 1;

    let mut options: Vec<PickerOption> = Vec::new();
    // A source not yet answered is not the same as one with nothing starred — the hint below has to
    // tell them apart, or a fleet still loading would flash "No starred agents" for a moment.
    let mut loading = false;
    if state.session_local.get() {
        match state.agents.get() {
            Some(list) => options.extend(
                list.agents
                    .into_iter()
                    .filter(|a| a.starred || (current_node.is_none() && a.name == current))
                    .map(|a| PickerOption {
                        node: None,
                        name: a.name,
                        running: a.running,
                    }),
            ),
            None => loading = true,
        }
    }
    let node_agents = state.rail_node_agents.get();
    for node in state.session_nodes.get() {
        match node_agents.get(&node) {
            Some(list) => options.extend(
                list.agents
                    .iter()
                    .filter(|a| {
                        a.starred
                            || (current_node.as_deref() == Some(node.as_str()) && a.name == current)
                    })
                    .map(|a| PickerOption {
                        node: Some(node.clone()),
                        name: a.name.clone(),
                        running: a.running,
                    }),
            ),
            None => loading = true,
        }
    }
    if options.is_empty() {
        let hint = if loading {
            "Loading agents…"
        } else {
            "No starred agents"
        };
        return view! { <span class="adi-chome__pickhint">{hint}</span> }.into_any();
    }
    // The root agent leads — it's the one the app is set up around, and only ever local. A stable
    // sort, so the rest keep the order they arrived in (each source's own name order, sources in
    // [`session_bands`]'s own merge order) behind it.
    options.sort_by_key(|o| !(o.node.is_none() && o.name == ROOT_AGENT));
    let selected_idx = options
        .iter()
        .position(|o| o.node == current_node && o.name == current);
    // Indexed rather than the agent's own name, because a name is only unique on the machine that
    // defines it (`docs/fleet.md` §13): two selected sources may star the same name, and a `<select>`
    // needs one value per option to tell them apart.
    let rows: Vec<AnyView> = options
        .iter()
        .enumerate()
        .map(|(i, o)| {
            let selected = Some(i) == selected_idx;
            // An option carries no markup, so the live dot rides in its text — worth spotting in a
            // collapsed select, which is all you see of the other agents until you open it.
            let mut label = if o.running {
                format!("\u{25CF} {}", o.name)
            } else {
                o.name.clone()
            };
            if multi_source {
                label = format!(
                    "{label} \u{2014} {}",
                    o.node.as_deref().unwrap_or("this machine")
                );
            }
            view! { <option value=i.to_string() selected=selected>{label}</option> }.into_any()
        })
        .collect();
    view! {
        <span class="adi-chome__agentpick">
            <select class="adi-chome__agentsel"
                title="which agent (and source) this chat runs on — starred agents only"
                prop:value=selected_idx.map(|i| i.to_string()).unwrap_or_default()
                on:change=move |ev| {
                    let Ok(idx) = event_target_value(&ev).parse::<usize>() else {
                        return;
                    };
                    if let Some(o) = options.get(idx) {
                        switch_agent(state, watch, o.node.clone(), o.name.clone());
                    }
                }>
                {rows}
            </select>
            // The chevron the native control gives up under `appearance: none`, laid over the room
            // the select reserves for it.
            <span class="adi-chome__agentcaret" aria-hidden="true">
                <adi_ui::Icon icon=adi_ui::Lucide::ChevronDown size=adi_ui::IconSize::Sm/>
            </span>
        </span>
    }
    .into_any()
}

/// Switch the chat home to another agent, on `node` (`docs/fleet.md` §13) — `None` for this machine,
/// otherwise one of the picker's currently-selected sources. Everything on this screen reads `watch`,
/// so pointing it at `node`/`name` moves the rail, the centre pane and "+ New" together. Whether the
/// agent is interactive comes from that source's own loaded list; a name that isn't in it is left
/// alone rather than watched blind.
fn switch_agent(state: State, watch: AgentsWatch, node: Option<String>, name: String) {
    if name.is_empty()
        || (watch.name.get_untracked().as_deref() == Some(name.as_str())
            && watch.node.get_untracked() == node)
    {
        return;
    }
    let Some(interactive) = agent_interactive(state, node.as_deref(), &name) else {
        return;
    };
    point_watch(watch, node, name, interactive);
}

/// Whether `name` is a pty (interactive) agent on `node`, or `None` when that source's loaded agents
/// list holds no such agent — the caller's cue not to point the view at it. This machine's own
/// [`State::agents`] for `None`, or one node's slice of [`State::rail_node_agents`]
/// (`docs/fleet.md` §13).
fn agent_interactive(state: State, node: Option<&str>, name: &str) -> Option<bool> {
    let list = match node {
        None => state.agents.get_untracked()?,
        Some(node) => state.rail_node_agents.get_untracked().get(node)?.clone(),
    };
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

/// Whether a person asked for this run, as the server recorded at its launch.
///
/// The word rather than a flag, and matched exactly: everything else the field can hold —
/// `agent:<name>`, `automation`, and the empty string every session opened before the store began
/// writing this down carries — is *not* a person. See `adi_agents::launcher`.
fn launched_by_human(r: &AgentRunInfo) -> bool {
    r.launched_by == adi_webapp_api::types::LAUNCHED_BY_HUMAN
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

/// The rail's **filter box**, in the Sessions head beside "+ New".
///
/// A fleet grows a long tail of one-off and machine-made agents, and their chats land in the same
/// flat list as the handful of conversations a person actually had. The box narrows the rail two
/// ways, because they are different questions: to the agents starred on the Agents page — the same
/// shortlist the agent picker draws from — or to the sessions a person started, as against the ones
/// agents spawned for themselves ([`SessionFilter`]).
///
/// [`SessionFilter::Mine`] by default: the rail opens on the sessions a person started, because
/// that is the list it exists to be — a fleet that launches its own work buries the handful of
/// conversations you had under hundreds it spawned for itself, and a rail nobody can find their own
/// chat in is worse than one they have to widen. It takes the accent while it is narrowed, because
/// a list showing less than everything has to say so — and at this size the colour is the whole of
/// what says it, which is why it is lit from the first draw rather than only after a choice. The
/// choice is not stored, so a reload comes back to your own sessions.
///
/// A funnel button rather than a dropdown in the head: the head already carries "+ New" and a close
/// ✕ on a 264px rail, and a control wide enough to print "Only started by me" would take the room
/// they need to say anything at all. What it is narrowed to is read from the menu it opens, and
/// from the accent in the meantime.
fn chat_session_filter(state: State) -> AnyView {
    let current = state.session_filter.get();
    let hint = match current {
        SessionFilter::All => "narrow the sessions listed here",
        SessionFilter::Starred => "showing sessions from starred agents only",
        SessionFilter::Mine => {
            "showing sessions a person started \u{2014} not the ones agents started for \
             themselves. Sessions from before ADI recorded who started a run are not listed here."
        }
    };
    view! {
        <button class="adi-chat__head-btn" class:is-on=current != SessionFilter::All type="button"
            title=hint aria-haspopup="menu"
            aria-expanded=move || state.session_filter_menu.get().is_some().to_string()
            on:click=move |ev: web_sys::MouseEvent| open_filter_menu(state, &ev)>
            <adi_ui::Icon icon=crate::icons::Icon::Filter.lucide() label=hint/>
        </button>
    }
    .into_any()
}

/// Drop the filter menu from under the button that was pressed, rather than from the pointer.
///
/// A menu opened at the click lands in a different place each time on a control this small, and
/// half the time over the button itself. The button's own rect is stable, so the menu is always in
/// the same place under it — and a second press closes what the first opened.
fn open_filter_menu(state: State, ev: &web_sys::MouseEvent) {
    use wasm_bindgen::JsCast as _;

    if state.session_filter_menu.get_untracked().is_some() {
        state.session_filter_menu.set(None);
        return;
    }
    // The rect of the button, not of whatever inside it took the click: a press on the `<svg>`
    // reports the glyph's box, which is 5px narrower and would step the menu sideways.
    let at = ev
        .current_target()
        .and_then(|t| t.dyn_into::<web_sys::Element>().ok())
        .map(|el: web_sys::Element| {
            let r = el.get_bounding_client_rect();
            (r.left() as i32, r.bottom() as i32 + 4)
        })
        .unwrap_or((ev.client_x(), ev.client_y()));
    state.session_filter_menu.set(Some(at));
}

/// The filter menu itself: the three narrowings, the current one ticked.
///
/// The same `adi-menu` the rail's right-click menu is, for the same reason it is: a scrim behind it
/// makes the next click anywhere a dismiss, which is the gesture a person tries first on a menu
/// they opened by accident.
fn chat_filter_menu(state: State) -> Option<AnyView> {
    let (x, y) = state.session_filter_menu.get()?;
    let current = state.session_filter.get();
    Some(
        view! {
            <div class="adi-menu__scrim"
                on:click=move |_| state.session_filter_menu.set(None)></div>
            <div class="adi-menu" role="menu" style=format!("left:{x}px; top:{y}px")>
                <div class="adi-menu__head">"Show"</div>
                {SessionFilter::ALL.into_iter().map(|f| {
                    let on = f == current;
                    view! {
                        <button class="adi-menu__item" class:is-on=on type="button"
                            role="menuitemradio" aria-checked=on.to_string()
                            on:click=move |_| {
                                state.session_filter.set(f);
                                state.session_filter_menu.set(None);
                            }>
                            // A tick in a column of its own, so the labels line up under each
                            // other whether or not one of them is carrying it.
                            <span class="adi-menu__tick" aria-hidden="true">
                                {on.then(|| view! {
                                    <adi_ui::Icon icon=adi_ui::Lucide::Check size=adi_ui::IconSize::Sm/>
                                })}
                            </span>
                            {f.label()}
                        </button>
                    }
                }).collect::<Vec<_>>()}
            </div>
        }
        .into_any(),
    )
}

/// Every paired node, active first, as a name beside a status dot (`/api/fleet`'s presence half,
/// ADI-MONO-11) — "who else is working right now" over the sessions rail it sits above.
///
/// `None` on a machine paired with nobody: a strip that can only ever say "nobody" costs the column
/// height to say nothing. (Unlike [`chat_session_node`] under it, which stays whatever the fleet
/// looks like — that one is a control, and where the rail's rows come from is worth a click even
/// when the answer is "only here".) `active` and `last_seen` are read straight off the wire rather
/// than computed here, so this and the Fleet page's own table can never come to answer the question
/// differently.
fn chat_fleet_presence(state: State) -> Option<AnyView> {
    let fleet = state.fleet.get()?;
    if fleet.nodes.is_empty() {
        return None;
    }
    let mut nodes = fleet.nodes;
    nodes.sort_by(|a, b| b.active.cmp(&a.active).then_with(|| a.petname.cmp(&b.petname)));
    Some(
        view! {
            <div class="adi-chome__presence">
                {nodes.into_iter().map(|n| {
                    let title = if n.active {
                        format!("{} \u{2014} active now", n.petname)
                    } else {
                        format!("{} \u{2014} known, not active", n.petname)
                    };
                    let data_state = if n.active { "online" } else { "known" };
                    view! {
                        <span class="adi-status" data-state=data_state title=title>
                            <span class="adi-status__led"></span>
                            <span>{n.petname}</span>
                        </span>
                    }
                }).collect::<Vec<_>>()}
            </div>
        }
        .into_any(),
    )
}

/// The rail's node button: **whose** sessions are merged below (`docs/fleet.md` §13, multi-select).
///
/// Always in the head, including on a machine paired with nobody — where the rail comes from is
/// part of reading the rail, and a control that appears only once a fleet exists is one the
/// operator has to already know about to look for. With one source it is the icon alone, `--ink-3`
/// like the filter beside it; once anything besides this machine alone is selected it takes the
/// accent and prints a summary, because every row under it can be stopped, hidden or deleted, and
/// which machines that reaches is not a fact to leave in a tooltip. The full list is always in the
/// title, whatever the button prints — the short form exists for the head's 264px, not to hide
/// anything the operator needs before clicking Stop.
fn chat_session_node(state: State) -> AnyView {
    // Only once the fleet list has actually answered — while it is in flight the neutral "merged
    // from" wording below is the honest one, since a fleet may be about to arrive.
    let unpaired = state.fleet_nodes.get().is_some_and(|n| n.nodes.is_empty());
    let local = state.session_local.get();
    let nodes = state.session_nodes.get();
    let mut names: Vec<String> = nodes.into_iter().collect();
    names.sort_unstable();
    let total = usize::from(local) + names.len();
    // Exactly this machine, and nothing else — the default, and the one case the button reads
    // exactly as it did before this had a menu of more than two options.
    let multi = !(local && names.is_empty());
    let label = match (local, names.as_slice()) {
        (false, [one]) => Some(one.clone()),
        _ if multi => Some(format!("{total} sources")),
        _ => None,
    };
    let hint = if total == 0 {
        "no sources selected \u{2014} pick this machine or a paired node to see any sessions at all"
            .to_string()
    } else if unpaired {
        // The only source there is. Saying "merged from" here would name a merge that has nothing
        // to merge with, so the title says where the one source is and where a second comes from.
        "sessions on this machine \u{2014} pair a node on the Fleet page to drive its sessions \
         from here too"
            .to_string()
    } else {
        let mut parts: Vec<String> = Vec::new();
        if local {
            parts.push("this machine".to_string());
        }
        parts.extend(names);
        format!(
            "showing sessions merged from {} \u{2014} starting, replying to and stopping a run \
             acts on whichever of these actually owns it",
            parts.join(", ")
        )
    };
    view! {
        <button class="adi-chat__head-btn" class:is-on=multi type="button"
            title=hint.clone() aria-haspopup="menu"
            aria-expanded=move || state.session_node_menu.get().is_some().to_string()
            on:click=move |ev: web_sys::MouseEvent| open_node_menu(state, &ev)>
            <adi_ui::Icon icon=crate::icons::Icon::Node.lucide() label=hint.clone()/>
            {label.map(|l| view! { <span class="adi-chat__head-btn-name">{l}</span> })}
        </button>
    }
    .into_any()
}

/// Drop the node menu from under its button, exactly as [`open_filter_menu`] does — and close the
/// filter menu if that one is open, since both hang off the same 264px head and two menus over each
/// other is one of them unreachable.
fn open_node_menu(state: State, ev: &web_sys::MouseEvent) {
    use wasm_bindgen::JsCast as _;

    state.session_filter_menu.set(None);
    if state.session_node_menu.get_untracked().is_some() {
        state.session_node_menu.set(None);
        return;
    }
    let at = ev
        .current_target()
        .and_then(|t| t.dyn_into::<web_sys::Element>().ok())
        .map(|el: web_sys::Element| {
            let r = el.get_bounding_client_rect();
            (r.left() as i32, r.bottom() as i32 + 4)
        })
        .unwrap_or((ev.client_x(), ev.client_y()));
    state.session_node_menu.set(Some(at));
}

/// The node menu: a checklist, this machine then every paired node — any subset may be ticked at
/// once (`docs/fleet.md` §13, multi-select). Left open after a tick so several sources can be picked
/// in one visit; the scrim (or the button itself) is what closes it.
///
/// A locked node is **listed and disabled**, not dropped. Dropping it would say the node is gone
/// when what is true is that this machine holds no password for it, and the item's title says where
/// to fix that — the same Fleet page that took the password for the dashboards rail. There is only
/// ever one credential per node, so unlocking a node for its dashboards unlocks it for this too.
///
/// With nothing paired the menu is this machine and a line saying so, rather than nothing at all:
/// the button is in the head whatever the fleet looks like, so the click it invites has to land on
/// an answer to "where else could these come from?".
fn chat_node_menu(state: State, watch: AgentsWatch) -> Option<AnyView> {
    let (x, y) = state.session_node_menu.get()?;
    // Empty rather than absent while `/api/fleet/nodes` is still in flight: the button opens this
    // menu from the first paint, and a `None` here would be a click that does nothing at all. The
    // note, though, waits for the answer — "no paired nodes yet" is a claim, not a loading state.
    let fleet = state.fleet_nodes.get();
    let unpaired = fleet.as_ref().is_some_and(|f| f.nodes.is_empty());
    let nodes = fleet.map(|f| f.nodes).unwrap_or_default();
    let local = state.session_local.get();
    let selected = state.session_nodes.get();
    Some(
        view! {
            <div class="adi-menu__scrim"
                on:click=move |_| state.session_node_menu.set(None)></div>
            <div class="adi-menu" role="menu" style=format!("left:{x}px; top:{y}px")>
                <div class="adi-menu__head">"Sessions from"</div>
                <button class="adi-menu__item" class:is-on=local type="button"
                    role="menuitemcheckbox" aria-checked=local.to_string()
                    on:click=move |_| {
                        crate::state::toggle_session_source(state, watch, None, !local);
                    }>
                    <span class="adi-menu__tick" aria-hidden="true">
                        {local.then(|| view! {
                            <adi_ui::Icon icon=adi_ui::Lucide::Check size=adi_ui::IconSize::Sm/>
                        })}
                    </span>
                    "This machine"
                </button>
                {nodes.into_iter().map(|node| {
                    let on = selected.contains(&node.node);
                    let name = node.node.clone();
                    // Locked blocks *adding* it, never *removing* it: a node persisted here from a
                    // past session, then locked from the Fleet page since, must still be one click
                    // to drop — a disabled tick with no way to untick it would strand it in the
                    // selection forever, checked and unreachable at once.
                    let locked_out = node.locked && !on;
                    let title = if node.locked && !on {
                        format!(
                            "{name} is locked here \u{2014} give this machine its password on the \
                             Fleet page first"
                        )
                    } else if on {
                        format!("stop merging {name}'s sessions into the rail")
                    } else {
                        format!("merge {name}'s sessions into the rail, and drive them from here")
                    };
                    view! {
                        <button class="adi-menu__item" class:is-on=on type="button"
                            role="menuitemcheckbox" aria-checked=on.to_string()
                            disabled=locked_out title=title
                            on:click=move |_| {
                                crate::state::toggle_session_source(
                                    state, watch, Some(name.clone()), !on,
                                );
                            }>
                            <span class="adi-menu__tick" aria-hidden="true">
                                {on.then(|| view! {
                                    <adi_ui::Icon icon=adi_ui::Lucide::Check size=adi_ui::IconSize::Sm/>
                                })}
                            </span>
                            {node.node}
                            {node.locked.then(|| view! {
                                <span class="adi-menu__tick" aria-hidden="true">
                                    <adi_ui::Icon icon=adi_ui::Lucide::Lock size=adi_ui::IconSize::Sm/>
                                </span>
                            })}
                        </button>
                    }
                }).collect::<Vec<_>>()}
                {unpaired.then(|| view! {
                    <p class="adi-menu__note">
                        "No paired nodes yet. Pair one on the Fleet page to merge its sessions in \
                         here."
                    </p>
                })}
            </div>
        }
        .into_any(),
    )
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

/// One row of the rail. The list spans every agent *and every selected source*, so a row has to
/// carry both which agent it belongs to and which machine that agent is on — there is no group
/// heading above it to say either (`docs/fleet.md` §13, multi-select).
///
/// `Clone` because the rail lists rows through a keyed `For`, which owns its items.
#[derive(Clone)]
struct SessionRow {
    /// The row's own source — a paired node's petname, or `None` for this machine. Never read off
    /// `state.session_nodes`/`AgentsWatch::node` by a caller acting on a row: this is the one field
    /// that says where a row's own action (open, stop, hide, star, delete) actually goes, because
    /// several sources may be selected at once and only the row itself knows which one it came from.
    node: Option<String>,
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
/// activity first. The [`SessionFilter`] is handed back with them: which of the rail's three
/// emptinesses an empty answer is depends on which narrowing produced it.
///
/// The watched agent's conversations come from `watch.runs` when it has any — that list is updated
/// the moment a chat is deleted or hidden, so the rail doesn't go on showing a row that has just
/// gone — and from the cross-agent index otherwise. A pty agent keeps no run history, so it
/// contributes one row for its live session, sorted as though it moved just now: it is active by
/// definition and has no older timestamp to be placed by. That row shows while the session runs, or
/// while its agent is the one on screen — otherwise there is nothing there to open.
///
/// One selected source's own rows, before the merge across sources, the "Mine" filter, the sort and
/// the bands. This is the per-agent loop [`chat_all_sessions`] used to run once, over one machine's
/// agents; multi-select (`docs/fleet.md` §13) runs it once per selected source instead of copying it
/// per source, and [`session_bands`] concatenates the answers before doing anything else.
///
/// `all`/`agents` are that source's own `/api/agents/runs/all` and `/api/agents` — this machine's
/// [`State::all_chats`]/[`State::agents`] for `node: None`, or one node's slice of
/// [`State::rail_node_chats`]/[`State::rail_node_agents`]. `is_here` is whether the open conversation
/// lives on *this* source: it is what lets the watched agent's row use the fresher `watch.runs` copy
/// and what keeps the ★ filter's "always keep the one on screen" escape hatch from also opening every
/// other source's same-named agent.
///
/// Returns the rows and whether this source's own listing already carried the watched agent — the
/// caller's cue that the "conversation not in any index yet" fallback does not apply to this source.
#[allow(clippy::too_many_arguments)]
fn source_rows(
    state: State,
    watch: AgentsWatch,
    node: Option<&str>,
    all: Option<&AllAgentRuns>,
    agents: Option<&AgentsState>,
    watched: &str,
    is_here: bool,
    filter: SessionFilter,
    now: u64,
) -> (Vec<SessionRow>, bool) {
    // A pty agent's run history is empty either way; the agents list is what says it's live now —
    // this source's own list, since a pty agent only ever runs on the machine that defines it.
    let live: std::collections::HashSet<&str> = agents
        .map(|s| {
            s.agents
                .iter()
                .filter(|a| a.running)
                .map(|a| a.name.as_str())
                .collect()
        })
        .unwrap_or_default();
    // `None` unless the head's ★ is on, in which case only this source's own starred agents are
    // listed — a fact about that machine's agents, never borrowed from a different one.
    let keep: Option<std::collections::HashSet<&str>> = (filter == SessionFilter::Starred).then(
        || {
            let mut keep: std::collections::HashSet<&str> = agents
                .map(|s| {
                    s.agents
                        .iter()
                        .filter(|a| a.starred)
                        .map(|a| a.name.as_str())
                        .collect()
                })
                .unwrap_or_default();
            if is_here && !watched.is_empty() {
                keep.insert(watched);
            }
            keep
        },
    );

    let mut rows: Vec<SessionRow> = Vec::new();
    let mut listed_watched = false;
    for ar in all.iter().flat_map(|a| a.agents.iter()) {
        if keep.as_ref().is_some_and(|k| !k.contains(ar.name.as_str())) {
            continue;
        }
        let is_watched = is_here && ar.name == watched;
        listed_watched |= is_watched;
        if ar.interactive {
            let running = live.contains(ar.name.as_str());
            if running || is_watched {
                rows.push(SessionRow {
                    node: node.map(str::to_string),
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
            if own.is_empty() {
                ar.runs.clone()
            } else {
                paged(own, state)
            }
        } else {
            ar.runs.clone()
        };
        rows.extend(runs.into_iter().filter(|r| !r.hidden).map(|r| SessionRow {
            node: node.map(str::to_string),
            agent: ar.name.clone(),
            when: last_touch(&r),
            running: r.running,
            starred: r.starred,
            run: Some(r),
            hotkey: None,
        }));
    }
    (rows, listed_watched)
}

/// Every visible session, whichever agent it belongs to, in the five bands the rail reads them in:
/// blocked on you, then running, then awaiting a wake, then starred, then the rest — each newest
/// activity first. The [`SessionFilter`] is handed back with them: which of the rail's three
/// emptinesses an empty answer is depends on which narrowing produced it.
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
fn session_bands(state: State, watch: AgentsWatch) -> ([Vec<SessionRow>; 5], SessionFilter) {
    let filter = state.session_filter.get();
    let watched = watch.name.get().unwrap_or_default();
    let watched_node = watch.node.get();
    // The conversation on screen, which no filter may hide — the same escape hatch the ★ gives the
    // watched agent, narrowed to one session because "mine" is a question about sessions.
    let open = watch.run_id.get().unwrap_or_default();
    let now = js_sys::Date::now() as u64;

    // Every selected source's own rows, merged the same way `chat_all_sessions` already merged
    // across agents on one machine — [`source_rows`] is that same per-agent loop, run once per
    // source instead of duplicated per source (`docs/fleet.md` §13, multi-select).
    let mut rows: Vec<SessionRow> = Vec::new();
    let mut listed_watched = false;
    if state.session_local.get() {
        let is_here = watched_node.is_none();
        let all = state.all_chats.get();
        let agents = state.agents.get();
        let (r, lw) = source_rows(
            state,
            watch,
            None,
            all.as_ref(),
            agents.as_ref(),
            &watched,
            is_here,
            filter,
            now,
        );
        rows.extend(r);
        listed_watched |= lw;
    }
    let node_chats = state.rail_node_chats.get();
    let node_agents = state.rail_node_agents.get();
    for node in state.session_nodes.get() {
        let is_here = watched_node.as_deref() == Some(node.as_str());
        let (r, lw) = source_rows(
            state,
            watch,
            Some(&node),
            node_chats.get(&node),
            node_agents.get(&node),
            &watched,
            is_here,
            filter,
            now,
        );
        rows.extend(r);
        listed_watched |= lw;
    }
    // The watched source's own cross-agent index hasn't arrived yet (or doesn't carry this agent):
    // the watch alone still knows what is on screen, which is the one thing the rail must never be
    // missing.
    if !listed_watched && !watched.is_empty() {
        if watch.interactive.get() {
            rows.push(SessionRow {
                node: watched_node.clone(),
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
                node: watched_node.clone(),
                agent: watched.clone(),
                when: last_touch(&r),
                running: r.running,
                starred: r.starred,
                run: Some(r),
                hotkey: None,
            }));
        }
    }
    // "Started by me": applied here rather than inside `source_rows`, because it asks about the
    // session rather than the agent it belongs to and so cannot skip a whole listing the way ★ does.
    //
    // A row with no run record is a pty agent's live terminal — nobody wrote down who opened it, and
    // it is a thing a person opens and sits in front of, so it stays. The conversation on screen
    // stays whoever started it. Everything else has to say `human`: an unattributed session is one
    // nobody recorded, not one a person is owed.
    if filter == SessionFilter::Mine {
        rows.retain(|row| match &row.run {
            None => true,
            Some(r) => {
                launched_by_human(r)
                    || (row.node == watched_node
                        && row.agent == watched
                        && !open.is_empty()
                        && r.run_id == open)
            }
        });
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
    let (mut waiting, rows): (Vec<SessionRow>, Vec<SessionRow>) = rows.into_iter().partition(|r| {
        r.run
            .as_ref()
            .is_some_and(|run| run.pending_question.is_some())
    });
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
    ([waiting, running, awaiting, starred, rest], filter)
}

/// Whether any selected source's cross-agent index holds a listable session *before* the filter —
/// what tells an empty rail apart from a rail emptied by the default narrowing.
///
/// Only run records count. A pty agent contributes a row when it is live, and a live one survives
/// the "mine" filter anyway (nobody records who opened a terminal, so it is never filtered out), so
/// a rail that is empty despite one cannot exist.
fn any_session(state: State) -> bool {
    let has_runs = |all: &AllAgentRuns| all.agents.iter().any(|ar| ar.runs.iter().any(|r| !r.hidden));
    (state.session_local.get() && state.all_chats.get().is_some_and(|all| has_runs(&all)))
        || state
            .rail_node_chats
            .get()
            .values()
            .any(|all| has_runs(all))
}

/// The rail's session list: the five bands, or the one line that says why there are none.
fn chat_all_sessions(state: State, watch: AgentsWatch) -> AnyView {
    let ([waiting, running, awaiting, kept, rest], filter) = session_bands(state, watch);
    if waiting.is_empty()
        && running.is_empty()
        && awaiting.is_empty()
        && kept.is_empty()
        && rest.is_empty()
    {
        // Which emptiness this is: nothing to show, or nothing left after the filter — said apart,
        // so a narrowed rail never reads as "you have no chats". Each says how to get back.
        //
        // "Mine" is the default, so its line is also what a machine with no chats at all would read
        // on its first run — and telling someone their sessions were filtered out when they have
        // none is how a person learns nothing. When the unfiltered index is empty too, the plain
        // first-run line wins.
        let msg = match filter {
            SessionFilter::All => "No chats yet — press New to start one.",
            SessionFilter::Starred => {
                "No chats from starred agents — star one on the Agents page, or show all sessions."
            }
            SessionFilter::Mine if !any_session(state) => "No chats yet — press New to start one.",
            SessionFilter::Mine => {
                "No sessions started by you — the ones agents start for themselves are filtered \
                 out, as are sessions from before ADI recorded who started a run. Show all \
                 sessions from the filter box above to see them."
            }
        };
        return view! { <div class="adi-chome__empty">{msg}</div> }.into_any();
    }
    // Keyed, and that is not tidiness: a row's click handler is bound when the row is
    // *built*, so a plain list that is rebuilt with a different shape — which is exactly what
    // the filter box does — leaves handlers patched onto rows they no longer belong to, and a click
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
                                "{}:{}:{}",
                                row.node.as_deref().unwrap_or(""),
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
    let SessionRow {
        node,
        agent,
        run,
        when,
        running,
        starred,
        hotkey,
    } = item;
    let on_this_agent =
        watch.name.get().as_deref() == Some(agent.as_str()) && watch.node.get() == node;
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
    // The row's own source, appended to its meta line whenever more than one is on screen at once —
    // always, not only for a remote row, so "local" reads as a fact about the row rather than the
    // absence of one (`docs/fleet.md` §13, multi-select). With one source selected (the common case,
    // and everything this rail showed before multi-select existed) the line is unchanged.
    let multi_source = usize::from(state.session_local.get()) + state.session_nodes.get().len() > 1;
    let origin = multi_source
        .then(|| format!(" \u{00b7} {}", node.as_deref().unwrap_or("this machine")))
        .unwrap_or_default();
    let (title, sub, run_id) = match run {
        Some(r) => {
            let t = truncate_task(display_message(&r));
            let t = if t.trim().is_empty() {
                "New chat".to_string()
            } else {
                t
            };
            (
                t,
                format!("{agent} \u{00b7} {}{origin}", run_age(when)),
                r.run_id,
            )
        }
        None => (
            if running {
                "Live session"
            } else {
                "No live session"
            }
            .to_string(),
            format!("{agent} \u{00b7} interactive terminal{origin}"),
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
    let menu = SessionRef::of(node.clone(), &agent, &run_id, &title, false, starred);
    // Only a conversation can be deleted: a pty agent's live session is started and stopped from the
    // centre pane, and keeps no transcript to take with it.
    let del = (!run_id.is_empty()).then(|| {
        let (del_node, del_agent, del_id, del_title) =
            (node.clone(), agent.clone(), run_id.clone(), title.clone());
        view! {
            <button class="adi-chat__row-btn" type="button"
                title="delete this chat and its transcript"
                on:click=move |_| delete_one_run(
                    state, watch, del_node.clone(), del_agent.clone(), del_id.clone(),
                    del_title.clone(),
                )>
                <adi_ui::Icon icon=adi_ui::Lucide::X size=adi_ui::IconSize::Sm
                    label="Delete this chat"/>
            </button>
        }
    });
    // The star toggle, on the same edge and by the same rule — a pty row has no record to mark.
    //
    // Unlike the delete beside it, it does not wait for a hover once the chat *is* starred: the
    // mark is the row's state as much as its control, and a keep that only showed under the cursor
    // would be a keep you had to go looking for to confirm.
    let star = (!run_id.is_empty()).then(|| {
        let (star_node, star_agent, star_id) = (node.clone(), agent.clone(), run_id.clone());
        let hint = if starred {
            "unstar this chat"
        } else {
            "star this chat \u{2014} it stays in the rail and outlives the session cap"
        };
        view! {
            <button class="adi-chat__row-btn" class:is-on=starred type="button"
                title=hint aria-pressed=starred.to_string()
                on:click=move |_| set_session_starred(
                    state, watch, star_node.clone(), star_agent.clone(), star_id.clone(), !starred,
                )>
                <adi_ui::Icon icon=adi_ui::Lucide::Star size=adi_ui::IconSize::Sm label=hint/>
            </button>
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
                        point_watch(watch, node.clone(), agent.clone(), true);
                    } else {
                        open_session(watch, node.clone(), &agent, &run_id);
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
    /// The row's own source (`docs/fleet.md` §13) — `None` for this machine.
    node: Option<String>,
    agent: String,
    /// Empty for an interactive agent's live pty session — it is not a run, keeps no per-run record,
    /// and so has nothing to hide, which is why such a row opens no menu at all.
    run_id: String,
    title: String,
    hidden: bool,
    starred: bool,
}

impl SessionRef {
    fn of(
        node: Option<String>,
        agent: &str,
        run_id: &str,
        title: &str,
        hidden: bool,
        starred: bool,
    ) -> Self {
        Self {
            node,
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
            node: self.node.clone(),
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

/// The rail's right-click menu on a session: which chat it is, then Rename, Star (or Unstar), and
/// Hide (or, for one already put away, Unhide). A full-viewport scrim behind it makes the next
/// click a dismiss, as on the store tree's menu.
///
/// Rename leads because it is the one item here that opens a further prompt rather than acting at
/// once. Star comes next because, of the two flags, it is the one reached from here rather than
/// from the row — the row's own edge carries a star, but the Hidden band's rows have no edge
/// control for it and this menu is the whole of their way to one.
fn chat_session_menu(state: State, watch: AgentsWatch) -> Option<AnyView> {
    let menu = state.session_menu.get()?;
    let SessionMenu {
        node,
        agent,
        run_id,
        title,
        hidden,
        starred,
        x,
        y,
    } = menu;
    let hide_label = if hidden { "Unhide" } else { "Hide" };
    let star_label = if starred { "Unstar" } else { "Star" };
    let head = format!("{title} \u{00b7} {agent}");
    let (rename_node, rename_agent, rename_id, rename_title) =
        (node.clone(), agent.clone(), run_id.clone(), title.clone());
    let (star_node, star_agent, star_id) = (node.clone(), agent.clone(), run_id.clone());
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
                    on:click=move |_| start_rename_session(
                        state, watch, rename_node.clone(), rename_agent.clone(),
                        rename_id.clone(), &rename_title,
                    )>"Rename\u{2026}"</button>
                <button class="adi-menu__item" type="button"
                    on:click=move |_| set_session_starred(
                        state, watch, star_node.clone(), star_agent.clone(), star_id.clone(),
                        !starred,
                    )>{star_label}</button>
                <button class="adi-menu__item" type="button"
                    on:click=move |_| set_session_hidden(
                        state, watch, node.clone(), agent.clone(), run_id.clone(), !hidden,
                    )>{hide_label}</button>
            </div>
        }
        .into_any(),
    )
}

/// Hide a session from the rail, or bring it back. See [`settle_session_change`] for what happens to
/// the answer.
///
/// Nothing is stopped and nothing is deleted — a hidden run keeps working and keeps its transcript.
/// The one thing that does move is the centre pane: hiding the conversation on screen closes it,
/// since a chat still open after being put away is a puzzle about where it went.
fn set_session_hidden(
    state: State,
    watch: AgentsWatch,
    node: Option<String>,
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
        && watch.node.get_untracked() == node
    {
        close_run_view(watch);
    }
    spawn_local(async move {
        let flagged = fetch::hide_run(node.as_deref(), agent.clone(), run_id, hidden).await;
        settle_session_change(state, watch, node, &agent, flagged).await;
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
    node: Option<String>,
    agent: String,
    run_id: String,
    starred: bool,
) {
    state.session_menu.set(None);
    if run_id.is_empty() {
        return;
    }
    spawn_local(async move {
        let flagged = fetch::star_run(node.as_deref(), agent.clone(), run_id, starred).await;
        settle_session_change(state, watch, node, &agent, flagged).await;
    });
}

/// Ask for a new name for a conversation and post it — a prompt rather than a form field, for the
/// reason the fleet's rename uses one (`pages/fleet.rs::start_rename`): a rename names the row it
/// was started from, and carrying that in a form would let the row you meant and the row the form
/// remembers drift apart between the click and the submit.
///
/// A blank answer clears the title rather than setting an empty one, which doubles as the way
/// back to the title `message` derives once a chat has been renamed.
fn start_rename_session(
    state: State,
    watch: AgentsWatch,
    node: Option<String>,
    agent: String,
    run_id: String,
    from: &str,
) {
    state.session_menu.set(None);
    if run_id.is_empty() {
        return;
    }
    let Some(to) = prompt(&format!("Rename “{from}” to:"), from) else {
        return;
    };
    let to = to.trim().to_string();
    if to == from {
        return;
    }
    spawn_local(async move {
        let flagged = fetch::rename_run(node.as_deref(), agent.clone(), run_id, to).await;
        settle_session_change(state, watch, node, &agent, flagged).await;
    });
}

/// What the rail's row mutations — the two flag toggles, and a rename — do with their answer.
///
/// The endpoint replies with that agent's fresh history, so the row settles into its new band at
/// once instead of at the socket's next tick — and the mutated row's own source's cross-agent index
/// (the Starred, Recent and Hidden bands' data) is re-fetched for the same reason: this machine's
/// [`State::all_chats`] for `node: None`, or one node's slice of [`State::rail_node_chats`]
/// (`docs/fleet.md` §13). Refetching it **at the rail's current page**, not without a limit — an
/// unlimited answer would widen the rail to the whole index until the socket's next answer narrowed
/// it back.
async fn settle_session_change(
    state: State,
    watch: AgentsWatch,
    node: Option<String>,
    agent: &str,
    flagged: Result<AgentRuns, String>,
) {
    match flagged {
        Ok(runs) => {
            if watch.name.get_untracked().as_deref() == Some(agent)
                && watch.node.get_untracked() == node
            {
                watch.runs.set(runs.runs);
            }
            let limit = Some(state.rail_limit.get_untracked());
            match node {
                None => {
                    if let Ok(all) = fetch::all_agent_runs(limit).await {
                        state.all_chats.set(Some(all));
                    }
                }
                Some(node) => {
                    if let Ok(all) = fetch::all_agent_runs_on(&node, limit).await {
                        state.rail_node_chats.update(|m| {
                            m.insert(node, all);
                        });
                    }
                }
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
        let Some(row) = session_bands(state, watch)
            .0
            .into_iter()
            .flatten()
            .nth(n - 1)
        else {
            return;
        };
        ev.prevent_default();
        match row.run {
            Some(run) => open_session(watch, row.node.clone(), &row.agent, &run.run_id),
            // A pty agent's row *is* the agent — there is no run to select, only a screen to point.
            None => point_watch(watch, row.node, row.agent, true),
        }
        // As with a click: on a narrow viewport the rail is a drawer laid over the chat, and the
        // chat you just picked would open behind the list you picked it from.
        state.chat_drawer.set(None);
    });
    on_cleanup(move || handle.remove());
}

/// Open one session from anywhere in the rail: repoint the whole screen when it belongs to another
/// agent (or the same-named agent on a different source), or only select the conversation when it is
/// already the picked one — so a click on a chat of the agent already on screen doesn't tear the
/// centre pane down and rebuild it. `node` is the row's own source (`docs/fleet.md` §13).
fn open_session(watch: AgentsWatch, node: Option<String>, agent: &str, run_id: &str) {
    if watch.name.get_untracked().as_deref() == Some(agent) && watch.node.get_untracked() == node {
        select_run(watch, run_id.to_string());
    } else {
        point_conversation(watch, node, agent.to_string(), run_id.to_string(), false);
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
    // One selected source's own hidden rows, tagged with where they came from — the same per-source
    // merge `session_bands` runs above, kept separate because this band reads `all_chats` /
    // `rail_node_chats` directly rather than through `source_rows` (`docs/fleet.md` §13).
    let starred_only = state.session_filter.get() == SessionFilter::Starred;
    let collect = |node: Option<String>,
                   all: Option<AllAgentRuns>,
                   agents: Option<AgentsState>|
     -> Vec<(Option<String>, String, AgentRunInfo)> {
        let Some(all) = all else {
            return Vec::new();
        };
        // The ★ narrows this band with the list above it, from *this source's* own starred agents —
        // one filter over the whole rail, or unhiding would offer chats the rail has just been told
        // not to show.
        let keep: Option<std::collections::HashSet<String>> = starred_only.then(|| {
            agents
                .map(|s| {
                    s.agents
                        .into_iter()
                        .filter(|a| a.starred)
                        .map(|a| a.name)
                        .collect()
                })
                .unwrap_or_default()
        });
        all.agents
            .into_iter()
            .filter(|ar| keep.as_ref().is_none_or(|k| k.contains(&ar.name)))
            .flat_map(|ar| {
                let node = node.clone();
                let name = ar.name;
                ar.runs
                    .into_iter()
                    .filter(|r| r.hidden)
                    .map(move |r| (node.clone(), name.clone(), r))
            })
            .collect()
    };

    let mut rows: Vec<(Option<String>, String, AgentRunInfo)> = Vec::new();
    if state.session_local.get() {
        rows.extend(collect(None, state.all_chats.get(), state.agents.get()));
    }
    let node_chats = state.rail_node_chats.get();
    let node_agents = state.rail_node_agents.get();
    for node in state.session_nodes.get() {
        rows.extend(collect(
            Some(node.clone()),
            node_chats.get(&node).cloned(),
            node_agents.get(&node).cloned(),
        ));
    }
    if rows.is_empty() {
        return None;
    }
    rows.sort_by_key(|(_, _, r)| std::cmp::Reverse(last_touch(r)));
    let open = state.show_hidden.get();
    let label = format!("Hidden \u{00b7} {}", rows.len());
    let chevron = if open {
        adi_ui::Lucide::ChevronDown
    } else {
        adi_ui::Lucide::ChevronRight
    };
    let body = open.then(|| {
        rows.into_iter()
            .map(|(node, agent, r)| chat_hidden_row(state, watch, node, &agent, &r))
            .collect::<Vec<_>>()
    });
    Some(
        view! {
            <button class="adi-chome__divider adi-chome__divider--toggle" type="button"
                title="sessions hidden from the rail"
                aria-expanded=open.to_string()
                on:click=move |_| state.show_hidden.update(|v| *v = !*v)>
                <span class="inline-flex items-center gap-1.5">
                    <adi_ui::Icon icon=chevron size=adi_ui::IconSize::Sm/>
                    {label}
                </span>
            </button>
            {body}
        }
        .into_any(),
    )
}

/// One hidden session: the same strip as any other agent's, dimmed under the Hidden band, with an
/// unhide (↩) at its right edge in place of the delete — putting a chat back is the one thing this
/// band is for, so it doesn't hide behind the right-click menu.
fn chat_hidden_row(
    state: State,
    watch: AgentsWatch,
    node: Option<String>,
    agent: &str,
    r: &AgentRunInfo,
) -> AnyView {
    let title = truncate_task(display_message(r));
    let title = if title.trim().is_empty() {
        "New chat".to_string()
    } else {
        title
    };
    let multi_source = usize::from(state.session_local.get()) + state.session_nodes.get().len() > 1;
    let origin = multi_source
        .then(|| format!(" \u{00b7} {}", node.as_deref().unwrap_or("this machine")))
        .unwrap_or_default();
    let sub = format!("{agent} \u{00b7} {}{origin}", run_age(last_touch(r)));
    let dot = if r.running {
        "adi-chome__dot adi-chome__dot--on"
    } else {
        "adi-chome__dot"
    };
    let hint = format!("open this hidden chat with {agent}");
    let menu = SessionRef::of(node.clone(), agent, &r.run_id, &title, true, r.starred);
    let (open_node, open_name, open_id) = (node.clone(), agent.to_string(), r.run_id.clone());
    let (show_node, show_name, show_id) = (node, agent.to_string(), r.run_id.clone());
    view! {
        <div class="adi-chome__sessionrow">
            <button class="adi-chome__session adi-chome__session--hidden"
                type="button" title=hint
                on:click=move |_| open_session(watch, open_node.clone(), &open_name, &open_id)
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
                    state, watch, show_node.clone(), show_name.clone(), show_id.clone(), false,
                )>
                <adi_ui::Icon icon=adi_ui::Lucide::Undo2 size=adi_ui::IconSize::Sm
                    label="Bring this chat back"/>
            </button>
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
        let node = watch.node.get();
        view! {
            <button class="adi-btn adi-btn--sm" type="button"
                title="start a fresh session"
                on:click=move |_| {
                    let agents = match &node {
                        None => state.agents.get_untracked(),
                        Some(n) => state.rail_node_agents.get_untracked().get(n).cloned(),
                    };
                    run_now_with(state, node.clone(), name.clone(),
                        at_run_limit(agents.as_ref(), &name),
                        run_dir_of(watch), run_overrides_of(state, watch));
                }>"New"</button>
        }
        .into_any()
    } else {
        view! {
            <button class="adi-btn adi-btn--sm" type="button"
                title="start a new chat"
                on:click=move |_| close_run_view(watch)>"New"</button>
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
                                // A terminal agent has no composer, so its run settings hang here —
                                // beside the one control that starts it, which is the only moment
                                // "where, and as what" can still be answered.
                                <p class="adi-chome__pickline">
                                    {move || run_settings_button(watch)}
                                </p>
                                {move || run_settings_panel(state, watch)}
                                <button class="adi-btn adi-btn--primary" type="button"
                                    on:click=move |_| {
                                        let node = watch.node.get_untracked();
                                        let agents = match &node {
                                            None => state.agents.get_untracked(),
                                            Some(n) => state.rail_node_agents.get_untracked().get(n).cloned(),
                                        };
                                        run_now_with(state, node, name.clone(),
                                            at_run_limit(agents.as_ref(), &name),
                                            run_dir_of(watch), run_overrides_of(state, watch));
                                    }>
                                    "Start session"
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
            <div class="adi-chome__inbox">
                <adi_ui::RailGroup label="Waiting on you" count=total>
                    <For
                        each=move || shown.get_value()
                        key=|row: &SessionRow| {
                            format!(
                                "{}:{}:{}",
                                row.node.as_deref().unwrap_or(""),
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
    let live: Vec<Dashboard> = ds
        .dashboards
        .into_iter()
        .filter(|d| !d.is_archived())
        .collect();
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
    let mut out = vec![view! { <div class="mt-4 border-t border-line pt-1"></div> }.into_any()];
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
