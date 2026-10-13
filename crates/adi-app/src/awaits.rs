//! Event and deadline worker for awaits, questions, and goals.
//!
//! Await checks run commands, so the dispatcher queues events to one worker to avoid blocking
//! trigger dispatch or spawning unbounded threads during bursts.

use std::sync::Arc;
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::time::{Duration, Instant};

use adi_agents::{Agents, awaits, dispatch, goals, questions};
use adi_events::EventRecord;
use adi_triggers::EventObserver;
use adi_workflows::{Engine as WorkflowEngine, StartedRun};
use tracing::{info, warn};

const TICK: Duration = Duration::from_secs(1);

/// Owned event name and payload.
type Posted = (String, String);

/// Start the worker; dropping every clone of the returned observer stops it.
pub fn start(agents: Agents, workflows: WorkflowEngine) -> EventObserver {
    let (tx, rx) = channel::<Posted>();
    std::thread::spawn(move || run(&agents, &workflows, &rx));
    observer(tx)
}

fn observer(tx: Sender<Posted>) -> EventObserver {
    Arc::new(move |record: &EventRecord| {
        let _ = tx.send((record.name.clone(), record.payload.clone()));
    })
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
            // root task; nothing here yet drives that run's own `ctx` checkpoints (see
            // `adi_workflows`' crate docs for what's still WF-5 work).
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
}
