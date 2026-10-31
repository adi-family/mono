//! Dispatch tick — the sweep that turns a ready, assigned task into a run.
//!
//! Beside [`goals::tick`](crate::goals) and [`awaits::tick`](crate::awaits): this crate holds the
//! store and the decision, and the app owns the clock (`adi-app/src/awaits.rs`'s once-a-second
//! worker). Nothing here polls, listens, or spawns on its own.
//!
//! # What gets dispatched
//!
//! A task is a candidate when [`adi_tasks::EffectiveStatus::Ready`] (open, no open child),
//! `assignee` names an agent — not a person; a name this build does not know as an agent is read
//! as a person's, and the dispatcher never guesses — and that agent has opted in
//! ([`AgentManifest::dispatch`](crate::AgentManifest::dispatch)). A task assigned to a person is
//! never touched here, at any point in its life.
//!
//! # Liveness, never trusting `adi.agents.run.finished`
//!
//! "Is this task currently being worked" (rule 7b) is derived fresh every tick from
//! [`Agents::runs`] — the same pass the panel's own listing takes, including the noticing that
//! writes an ending's outcome down the first time anybody looks — never from an event. A task is
//! **working**, and never redispatched, while its last dispatched run is `Running`/`Waiting`, or
//! its conversation still has an open goal: either one means something is going to move on its
//! own, and redispatching over the top of it would double-run the task.
//!
//! The goal is retired the moment the run itself is evaluated below (whatever that evaluation
//! decides), rather than left open until the nudge cap reaches it minutes later — its only job was
//! giving a run the chance to self-report while its conversation was still quiet, and a run with
//! no process left to answer never will. That is what lets an *uncounted* ending retry
//! immediately. A run that is still genuinely `Waiting` (a pending await, queue, or question) is
//! never evaluated at all, which is what leaves the nudge cap, below, as its own separate path —
//! for exactly the case the goal cannot resolve the ordinary way: a run that keeps ending turns
//! without ever meeting or giving up on it.
//!
//! # The counter the spec asks for is not a count of runs
//!
//! Runs get pruned past [`MAX_SESSIONS`](crate::store::MAX_SESSIONS); the attempt budget cannot
//! be. [`DispatchRow`] is the counter row the dispatcher owns: `attempts` (toward a budget
//! of [`ATTEMPT_BUDGET`]) and `launch_failures` (a tighter [`LAUNCH_FAILURE_BUDGET`], since a
//! launch that cannot even start rarely fixes itself on a bare retry). A platform kill/restart —
//! no engine verdict at all, because nothing survived to write one — and a bare `api_error` do
//! **not** count; see [`fold_outcome`]. Backoff is derived from `attempts`, never stored
//! ([`backoff_secs`]).
//!
//! # Stuck
//!
//! A task is stuck when it gave up, hit either budget above, or — independently of all of that —
//! its conversation's goal has been nudged [`NUDGE_CAP`] times with nothing to show for it (a live
//! loop that is not going anywhere is still stuck, even though it is still "working" and must not
//! be redispatched out from under it). Stuck raises exactly one [`Ask`](crate::store::Ask) — retry
//! once, reassign, or archive — read back and acted on, never repeated while one is already
//! pending. A `Running` task whose log has gone silent for an hour gets its own, separate ask
//! ([`SILENCE_ALARM_SECS`]) and is never auto-relaunched.

use std::collections::HashMap;

use adi_config::now_unix;

use crate::launcher;
use crate::progress::capabilities;
use crate::store::{Answer, AskRequest, Choice, DispatchRow, Question, SetBy};
use crate::{Agents, Error, Launch, LaunchOptions, RunInfo, RunLifecycle, StoredAgent};

/// Counted attempts a task may be launched before it is stuck.
const ATTEMPT_BUDGET: u32 = 3;

/// A tighter budget for `launch_failed` endings specifically — a launch the platform could not
/// even start (a missing binary, a bad spec) rarely starts working on the third try the way a
/// run that merely crashed sometimes does.
const LAUNCH_FAILURE_BUDGET: u32 = 2;

/// Nudges with no change to the task before its own goal is read as stuck, independent of whether
/// the conversation is still technically "working".
const NUDGE_CAP: u64 = 12;

/// The shortest gap between two dispatches of the same task, counted attempt or not.
const DISPATCH_FLOOR_SECS: u64 = 30;

/// Backoff after the first counted failure.
const BACKOFF_FIRST_SECS: u64 = 2 * 60;

/// Backoff after the second (and every later) counted failure.
const BACKOFF_SECOND_SECS: u64 = 10 * 60;

/// How long a `Running` task's log may sit untouched before the silence alarm fires.
const SILENCE_ALARM_SECS: u64 = 60 * 60;

/// One task a tick actually launched — what the app logs, mirroring
/// [`goals::Nudged`](crate::goals::Nudged) and [`awaits::Woken`](crate::awaits::Woken). Everything
/// a tick decided *not* to do (waiting on backoff, waiting on the cap, already working) is not an
/// event; only a launch is.
#[derive(Debug, Clone)]
pub struct Dispatched {
    pub task_id: String,
    pub agent: String,
    pub run_id: String,
}

