//! Event and deadline worker for awaits, questions, and goals.
//!
//! Await checks run commands, so the dispatcher queues events to one worker to avoid blocking
//! trigger dispatch or spawning unbounded threads during bursts.
//!
//! Workflow *execution* (spawning `bun` attempts for runs already poured) runs on its own thread,
//! [`run_workflow_attempts`], separate from [`run`]'s shared one-second loop — see that function's
//! own doc for why (ADI-MONO-150). Workflow *starting* (`schedule_tick`/`event_tick`, pouring a
//! newly-due run's row and root task) stays on the shared loop: cheap, and bounded by how often a
//! schedule or event actually fires, never by how many runs are already waiting.

use std::sync::Arc;
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::time::{Duration, Instant};

use adi_agents::{Agents, awaits, dispatch, goals, questions};
use adi_events::EventRecord;
use adi_triggers::EventObserver;
use adi_workflows::{AttemptOutcome, Engine as WorkflowEngine, StartedRun};
use tracing::{info, warn};

const TICK: Duration = Duration::from_secs(1);

/// How many workflow runs [`run_workflow_attempts`] will attempt at once — spec ADI-MONO-150's
/// own "e.g. 2 bun processes at a time". A run waiting on a 48-hour review and a run waiting on a
/// 5-minute sleep must never queue behind each other indefinitely; two in flight is enough
/// headroom without this process running an unbounded number of `bun` children at once.
const WORKFLOW_CONCURRENCY: usize = 2;

/// Owned event name and payload.
type Posted = (String, String);

/// Start the worker; dropping every clone of the returned observer stops it (both threads this
/// spawns — the shared loop and the dedicated workflow-attempt loop).
pub fn start(agents: Agents, workflows: WorkflowEngine) -> EventObserver {
    let (tx, rx) = channel::<Posted>();
    let (keepalive_tx, keepalive_rx) = channel::<()>();
    let workflow_attempts = workflows.clone();
    std::thread::spawn(move || run(&agents, &workflows, &rx));
    std::thread::spawn(move || run_workflow_attempts(&workflow_attempts, &keepalive_rx));
    observer(tx, keepalive_tx)
}

fn observer(tx: Sender<Posted>, keepalive: Sender<()>) -> EventObserver {
    Arc::new(move |record: &EventRecord| {
        let _ = tx.send((record.name.clone(), record.payload.clone()));
        // Held only so its matching receiver disconnects — and `run_workflow_attempts` stops —
        // the moment every clone of this very closure (the `EventObserver` this crate hands out)
        // is dropped, exactly like `tx` already does for `run`'s own loop.
        let _ = &keepalive;
    })
}

/// Drives workflow execution on its own thread and cadence, independent of [`run`]'s shared
/// one-second loop: a run waiting on a slow person task or a many-hour sleep must never delay
/// `dispatch`/`goals`/`awaits`, which share that loop (ADI-MONO-150). `run_tick_bounded`'s own
/// readiness cache already means a sweep that finds nothing due costs a handful of indexed SQLite
/// reads, never a `bun` spawn; the concurrency cap here is about the sweeps that *do* find
/// something, so one slow attempt can never queue up every other due run behind it.
fn run_workflow_attempts(workflows: &WorkflowEngine, keepalive: &Receiver<()>) {
    loop {
        report_workflows_ran(&workflows.run_tick_bounded(WORKFLOW_CONCURRENCY));
        match keepalive.recv_timeout(TICK) {
            Ok(()) | Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
        }
    }
}

/// Track deadlines explicitly so a steady event stream cannot starve sweeps.
fn run(agents: &Agents, workflows: &WorkflowEngine, rx: &Receiver<Posted>) {
    let mut next_tick = Instant::now() + TICK;
    loop {
        let wait = next_tick.saturating_duration_since(Instant::now());
        match rx.recv_timeout(wait) {
            Ok((name, payload)) => report(&awaits::on_event(agents, &name, &payload)),
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
        }
        if Instant::now() >= next_tick {
            report(&awaits::tick(agents));
            report_settled(&questions::tick(agents));
            // Awaits and question defaults may wake a conversation before its goal check.
            report_nudged(&goals::tick(agents));
            // Last: a task just finished by a goal check above is exactly the kind of "not
            // working any more" the dispatch tick looks for, so it should see today's freshest
            // state rather than next second's.
            report_dispatched(&dispatch::tick(agents));
            // The workflow engine's two starters — schedule and event — pour a new run's row and
            // root task; `run_workflow_attempts`' own dedicated thread is what actually drives one
            // (spawning a fresh `bun` attempt for every due run), off this shared loop entirely so
            // a waiting run can never delay the ticks above (ADI-MONO-150).
            report_workflows_started(&workflows.schedule_tick(chrono::Utc::now()));
            report_workflows_started(&workflows.event_tick());
            next_tick = Instant::now() + TICK;
        }
    }
}

