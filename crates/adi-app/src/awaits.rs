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
use adi_events::{EventRecord, Events, supervise};
use adi_triggers::EventObserver;
use adi_workflows::{AttemptOutcome, Engine as WorkflowEngine, StartedRun};
use tracing::{info, warn};

const TICK: Duration = Duration::from_secs(1);

/// How often [`watch`] looks at the scheduler's heartbeat.
const WATCH_EVERY: Duration = Duration::from_secs(30);

/// How many workflow runs [`run_workflow_attempts`] will attempt at once — spec ADI-MONO-150's
/// own "e.g. 2 bun processes at a time". A run waiting on a 48-hour review and a run waiting on a
/// 5-minute sleep must never queue behind each other indefinitely; two in flight is enough
/// headroom without this process running an unbounded number of `bun` children at once.
const WORKFLOW_CONCURRENCY: usize = 2;

/// Owned event name and payload.
type Posted = (String, String);

/// Start the worker; dropping every clone of the returned observer stops it (both threads this
/// spawns — the shared loop and the dedicated workflow-attempt loop).
///
/// Both threads are [supervised](supervise::thread): a panic that gets past [`isolated`] starts
/// the loop again a second later and is recorded. Before that, the only thing that fires awaits
/// on a machine could end with the process still up and every status green — on 2026-11-24 it
/// did, for eleven hours, and each `isolated` call's own `tracing::error!` was the second panic
/// that took the thread. Named, so a thread list shows whether they exist.
pub fn start(agents: Agents, workflows: WorkflowEngine) -> EventObserver {
    let (tx, rx) = channel::<Posted>();
    let (keepalive_tx, keepalive_rx) = channel::<()>();
    let (watch_tx, watch_rx) = channel::<()>();
    let workflow_attempts = workflows.clone();
    let store = Events::with_config(agents.config().clone());
    let pending = awaits::Awaits::with_config(agents.config().clone());
    supervise::thread(store.clone(), "adi-awaits", move || {
        run(&agents, &workflows, &rx);
    });
    supervise::thread(store.clone(), "adi-workflows", move || {
        run_workflow_attempts(&workflow_attempts, &keepalive_rx);
    });
    let events = store.clone();
    supervise::thread(store, "adi-liveness", move || {
        watch(&pending, &events, &watch_rx);
    });
    observer(tx, (keepalive_tx, watch_tx))
}

fn observer(tx: Sender<Posted>, keepalive: (Sender<()>, Sender<()>)) -> EventObserver {
    Arc::new(move |record: &EventRecord| {
        let _ = tx.send((record.name.clone(), record.payload.clone()));
        // Held only so their matching receivers disconnect — and `run_workflow_attempts` and
        // `watch` stop — the moment every clone of this very closure (the `EventObserver` this
        // crate hands out) is dropped, exactly like `tx` already does for `run`'s own loop.
        let _ = &keepalive;
    })
}

/// Say so when the scheduler stops ticking without having panicked.
///
/// Supervision restarts a loop that panics; it can do nothing for one that is blocked — in a
/// lock, in a delivery that never returns. That loop leaves no incident, only a
/// [heartbeat](awaits::Awaits::beat) that stops moving. This thread reads it and records the
/// stall once, as an error in the log and the diagnostics store, and once more when it ends. It
/// restarts nothing: the remedy for a blocked thread is a restart of the process, and that is an
/// operator's decision, not a watchdog's.
fn watch(pending: &awaits::Awaits, events: &Events, keepalive: &Receiver<()>) {
    let mut told = false;
    loop {
        match keepalive.recv_timeout(WATCH_EVERY) {
            Ok(()) | Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
        }
        supervise::flush(events);
        let now = adi_config::now_unix();
        let scheduler = pending.scheduler();
        match stall_change(scheduler.stalled(now), &mut told) {
            Some(true) => tracing::error!("{}", scheduler.describe(now)),
            Some(false) => info!("the await scheduler is ticking again"),
            None => {}
        }
    }
}

/// Whether a stall just began (`Some(true)`) or just ended (`Some(false)`), given whether the
/// last one was already reported — so a stall is one line, however long it lasts.
fn stall_change(stalled: bool, told: &mut bool) -> Option<bool> {
    if stalled == *told {
        return None;
    }
    *told = stalled;
    Some(stalled)
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
    let store = Events::with_config(agents.config().clone());
    let isolated = |label: &'static str| Isolated {
        store: &store,
        label,
    };
    let mut next_tick = Instant::now() + TICK;
    loop {
        let wait = next_tick.saturating_duration_since(Instant::now());
        match rx.recv_timeout(wait) {
            Ok((name, payload)) => {
                report(&isolated("await-events").run(|| awaits::on_event(agents, &name, &payload)))
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
        }
        if Instant::now() >= next_tick {
            // Whatever an earlier pass could not record — the store it records into was the
            // thing failing — is recorded by the first pass that can.
            supervise::flush(&store);
            report(&isolated("awaits-tick").run(|| awaits::tick(agents)));
            report_settled(&isolated("questions-tick").run(|| questions::tick(agents)));
            // Awaits and question defaults may wake a conversation before its goal check.
            report_nudged(&isolated("goals-tick").run(|| goals::tick(agents)));
            // Last: a task just finished by a goal check above is exactly the kind of "not
            // working any more" the dispatch tick looks for, so it should see today's freshest
            // state rather than next second's. `isolated` is what actually keeps this promise
            // under a bug, not just the ordering: without it, a panic inside the goals tick above
            // would unwind straight through this whole loop and silently end it, which is the one
            // way "ran right before" could turn into "never ran again" (ADI-TIKTOK-168 asked this
            // of exactly this pair — they share no lock and nothing here blocks on the other, so
            // today this is a safety net against a future bug rather than a fix for a present one).
            report_dispatched(&isolated("dispatch-tick").run(|| dispatch::tick(agents)));
            // The workflow engine's two starters — schedule and event — pour a new run's row and
            // root task; `run_workflow_attempts`' own dedicated thread is what actually drives one
            // (spawning a fresh `bun` attempt for every due run), off this shared loop entirely so
            // a waiting run can never delay the ticks above (ADI-MONO-150).
            report_workflows_started(
                &isolated("workflows-schedule-tick")
                    .run(|| workflows.schedule_tick(jiff::Timestamp::now())),
            );
            report_workflows_started(
                &isolated("workflows-event-tick").run(|| workflows.event_tick()),
            );
            next_tick = Instant::now() + TICK;
        }
    }
}