/// Sweep every ready task, dispatching the ones that are eligible and are not already being
/// worked. Called by the app on its own clock; a pass over a store with nothing ready costs one
/// task listing and no agent reads at all.
#[must_use]
pub fn tick(agents: &Agents) -> Vec<Dispatched> {
    let tasks = adi_tasks::Tasks::with_config(agents.config().clone());
    let now = now_unix();
    // One `Agents::runs` per agent per tick, however many of its tasks are being considered — the
    // same listing the panel already pays for, shared rather than repeated.
    let mut runs_cache: HashMap<String, Vec<RunInfo>> = HashMap::new();
    ready_tasks(&tasks)
        .into_iter()
        .filter_map(|task| {
            let agent = assigned_opted_in_agent(agents, &task)?;
            process_one(agents, &tasks, &task, &agent, &mut runs_cache, now)
        })
        .collect()
}

/// Every task whose effective status is `ready`, whatever project or workflow it belongs to.
fn ready_tasks(tasks: &adi_tasks::Tasks) -> Vec<adi_tasks::Task> {
    tasks
        .list(None, None, None, Some(adi_tasks::EffectiveStatus::Ready))
        .unwrap_or_default()
        .into_iter()
        .map(|view| view.task)
        .collect()
}

/// The agent `task` is assigned to, if it is one this build may dispatch to at all: a known
/// agent (not a person), opted in, and on a backend that can be sent a task (never an interactive
/// pty — there is no run to track a message into).
fn assigned_opted_in_agent(agents: &Agents, task: &adi_tasks::Task) -> Option<StoredAgent> {
    let assignee = task.assignee.as_deref()?;
    let agent = agents.get(assignee).ok().flatten()?;
    if !agent.manifest.dispatch || !capabilities(agent.manifest.runtime()).answerable {
        return None;
    }
    Some(agent)
}

/// The whole per-task decision: evaluate what the last attempt did, decide whether this is stuck,
/// and either dispatch a fresh attempt or leave it exactly as it is. `None` whenever nothing was
/// launched — waiting is not an event.
fn process_one(
    agents: &Agents,
    tasks: &adi_tasks::Tasks,
    task: &adi_tasks::Task,
    agent: &StoredAgent,
    runs_cache: &mut HashMap<String, Vec<RunInfo>>,
    now: u64,
) -> Option<Dispatched> {
    let store = agents.sessions();
    let mut row = store.dispatch_row(&task.id).ok().flatten().unwrap_or(DispatchRow {
        task_id: task.id.clone(),
        ..DispatchRow::default()
    });

    // The last attempt's own lifecycle, read once — `None` for a task never yet attempted, or one
    // whose run has aged out of the listing (pruned past `MAX_SESSIONS`) before this got to read
    // it; rare, and nothing is left to evaluate either way.
    let info = (!row.run_id.is_empty())
        .then(|| {
            runs_cache
                .entry(agent.name.clone())
                .or_insert_with(|| agents.runs(agent))
                .iter()
                .find(|r| r.run_id == row.run_id)
                .cloned()
        })
        .flatten();

    let goals = if row.run_id.is_empty() {
        Vec::new()
    } else {
        crate::goals::of_conversation(agents, &row.agent, &row.run_id)
    };
    let mut open_goal = goals.iter().any(crate::store::Goal::open);
    let nudge_capped = goals.iter().any(|g| g.open() && g.nudges >= NUDGE_CAP);

    // Evaluated as soon as the run itself is no longer live — independent of its goal. A goal left
    // open by a run that is otherwise genuinely finished is retired right here, rather than left
    // for the nudge cap to reach minutes later: its only job was giving the run a chance to
    // self-report while its conversation was still quiet, and a run with no process left to answer
    // it never will. Retiring it immediately is what lets an *uncounted* ending (a platform kill,
    // an `api_error`) retry at once instead of waiting out several minutes of nudges into a
    // conversation nothing will ever answer; a genuinely still-cycling run (`Waiting`) never
    // reaches this at all, which is exactly what leaves the nudge cap as its own, separate path.
    if let Some(info) = &info
        && !matches!(info.state, RunLifecycle::Running | RunLifecycle::Waiting)
        && row.counted_run_id != row.run_id
    {
        fold_outcome(&mut row, info);
        row.counted_run_id.clone_from(&row.run_id);
        let _ = store.save_dispatch_row(&row);
        for goal in goals.iter().filter(|g| g.open()) {
            let _ = crate::goals::give_up(
                agents,
                &goal.id,
                "the run ended before meeting or giving up on it",
            );
        }
        open_goal = false;
    }

    // Gave up, nudged past the cap with nothing to show for it, or burned through its budget:
    // stuck, independent of whether the conversation still reads as "working" below — a live loop
    // going nowhere, or a budget already spent, is worth Igor's attention regardless.
    if task.give_up_reason.is_some()
        || nudge_capped
        || row.attempts >= ATTEMPT_BUDGET
        || row.launch_failures >= LAUNCH_FAILURE_BUDGET
    {
        let reason = if task.give_up_reason.is_some() {
            "the agent gave up"
        } else if nudge_capped {
            "its goal has been asked about more than a dozen times with nothing to show for it"
        } else {
            "it has failed as many times as it is allowed to"
        };
        handle_stuck(tasks, task, &store, &mut row, reason);
        let _ = store.save_dispatch_row(&row);
        return None;
    }

    // Not stuck. A run still `Running`/`Waiting`, or one whose goal is still open (the nudge loop
    // may yet bring it back), is still being worked — redispatching over the top of it would run
    // the task twice.
    if let Some(info) = &info {
        let working = matches!(info.state, RunLifecycle::Running | RunLifecycle::Waiting) || open_goal;
        if working {
            if info.state == RunLifecycle::Running {
                silence_alarm(&store, &mut row, now);
                let _ = store.save_dispatch_row(&row);
            }
            return None;
        }
    }

    if now.saturating_sub(row.last_attempt_at) < backoff_secs(row.attempts) {
        return None;
    }
    if agents.at_capacity_for(agent) {
        // At the cap: this task waits, and nothing about it — attempts, backoff, the last attempt
        // time — changes. The next tick tries again, exactly as if this one had never happened.
        return None;
    }

    dispatch(agents, &store, task, agent, &mut row, now)
}