/// A failed wake still spends the await, so delivery failures need a warning.
fn report(woken: &[awaits::Woken]) {
    for w in woken {
        match w.error.as_deref() {
            None => info!(
                await_id = %w.id, agent = %w.agent, conversation = %w.conv, cause = %w.cause,
                "await woke a conversation"
            ),
            Some(error) => warn!(
                await_id = %w.id, agent = %w.agent, conversation = %w.conv, cause = %w.cause, %error,
                "await fired but its wake could not be delivered"
            ),
        }
    }
}

fn report_settled(settled: &[questions::Settled]) {
    for s in settled {
        match s.error.as_deref() {
            None => info!(
                ask = %s.id, agent = %s.agent, conversation = %s.conv, question = %s.question,
                queued = s.queued,
                "nobody answered in time — took the run's own default"
            ),
            Some(error) => warn!(
                ask = %s.id, agent = %s.agent, conversation = %s.conv, question = %s.question, %error,
                "a question's deadline passed but its default could not be delivered"
            ),
        }
    }
}

fn report_dispatched(dispatched: &[dispatch::Dispatched]) {
    for d in dispatched {
        info!(
            task = %d.task_id, agent = %d.agent, run_id = %d.run_id,
            "dispatch tick launched a ready task"
        );
    }
}

fn report_workflows_started(started: &[StartedRun]) {
    for s in started {
        info!(run_key = %s.run_key, workflow = %s.workflow, "a workflow starter poured a new run");
    }
}

fn report_workflows_ran(ran: &[adi_workflows::RanAttempt]) {
    for r in ran {
        match &r.outcome {
            AttemptOutcome::Completed(_) => {
                info!(run_key = %r.run_key, workflow = %r.workflow, "a workflow run completed");
            }
            AttemptOutcome::Failed(reason) | AttemptOutcome::GaveUp(reason) => warn!(
                run_key = %r.run_key, workflow = %r.workflow, %reason,
                "a workflow run failed"
            ),
            // A transient `bun` crash — `Attempt::finalize` has already decided whether this
            // retries with backoff or exhausted its budget and failed the run for real; either
            // way there's nothing more for this tick's own caller to do but note it happened.
            AttemptOutcome::Crashed(reason) => info!(
                run_key = %r.run_key, workflow = %r.workflow, %reason,
                "a workflow attempt crashed with no terminal message"
            ),
            AttemptOutcome::Suspended => {}
        }
    }
}