/// One sweep of this loop, run in isolation from its neighbours: a panic inside it (a bug this
/// sweep's own tests missed) is caught, recorded, and answered with "nothing happened" rather
/// than unwinding through [`run`]'s loop and ending every sweep after it.
///
/// Recorded through [`supervise::incident`] and not `tracing::error!`, which is what this used to
/// call: on a full disk that log line was itself the next panic, outside any `catch_unwind`, and
/// it ended the thread this exists to protect.
struct Isolated<'a> {
    store: &'a Events,
    label: &'static str,
}

impl Isolated<'_> {
    fn run<T>(&self, f: impl FnOnce() -> Vec<T>) -> Vec<T> {
        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)) {
            Ok(result) => result,
            Err(payload) => {
                supervise::incident(
                    self.store,
                    self.label,
                    &supervise::panic_message(payload.as_ref()),
                );
                Vec::new()
            }
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

    /// ADI-TIKTOK-168 asked whether one tick's own failure could stop every other tick in `run`'s
    /// loop from running again. It cannot block them today (they share no lock), but a panic — a
    /// bug none of these ticks' own tests caught — would otherwise unwind straight through the
    /// shared loop and end it for good. `isolated` is the guard against that: catching it here and
    /// a later call on the same thread still running proves the panic never left this frame.
    #[test]
    fn a_panicking_tick_does_not_stop_the_ones_after_it() {
        let store = Events::with_config(scratch("isolated"));
        let isolated = |label: &'static str| Isolated {
            store: &store,
            label,
        };
        let first = isolated("boom-tick").run::<u8>(|| panic!("simulated failure"));
        assert!(first.is_empty(), "a panicked tick reports nothing happened");

        let second = isolated("ok-tick").run(|| vec![1, 2, 3]);
        assert_eq!(
            second,
            vec![1, 2, 3],
            "the next tick on this thread still runs"
        );

        let recorded = store
            .diagnostics(&adi_events::diagnostics::Query::default())
            .expect("diagnostics");
        assert!(
            recorded
                .iter()
                .any(|r| r.diagnostic.message.contains("boom-tick panicked")
                    && r.diagnostic.message.contains("simulated failure")),
            "the panic is on record: {recorded:?}"
        );
    }

    #[test]
    fn a_stall_is_reported_once_when_it_begins_and_once_when_it_ends() {
        let mut told = false;
        assert_eq!(
            stall_change(false, &mut told),
            None,
            "ticking, nothing to say"
        );
        assert_eq!(stall_change(true, &mut told), Some(true), "it stopped");
        assert_eq!(
            stall_change(true, &mut told),
            None,
            "still stopped: said already"
        );
        assert_eq!(stall_change(false, &mut told), Some(false), "it came back");
        assert_eq!(stall_change(false, &mut told), None);
    }

    /// The freeze itself: a panic that escapes every `isolated` call — on 2026-11-24 it was the
    /// log line inside one — must not be the end of the worker. Before supervision this thread
    /// was simply gone, and the await below would have stayed pending for ever.
    #[test]
    fn the_worker_survives_a_panic_that_escapes_a_tick() {
        let config = scratch("survives");
        let store = Awaits::with_config(config.clone());
        let events = Events::with_config(config.clone());
        let on_timer = awaits::register(
            &store,
            "watcher",
            "conv-1",
            &Request {
                note: "the timer one".into(),
                after_seconds: Some(1),
                ..Request::default()
            },
        )
        .expect("register");

        let agents = Agents::with_config(config.clone());
        let workflows = WorkflowEngine::with_config(config);
        let (tx, rx) = channel::<Posted>();
        let mut first = true;
        let worker = supervise::thread(events.clone(), "adi-awaits-test", move || {
            assert!(!std::mem::take(&mut first), "failed printing to stderr");
            run(&agents, &workflows, &rx);
        });

        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline && !store.list().is_empty() {
            std::thread::sleep(Duration::from_millis(50));
        }
        assert!(
            store.list().is_empty(),
            "the restarted worker must still fire the await; pending: {:?}",
            store.list()
        );
        assert!(!store.claim(&on_timer.id));
        assert!(
            store
                .scheduler()
                .age(adi_config::now_unix())
                .is_some_and(|age| age <= 5),
            "and it leaves a heartbeat: {:?}",
            store.scheduler()
        );

        drop(tx);
        worker.join().expect("the supervisor never panics");
        let recorded = events
            .diagnostics(&adi_events::diagnostics::Query::default())
            .expect("diagnostics");
        assert!(
            recorded
                .iter()
                .any(|r| r.diagnostic.message.contains("adi-awaits-test panicked")),
            "the escape is on record: {recorded:?}"
        );
        let _ = std::fs::remove_dir_all(store.dir());
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
                .start_manual(
                    &format!("n{i}"),
                    &serde_json::json!({}),
                    &format!("req-{i}"),
                )
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