/// Backoff after `attempts` counted failures: none yet is the floor alone, one is two minutes, two
/// or more is ten. Derived, never stored — see the module docs.
fn backoff_secs(attempts: u32) -> u64 {
    match attempts {
        0 => DISPATCH_FLOOR_SECS,
        1 => BACKOFF_FIRST_SECS,
        _ => BACKOFF_SECOND_SECS,
    }
}

/// Fold one finished run's outcome into the counters, by the engine's own verdict (or the lack of
/// one).
///
/// No verdict at all — [`RunInfo::outcome`] absent, or present with no `terminal_reason` — is read
/// as the platform having lost the run out from under it (a kill, a restart) rather than the task's
/// own doing, because an engine that actually got to finish always leaves one. `api_error` is the
/// platform's load, not the task's fault, either. Everything else, `launch_failed` and
/// `launch_lost` (ADI-MONO-218) above all, counts.
fn fold_outcome(row: &mut DispatchRow, info: &RunInfo) {
    match info.outcome.as_ref().and_then(|o| o.terminal_reason.as_deref()) {
        None | Some("api_error") => {}
        // Neither ever reached an engine: `launch_failed` means `send` ran and said why,
        // `launch_lost` means the launcher died before `send` was even called. Both are the
        // platform failing to start the task rather than the task itself failing, so both are
        // weighed against the tighter budget.
        Some(reason) if reason == crate::LAUNCH_FAILED || reason == crate::LAUNCH_LOST => {
            row.attempts += 1;
            row.launch_failures += 1;
        }
        Some(_) => row.attempts += 1,
    }
}

/// Launch a fresh attempt: `adi-mono tasks show <id>` as the pre-run, the task's own `cwd`, stamped
/// as automation, with a goal of finishing it.
fn dispatch(
    agents: &Agents,
    store: &crate::store::SessionStore,
    task: &adi_tasks::Task,
    agent: &StoredAgent,
    row: &mut DispatchRow,
    now: u64,
) -> Option<Dispatched> {
    let pre_run = [format!("adi-mono tasks show {}", task.id)];
    let message = format!("Work task {}: {}", task.id, task.title);
    let result = agents.launch(
        &agent.name,
        &message,
        &LaunchOptions {
            working_dir: task.cwd.as_deref(),
            pre_run: &pre_run,
            launched_by: Some(launcher::AUTOMATION),
            ..LaunchOptions::default()
        },
    );
    match result {
        Ok(Launch::Process { run_id, .. }) => {
            row.agent.clone_from(&agent.name);
            row.run_id.clone_from(&run_id);
            row.last_attempt_at = now;
            row.silence_ask_id.clear();
            let _ = store.save_dispatch_row(row);
            let _ = crate::goals::create(
                agents,
                &agent.name,
                &run_id,
                &format!("task {} is done", task.id),
                SetBy::Automation,
            );
            Some(Dispatched {
                task_id: task.id.clone(),
                agent: agent.name.clone(),
                run_id,
            })
        }
        Ok(Launch::Pty { .. }) => {
            // `assigned_opted_in_agent` already filters on `capabilities(..).answerable`, which a
            // pty backend never is — reachable only if an agent is repointed mid-tick. Nothing to
            // track a task into; leave the row untouched and try again once it is fixed.
            tracing::warn!(
                task = %task.id, agent = %agent.name,
                "dispatch resolved to an interactive backend, which has no task to send — give \
                 this agent a headless backend or take it out of dispatch"
            );
            None
        }
        // The cap filled between this tick's own check and the launch itself — two dispatches (or
        // a human) racing the same boundary. The task waits for the next tick, exactly as if this
        // one had not been tried at all.
        Err(Error::TooManyRunning { .. }) => None,
        Err(e) => {
            // Nothing was created to carry this outcome through the ordinary evaluation path above
            // — there is no run at all — so it is counted here, synchronously, the same two ways a
            // `launch_failed` ending folds in once a session exists to report one.
            row.attempts += 1;
            row.launch_failures += 1;
            row.last_attempt_at = now;
            let _ = store.save_dispatch_row(row);
            tracing::warn!(task = %task.id, agent = %agent.name, error = %e, "dispatch could not launch");
            None
        }
    }
}