fn report_nudged(nudged: &[goals::Nudged]) {
    for n in nudged {
        match n.error.as_deref() {
            None => info!(
                agent = %n.agent, conversation = %n.conv, goals = n.goals.len(),
                "asked a quiet conversation whether its goal is met"
            ),
            Some(error) => warn!(
                agent = %n.agent, conversation = %n.conv, goals = n.goals.len(), %error,
                "a goal check could not be delivered"
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use adi_agents::AgentRunFinished;
    use adi_agents::awaits::{Awaits, Request};
    use adi_events::Event;
    use adi_tasks::TaskDeleted;

    fn scratch(tag: &str) -> adi_config::Config {
        let root = std::env::temp_dir().join(format!(
            "adi-app-awaits-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id(),
        ));
        let _ = std::fs::remove_dir_all(&root);
        adi_config::Config::with_root(root)
    }

    // These conversation ids do not exist; failed delivery still claims each await.
    #[test]
    fn a_posted_event_and_a_passing_deadline_both_reach_the_store() {
        let config = scratch("worker");
        let agents = Agents::with_config(config.clone());
        let store = Awaits::with_config(config.clone());

        let on_event = awaits::register(
            &store,
            "watcher",
            "conv-1",
            &Request {
                note: "the event one".into(),
                events: vec!["adi.tasks.*".into()],
                ..Request::default()
            },
        )
        .expect("register");
        let on_timer = awaits::register(
            &store,
            "watcher",
            "conv-2",
            &Request {
                note: "the timer one".into(),
                after_seconds: Some(1),
                ..Request::default()
            },
        )
        .expect("register");

        let observer = start(agents, WorkflowEngine::with_config(config));
        observer(&TaskDeleted::new("t1").to_record().expect("task event"));

        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline && !store.list().is_empty() {
            std::thread::sleep(Duration::from_millis(50));
        }
        assert!(
            store.list().is_empty(),
            "both awaits should have fired; still pending: {:?}",
            store.list()
        );
        assert!(!store.claim(&on_event.id));
        assert!(!store.claim(&on_timer.id));

        let _ = std::fs::remove_dir_all(store.dir());
    }

    #[test]
    fn a_wake_scoped_to_one_run_survives_another_runs_ending() {
        let config = scratch("scoped");
        let agents = Agents::with_config(config.clone());
        let store = Awaits::with_config(config.clone());

        let scoped = awaits::register(
            &store,
            "supervisor",
            "conv-1",
            &Request {
                note: "the agent you started ended".into(),
                events: vec![AgentRunFinished::NAME.into()],
                when: [("run_id".to_string(), "r-42".to_string())]
                    .into_iter()
                    .collect(),
                ..Request::default()
            },
        )
        .expect("register");

        let finished = |run_id: &str| {
            AgentRunFinished::new(run_id, "worker", false)
                .to_record()
                .expect("run-finished record")
        };
        let observer = start(agents, WorkflowEngine::with_config(config));

        observer(&finished("r-43"));
        std::thread::sleep(Duration::from_millis(500));
        assert_eq!(
            store.list().len(),
            1,
            "a stranger's ending is not this run's"
        );

        observer(&finished("r-42"));
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline && !store.list().is_empty() {
            std::thread::sleep(Duration::from_millis(50));
        }
        assert!(
            store.list().is_empty(),
            "its own run's ending must wake it; still pending: {:?}",
            store.list()
        );
        assert!(!store.claim(&scoped.id));

        let _ = std::fs::remove_dir_all(store.dir());
    }

    /// ADI-MONO-150: the shared loop's own cadence (a plain timer await firing close to its own
    /// deadline) must hold however many workflow runs are sitting there waiting on a never-
    /// completed person task — before this fix, `run_tick` attempted every one of them, serially,
    /// inside this very loop.
    #[test]
    fn n_waiting_workflow_runs_never_delay_the_shared_ticks() {
        let config = scratch("workflow-pressure");
        let agents = Agents::with_config(config.clone());
        let store = Awaits::with_config(config.clone());

        let workflows = WorkflowEngine::with_config(config.clone());
        workflows.bootstrap().expect("bootstrap");
        let dir = config.module("workflows").dir().to_path_buf();
        std::fs::create_dir_all(&dir).unwrap();
        for i in 0..5 {
            std::fs::write(
                dir.join(format!("n{i}.ts")),
                "import { manual, workflow } from \"@adi/workflows\";\n\
                 export const on = manual();\n\
                 export default workflow(async (ctx, input) => {\n\
                 \x20 await ctx.task({ key: \"gate\", title: \"review\", person: \"igor\" });\n\
                 \x20 return {};\n\
                 });\n",
            )
            .unwrap();
            workflows
                .start_manual(&format!("n{i}"), &serde_json::json!({}), &format!("req-{i}"))
                .expect("start");
        }

        let on_timer = awaits::register(
            &store,
            "watcher",
            "conv-timer",
            &Request {
                note: "the timer one".into(),
                after_seconds: Some(1),
                ..Request::default()
            },
        )
        .expect("register");

        let started_at = Instant::now();
        let _observer = start(agents, workflows);
        let deadline = started_at + Duration::from_secs(5);
        while Instant::now() < deadline && !store.list().is_empty() {
            std::thread::sleep(Duration::from_millis(50));
        }
        let elapsed = started_at.elapsed();
        assert!(
            store.list().is_empty(),
            "the timer await must still fire with 5 workflow runs waiting; still pending: {:?}",
            store.list()
        );
        assert!(
            elapsed < Duration::from_secs(2),
            "an await scheduled for 1s took {elapsed:?} to fire with 5 workflow runs waiting \
             on an unfinished person task — the shared loop must never wait on a workflow attempt"
        );
        assert!(!store.claim(&on_timer.id));

        let _ = std::fs::remove_dir_all(store.dir());
    }
}