/// One option in the stuck ask — a `Choice` whose label `act_on_stuck_answer` matches back.
fn choice(label: &str, description: &str) -> Choice {
    Choice {
        label: label.to_string(),
        description: description.to_string(),
    }
}

/// Raise the stuck ask if none is out yet, or read back and act on the one that is.
///
/// Idempotent by construction: a pending ask (this one or, for the `run_id` is empty case, none at
/// all) is left exactly as it is, an answered one is acted on and cleared, and nothing here is
/// asked twice for the same reason.
fn handle_stuck(
    tasks: &adi_tasks::Tasks,
    task: &adi_tasks::Task,
    store: &crate::store::SessionStore,
    row: &mut DispatchRow,
    reason: &str,
) {
    if row.run_id.is_empty() {
        // Every real attempt opens a conversation, even one that fails before the engine starts
        // (`send_or_note_failure` still appends to the session it just created) — the one way
        // there is none at all is a launch that was refused before a session ever existed (an
        // unrunnable agent's config). Rare, and there is nowhere to ask: say so in the log and
        // leave the task to a human reading it, rather than inventing a conversation from nothing.
        tracing::warn!(task = %task.id, %reason, "task is stuck with no run to ask through");
        return;
    }
    if !row.stuck_ask_id.is_empty() {
        if let Some(ask) = store
            .question_history(&row.agent, &row.run_id)
            .into_iter()
            .find(|a| a.id == row.stuck_ask_id)
            && let Some(answer) = &ask.answer
        {
            act_on_stuck_answer(tasks, task, row, answer);
        }
        return;
    }
    // A different ask is already pending on this conversation — the silence alarm, most likely.
    // Wait for it rather than erroring on a second `ask`.
    if store.pending_question(&row.agent, &row.run_id).is_some() {
        return;
    }
    let req = AskRequest {
        note: format!(
            "task {} is stuck: {reason}. Pick one — the dispatcher acts on whichever you choose.",
            task.id
        ),
        questions: vec![Question {
            header: "Stuck task".into(),
            question: format!("{} ({}) is stuck. What now?", task.id, task.title),
            options: vec![
                choice("retry once", "Clear the attempt count and try again."),
                choice(
                    "reassign",
                    "Take it off this agent so somebody else can pick it up.",
                ),
                choice("archive", "Drop the task; nobody will work it."),
            ],
            multi_select: false,
        }],
        ..AskRequest::default()
    };
    match store.ask(&row.agent, &row.run_id, &req) {
        Ok(ask) => row.stuck_ask_id = ask.id,
        Err(e) => tracing::warn!(task = %task.id, error = %e, "could not raise the stuck ask"),
    }
}

/// Act on Igor's answer to a stuck ask, by its label — anything else (free text typed instead of a
/// tap) is left stuck and logged rather than guessed at.
fn act_on_stuck_answer(
    tasks: &adi_tasks::Tasks,
    task: &adi_tasks::Task,
    row: &mut DispatchRow,
    answer: &Answer,
) {
    let reply = answer
        .replies
        .first()
        .map(|r| r.trim().to_ascii_lowercase())
        .unwrap_or_default();
    match reply.as_str() {
        "retry once" => {
            row.attempts = 0;
            row.launch_failures = 0;
            row.stuck_ask_id.clear();
        }
        "reassign" => {
            let _ = tasks.update(
                &task.id,
                adi_tasks::TaskPatch {
                    assignee: Some(String::new()),
                    ..adi_tasks::TaskPatch::default()
                },
            );
            row.stuck_ask_id.clear();
        }
        "archive" => {
            let _ = tasks.archive(&task.id, false);
            row.stuck_ask_id.clear();
        }
        _ => tracing::warn!(
            task = %task.id, reply = %reply,
            "stuck ask answered with something other than its own options — left stuck until it \
             is answered with one of them"
        ),
    }
}

/// One ask, ever, for a `Running` task whose log has gone silent — never a relaunch, and never a
/// second ask for the same run once the first is out (answered or not).
fn silence_alarm(store: &crate::store::SessionStore, row: &mut DispatchRow, now: u64) {
    if !row.silence_ask_id.is_empty() || store.pending_question(&row.agent, &row.run_id).is_some() {
        return;
    }
    let log = store.log_path(&row.agent, &row.run_id);
    let Some(last_write_ms) = crate::finished_at(&log) else {
        return;
    };
    if now.saturating_sub(last_write_ms / 1_000) < SILENCE_ALARM_SECS {
        return;
    }
    let req = AskRequest {
        note: "this run is still live, but nothing has reached its log in over an hour — it may \
               be hung. Never relaunched automatically; say what to do."
            .to_string(),
        questions: vec![Question {
            header: "Silent run".into(),
            question: "no output in over an hour, and it is still running. What now?".into(),
            options: vec![
                choice(
                    "let it keep going",
                    "Nothing changes; ask again if it is still silent later.",
                ),
                choice(
                    "stop it",
                    "End the run yourself from its conversation; the task stays ready either way.",
                ),
            ],
            multi_select: false,
        }],
        ..AskRequest::default()
    };
    if let Ok(ask) = store.ask(&row.agent, &row.run_id, &req) {
        row.silence_ask_id = ask.id;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::SetBy as StoreSetBy;
    use crate::{AgentManifest, Backend};
    use adi_config::Config;

    fn scratch(tag: &str) -> (Agents, adi_tasks::Tasks) {
        let root = std::env::temp_dir().join(format!(
            "adi-dispatch-tick-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id(),
        ));
        let _ = std::fs::remove_dir_all(&root);
        let config = Config::with_root(root);
        (
            Agents::with_config(config.clone()),
            adi_tasks::Tasks::with_config(config),
        )
    }

    /// `HarnessClaudeSdk` with no arguments is the one backend whose own `check()` passes with
    /// nothing configured at all (see `backends::detached`'s own test of exactly that) — so a
    /// launch gets as far as really trying to spawn the engine, which is what the "did this
    /// attempt even open a session" tests below need, whether or not that spawn itself succeeds
    /// in a sandbox with no such binary on `PATH`.
    fn worker(agents: &Agents, name: &str, dispatch: bool) {
        agents
            .save(
                name,
                AgentManifest::<serde_json::Value> {
                    backend: Some(Backend::HarnessClaudeSdk),
                    arguments: serde_json::json!({}),
                    dispatch,
                    ..AgentManifest::default()
                },
            )
            .expect("save");
    }

    fn cleanup(agents: &Agents) {
        let _ = std::fs::remove_dir_all(agents.config().root());
    }

    #[test]
    fn a_ready_task_assigned_to_an_opted_in_agent_is_dispatched() {
        let (agents, tasks) = scratch("dispatched");
        worker(&agents, "solver", true);
        let task = tasks
            .create("ship it".into(), None, None, None, None, Some("/tmp".into()))
            .expect("create")
            .task;
        tasks
            .update(
                &task.id,
                adi_tasks::TaskPatch {
                    assignee: Some("solver".into()),
                    ..adi_tasks::TaskPatch::default()
                },
            )
            .expect("assign");

        let dispatched = tick(&agents);

        assert_eq!(dispatched.len(), 1);
        assert_eq!(dispatched[0].task_id, task.id);
        assert_eq!(dispatched[0].agent, "solver");
        let goals = crate::goals::of_conversation(&agents, "solver", &dispatched[0].run_id);
        assert_eq!(goals.len(), 1);
        assert_eq!(goals[0].text, format!("task {} is done", task.id));
        assert_eq!(goals[0].set_by, StoreSetBy::Automation);

        cleanup(&agents);
    }

    #[test]
    fn a_task_assigned_to_a_person_is_never_dispatched() {
        let (agents, tasks) = scratch("person");
        let task = tasks
            .create("file the report".into(), None, None, None, None, None)
            .expect("create")
            .task;
        tasks
            .update(
                &task.id,
                adi_tasks::TaskPatch {
                    assignee: Some("igor".into()),
                    ..adi_tasks::TaskPatch::default()
                },
            )
            .expect("assign");

        assert!(tick(&agents).is_empty());
        cleanup(&agents);
    }

    #[test]
    fn a_task_assigned_to_an_agent_that_has_not_opted_in_waits() {
        let (agents, tasks) = scratch("not-opted-in");
        worker(&agents, "solver", false);
        let task = tasks
            .create("ship it".into(), None, None, None, None, None)
            .expect("create")
            .task;
        tasks
            .update(
                &task.id,
                adi_tasks::TaskPatch {
                    assignee: Some("solver".into()),
                    ..adi_tasks::TaskPatch::default()
                },
            )
            .expect("assign");

        assert!(tick(&agents).is_empty());
        cleanup(&agents);
    }

    /// At the cap, a task waits and nothing about it is spent — no attempt counted, no backoff
    /// started — so the very next free slot can still dispatch it immediately.
    #[test]
    fn a_task_waits_at_the_cap_and_nothing_about_it_is_spent() {
        let (agents, tasks) = scratch("cap-full");
        agents
            .set_limits(crate::RunLimits {
                max_concurrent_runs: 1,
                ..crate::RunLimits::default()
            })
            .expect("limits");
        worker(&agents, "busy", true);
        worker(&agents, "solver", true);
        // Fill the one slot the cap allows with an unrelated running conversation.
        let Launch::Process { .. } = agents.force_run_in("busy", "stay busy", None).expect("force")
        else {
            panic!("expected a headless run");
        };

        let task = tasks
            .create("ship it".into(), None, None, None, None, None)
            .expect("create")
            .task;
        tasks
            .update(
                &task.id,
                adi_tasks::TaskPatch {
                    assignee: Some("solver".into()),
                    ..adi_tasks::TaskPatch::default()
                },
            )
            .expect("assign");

        assert!(tick(&agents).is_empty(), "the cap is full");
        // Refused before anything was ever written down: no row exists at all, which is itself
        // "nothing about it is spent" — there is nothing to roll back if there was never a row.
        assert_eq!(agents.sessions().dispatch_row(&task.id).expect("read"), None);

        cleanup(&agents);
    }

    /// A run that simply vanished — no outcome, no `adi.agents.run.finished`, exactly what an app
    /// restart mid-run leaves behind — is not counted against the budget, and the task is tried
    /// again once the floor passes.
    #[test]
    fn a_run_that_vanished_without_an_outcome_is_not_counted_and_is_retried() {
        let (agents, tasks) = scratch("restart-mid-run");
        worker(&agents, "solver", true);
        let task = tasks
            .create("ship it".into(), None, None, None, None, None)
            .expect("create")
            .task;
        tasks
            .update(
                &task.id,
                adi_tasks::TaskPatch {
                    assignee: Some("solver".into()),
                    ..adi_tasks::TaskPatch::default()
                },
            )
            .expect("assign");

        // Stopped directly, not ended with `simulate_turn`: that writes its own terminal metrics
        // line (`terminal_reason: "simulated"`) even with no blocks, which is a *reported* ending
        // and not the scenario this test is for. `stop_run` empties the seat with nothing written
        // to the log at all — the same shape `note_finished` finds for a real run a platform
        // restart killed before it could report anything of its own.
        let store = agents.sessions();
        let Launch::Process { run_id, .. } = agents.simulate("solver", "work").expect("simulate")
        else {
            panic!("a simulated run is not a pane");
        };
        agents.stop_run("solver", &run_id).expect("stop");
        assert!(
            store.get("solver", &run_id).expect("session").outcome.is_none(),
            "nothing has recorded an ending yet — the scenario this test is for"
        );
        store
            .save_dispatch_row(&DispatchRow {
                task_id: task.id.clone(),
                agent: "solver".into(),
                run_id: run_id.clone(),
                last_attempt_at: 0,
                ..DispatchRow::default()
            })
            .expect("save row");

        let dispatched = tick(&agents);
        assert_eq!(
            dispatched.len(),
            1,
            "retried after noticing the vanished run, not stuck"
        );
        assert_ne!(dispatched[0].run_id, run_id, "a fresh attempt, not the vanished one");
        let row = store.dispatch_row(&task.id).expect("read").expect("row");
        assert_eq!(row.attempts, 0, "the vanished run was never counted");

        cleanup(&agents);
    }

    /// Two `launch_failed` endings reach the tighter budget before the general one, raise exactly
    /// one ask, and answering it with each of the three options does what it says.
    #[test]
    fn two_launch_failures_raise_a_stuck_ask_and_each_answer_acts_on_it() {
        let (agents, tasks) = scratch("stuck-launch-failed");
        worker(&agents, "solver", true);
        let task = tasks
            .create("ship it".into(), None, None, None, None, None)
            .expect("create")
            .task;
        tasks
            .update(
                &task.id,
                adi_tasks::TaskPatch {
                    assignee: Some("solver".into()),
                    ..adi_tasks::TaskPatch::default()
                },
            )
            .expect("assign");

        let store = agents.sessions();
        for _ in 0..2 {
            let launched = tick(&agents);
            assert_eq!(launched.len(), 1);
            let run_id = launched[0].run_id.clone();
            // Really ends the real engine this dispatched (`HarnessClaudeSdk` is runnable here),
            // so the next tick reads it as finished rather than racing its own exit.
            let _ = agents.stop_run("solver", &run_id);
            let outcome = crate::store::RunOutcome {
                terminal_reason: Some("launch_failed".into()),
                is_error: true,
                ..crate::store::RunOutcome::default()
            };
            store
                .record_outcome("solver", &run_id, &outcome)
                .expect("record");
            // Past the floor/backoff either way, so the very next tick evaluates this attempt.
            let mut row = store.dispatch_row(&task.id).expect("read").expect("row");
            row.last_attempt_at = 0;
            store.save_dispatch_row(&row).expect("backdate");
        }

        assert!(tick(&agents).is_empty(), "stuck, not relaunched");
        let row = store.dispatch_row(&task.id).expect("read").expect("row");
        assert_eq!(row.launch_failures, 2);
        assert!(!row.stuck_ask_id.is_empty(), "the stuck ask was raised");

        let ask = store
            .question_history("solver", &row.run_id)
            .into_iter()
            .find(|a| a.id == row.stuck_ask_id)
            .expect("the ask exists");
        store
            .resolve_question(
                "solver",
                &row.run_id,
                Some(&ask.id),
                &Answer {
                    at: crate::store::now_ms(),
                    by: crate::store::AnsweredBy::Human,
                    replies: vec!["retry once".into()],
                },
            )
            .expect("answer");

        assert!(
            tick(&agents).is_empty(),
            "retry once only clears the budget — it still waits out the 30s floor"
        );
        let row = store.dispatch_row(&task.id).expect("read").expect("row");
        assert_eq!(row.attempts, 0, "retry once cleared the budget");
        assert_eq!(row.launch_failures, 0);
        assert!(row.stuck_ask_id.is_empty());

        cleanup(&agents);
    }

    /// A task that gives up before any run was ever attempted has nowhere to ask through — no
    /// session exists yet — so it is left stuck and logged rather than asked.
    #[test]
    fn giving_up_with_no_run_at_all_is_stuck_with_nothing_to_ask_through() {
        let (agents, tasks) = scratch("give-up-no-run");
        let task = tasks
            .create("ship it".into(), None, None, None, None, None)
            .expect("create")
            .task;
        tasks.give_up(&task.id, "no route to the target host").expect("give up");

        assert!(tick(&agents).is_empty(), "nothing is assigned yet; nothing to dispatch at all");
        cleanup(&agents);
    }

    /// Giving up after a real attempt is stuck immediately — no need to exhaust the attempt
    /// budget — and answering "reassign" clears the assignee rather than touching the task's
    /// status, so it simply stops being a candidate until somebody else is named.
    #[test]
    fn giving_up_is_stuck_at_once_and_reassign_clears_the_assignee() {
        let (agents, tasks) = scratch("give-up-reassign");
        worker(&agents, "solver", true);
        let task = tasks
            .create("ship it".into(), None, None, None, None, None)
            .expect("create")
            .task;
        tasks
            .update(
                &task.id,
                adi_tasks::TaskPatch {
                    assignee: Some("solver".into()),
                    ..adi_tasks::TaskPatch::default()
                },
            )
            .expect("assign");
        let launched = tick(&agents);
        assert_eq!(launched.len(), 1, "one attempt happens before it ever gives up");

        tasks
            .give_up(&task.id, "no route to the target host")
            .expect("give up");
        assert!(tick(&agents).is_empty(), "stuck at once — attempts never reached the budget");

        let store = agents.sessions();
        let row = store.dispatch_row(&task.id).expect("read").expect("row");
        assert_eq!(row.attempts, 0, "give-up is its own path, not a counted failure");
        let ask = store
            .question_history(&row.agent, &row.run_id)
            .into_iter()
            .find(|a| a.id == row.stuck_ask_id)
            .expect("the ask exists");
        store
            .resolve_question(
                &row.agent,
                &row.run_id,
                Some(&ask.id),
                &Answer {
                    at: crate::store::now_ms(),
                    by: crate::store::AnsweredBy::Human,
                    replies: vec!["reassign".into()],
                },
            )
            .expect("answer");

        assert!(tick(&agents).is_empty(), "no assignee left to dispatch to");
        assert_eq!(
            tasks.get(&task.id).expect("read").task.assignee,
            None,
            "reassign clears the assignee rather than archiving or reopening"
        );

        cleanup(&agents);
    }

    /// Answering a stuck ask with "archive" drops the task out of the ready list for good.
    #[test]
    fn archive_on_a_stuck_ask_takes_the_task_out_of_the_ready_list() {
        let (agents, tasks) = scratch("stuck-archive");
        worker(&agents, "solver", true);
        let task = tasks
            .create("ship it".into(), None, None, None, None, None)
            .expect("create")
            .task;
        tasks
            .update(
                &task.id,
                adi_tasks::TaskPatch {
                    assignee: Some("solver".into()),
                    ..adi_tasks::TaskPatch::default()
                },
            )
            .expect("assign");
        let _ = tick(&agents);
        tasks.give_up(&task.id, "no route").expect("give up");
        let _ = tick(&agents);

        let store = agents.sessions();
        let row = store.dispatch_row(&task.id).expect("read").expect("row");
        let ask = store
            .question_history(&row.agent, &row.run_id)
            .into_iter()
            .find(|a| a.id == row.stuck_ask_id)
            .expect("the ask exists");
        store
            .resolve_question(
                &row.agent,
                &row.run_id,
                Some(&ask.id),
                &Answer {
                    at: crate::store::now_ms(),
                    by: crate::store::AnsweredBy::Human,
                    replies: vec!["archive".into()],
                },
            )
            .expect("answer");

        assert!(tick(&agents).is_empty());
        assert_eq!(
            tasks.get(&task.id).expect("read").task.status,
            adi_tasks::TaskStatus::Archived
        );

        cleanup(&agents);
    }

    /// A task that reaches the general attempt budget by ordinary (non-`launch_failed`) endings is
    /// stuck too, on the wider budget.
    #[test]
    fn three_ordinary_failures_reach_the_general_budget_and_stick() {
        let (agents, tasks) = scratch("stuck-general");
        worker(&agents, "solver", true);
        let task = tasks
            .create("ship it".into(), None, None, None, None, None)
            .expect("create")
            .task;
        tasks
            .update(
                &task.id,
                adi_tasks::TaskPatch {
                    assignee: Some("solver".into()),
                    ..adi_tasks::TaskPatch::default()
                },
            )
            .expect("assign");

        let store = agents.sessions();
        for _ in 0..3 {
            let launched = tick(&agents);
            assert_eq!(launched.len(), 1);
            let run_id = launched[0].run_id.clone();
            let _ = agents.stop_run("solver", &run_id);
            let outcome = crate::store::RunOutcome {
                terminal_reason: Some("completed".into()),
                ..crate::store::RunOutcome::default()
            };
            store
                .record_outcome("solver", &run_id, &outcome)
                .expect("record");
            let mut row = store.dispatch_row(&task.id).expect("read").expect("row");
            row.last_attempt_at = 0;
            store.save_dispatch_row(&row).expect("backdate");
        }

        assert!(tick(&agents).is_empty());
        let row = store.dispatch_row(&task.id).expect("read").expect("row");
        assert_eq!(row.attempts, 3);
        assert!(!row.stuck_ask_id.is_empty());

        cleanup(&agents);
    }

    /// A goal nudged past the cap with the task still open is stuck — even though the conversation
    /// still reads as "working" and so is never redispatched out from under it.
    #[test]
    fn a_goal_nudged_past_the_cap_is_stuck_without_being_redispatched() {
        let (agents, tasks) = scratch("nudge-cap");
        worker(&agents, "solver", true);
        let task = tasks
            .create("ship it".into(), None, None, None, None, None)
            .expect("create")
            .task;
        tasks
            .update(
                &task.id,
                adi_tasks::TaskPatch {
                    assignee: Some("solver".into()),
                    ..adi_tasks::TaskPatch::default()
                },
            )
            .expect("assign");

        let store = agents.sessions();
        let record = store
            .create("solver", Backend::HarnessAdi, "/tmp", "work")
            .expect("create session");
        crate::goals::create(
            &agents,
            "solver",
            &record.id,
            &format!("task {} is done", task.id),
            StoreSetBy::Automation,
        )
        .expect("goal");
        for n in 0..NUDGE_CAP {
            store
                .mark_goals_nudged("solver", &record.id, n + 1)
                .expect("nudge");
        }
        store
            .save_dispatch_row(&DispatchRow {
                task_id: task.id.clone(),
                agent: "solver".into(),
                run_id: record.id.clone(),
                ..DispatchRow::default()
            })
            .expect("save row");

        assert!(tick(&agents).is_empty(), "stuck, and never relaunched over a live goal");
        let row = store.dispatch_row(&task.id).expect("read").expect("row");
        assert!(!row.stuck_ask_id.is_empty());

        cleanup(&agents);
    }

    /// A `Running` task whose log has not moved in over an hour gets exactly one ask, never a
    /// second while it stays silent, and is never relaunched on its own account.
    #[test]
    fn a_silent_running_task_gets_exactly_one_ask_and_no_relaunch() {
        let (agents, tasks) = scratch("silence-alarm");
        worker(&agents, "solver", true);
        let task = tasks
            .create("ship it".into(), None, None, None, None, None)
            .expect("create")
            .task;
        tasks
            .update(
                &task.id,
                adi_tasks::TaskPatch {
                    assignee: Some("solver".into()),
                    ..adi_tasks::TaskPatch::default()
                },
            )
            .expect("assign");

        let Launch::Process { run_id, .. } = agents.simulate("solver", "work").expect("simulate")
        else {
            panic!("a simulated run is not a pane");
        };
        let store = agents.sessions();
        let old = std::time::SystemTime::now()
            .checked_sub(std::time::Duration::from_secs(SILENCE_ALARM_SECS + 60))
            .expect("a time an hour and a minute ago");
        std::fs::OpenOptions::new()
            .write(true)
            .open(store.log_path("solver", &run_id))
            .expect("the simulated send already created the log")
            .set_modified(old)
            .expect("backdate the log");
        store
            .save_dispatch_row(&DispatchRow {
                task_id: task.id.clone(),
                agent: "solver".into(),
                run_id: run_id.clone(),
                last_attempt_at: now_unix(),
                ..DispatchRow::default()
            })
            .expect("save row");

        assert!(tick(&agents).is_empty(), "never relaunched while still running");
        let row = store.dispatch_row(&task.id).expect("read").expect("row");
        assert!(!row.silence_ask_id.is_empty());
        assert!(store.pending_question("solver", &run_id).is_some());

        assert!(tick(&agents).is_empty(), "still silent, still never relaunched");
        assert_eq!(
            store.question_history("solver", &run_id).len(),
            1,
            "exactly one ask however many ticks pass while it stays silent"
        );

        cleanup(&agents);
    }

    #[test]
    fn backoff_widens_after_each_counted_failure() {
        assert_eq!(backoff_secs(0), DISPATCH_FLOOR_SECS);
        assert_eq!(backoff_secs(1), BACKOFF_FIRST_SECS);
        assert_eq!(backoff_secs(2), BACKOFF_SECOND_SECS);
        assert_eq!(backoff_secs(10), BACKOFF_SECOND_SECS);
    }
}
