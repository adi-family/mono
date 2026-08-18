//! The await worker — the app's clock for runs that asked to be woken.
//!
//! A harness run can register an [await](adi_agents::awaits): *wake this conversation when such an
//! event is published, or when this much time has passed, if this check agrees*. The store and the
//! decision live in `adi-agents`; what lives here is the only thing that crate cannot have — a
//! clock, and a seat at the event spool.
//!
//! Both arrive on one thread, for one reason: an await's check is a real command, and a command
//! takes as long as it takes. Running one on the [event
//! dispatcher](adi_triggers::EventDispatcher)'s tick would hold up every trigger in the platform
//! behind somebody's `curl`; running one per event on its own thread would fork-bomb the machine on
//! a burst. So the dispatcher's observer only *posts* the event here, the worker takes them one at a
//! time, and the same worker runs the second-by-second sweep for deadlines while it waits.
//!
//! The second-by-second sweep now carries three riders, in the order a conversation would want
//! them: [awaits](adi_agents::awaits) firing, [questions](adi_agents::questions) whose deadline
//! passed taking their default, and [goals](adi_agents::goals) asking a conversation that has
//! fallen quiet whether it is done. Goals go last for a reason — the first two can each *un*-quiet
//! a conversation, and a goal check that ran before them would ask a run about its goal in the same
//! second something else woke it.
//!
//! The worker outlives nothing in particular: it holds a store handle and a channel, and it ends
//! when the dispatcher that feeds it is dropped.

use std::sync::Arc;
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::time::{Duration, Instant};

use adi_agents::{Agents, awaits, goals, questions};
use adi_events::EventRecord;
use adi_triggers::EventObserver;
use tracing::{info, warn};

/// How often deadlines are swept. An await's timer is in whole seconds, so nothing finer would be
/// visible; a sweep over an empty store is one directory read.
const TICK: Duration = Duration::from_secs(1);

/// One published event, owned — the observer runs on the dispatcher's tick and must not keep it
/// waiting, so it copies the two fields the worker needs and returns.
type Posted = (String, String);

/// Start the worker over `agents`' store and return the observer to hand
/// [`EventDispatcher::start_watched`](adi_triggers::EventDispatcher::start_watched).
///
/// Dropping every clone of the returned observer ends the worker, which is what happens when the
/// dispatcher stops on the way out.
pub fn start(agents: Agents) -> EventObserver {
    let (tx, rx) = channel::<Posted>();
    std::thread::spawn(move || run(&agents, &rx));
    observer(tx)
}

/// The dispatcher-facing half: hand the event to the worker and get out of the way. A send that
/// fails means the worker has ended, which is not worth a log line on every remaining event.
fn observer(tx: Sender<Posted>) -> EventObserver {
    Arc::new(move |record: &EventRecord| {
        let _ = tx.send((record.name.clone(), record.payload.clone()));
    })
}

/// Take posted events one at a time, and sweep deadlines whenever a tick has come round — including
/// under a steady stream of events, which is why the deadline is tracked rather than inferred from
/// a `recv_timeout` that never times out.
fn run(agents: &Agents, rx: &Receiver<Posted>) {
    let mut next_tick = Instant::now() + TICK;
    loop {
        let wait = next_tick.saturating_duration_since(Instant::now());
        match rx.recv_timeout(wait) {
            Ok((name, payload)) => report(&awaits::on_event(agents, &name, &payload)),
            Err(RecvTimeoutError::Timeout) => {}
            // Every observer is gone: nothing will ever post again, and the app is on its way out.
            Err(RecvTimeoutError::Disconnected) => break,
        }
        if Instant::now() >= next_tick {
            report(&awaits::tick(agents));
            report_settled(&questions::tick(agents));
            report_nudged(&goals::tick(agents));
            next_tick = Instant::now() + TICK;
        }
    }
}

/// Say what woke. A wake is a conversation starting a turn on its own, which is exactly the kind of
/// thing someone reading the log later wants to find — and a wake whose reply failed is worth a
/// warning, because the await is spent either way.
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

/// Say what a lapsed question decided on the run's behalf.
///
/// Louder than a wake and deliberate: a default taken is a decision nobody made, and the whole
/// reason it is allowed is that it is visible afterward. A fleet whose log is full of these is a
/// fleet asking questions nobody is there to answer.
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

/// Say which conversations were asked about their goals.
///
/// Quieter than either of the above, because a nudge is the system working rather than something
/// going unattended — but worth having, since a goal is the one thing here nothing will ever close
/// on the run's behalf. A conversation whose id keeps appearing is a run circling a goal it will
/// neither meet nor give up on, and this log line is where that becomes visible.
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

    use adi_agents::awaits::{Awaits, Request};

    fn scratch(tag: &str) -> adi_config::Config {
        let root = std::env::temp_dir().join(format!(
            "adi-app-awaits-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id(),
        ));
        let _ = std::fs::remove_dir_all(&root);
        adi_config::Config::with_root(root)
    }

    /// The worker's whole contract in one pass: an event posted through the observer reaches the
    /// store and claims the await that wanted it, and a deadline is swept without any event at all.
    /// Neither wake can be *delivered* here — there is no conversation behind these ids — but firing
    /// is what this module owns, and a failed delivery still spends the await.
    #[test]
    fn a_posted_event_and_a_passing_deadline_both_reach_the_store() {
        let config = scratch("worker");
        let agents = Agents::with_config(config.clone());
        let store = Awaits::with_config(config);

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

        let observer = start(agents);
        observer(&EventRecord {
            name: "adi.tasks.created".into(),
            payload: r#"{"id":"t1"}"#.into(),
            emitted_at: 0,
        });

        // The event one goes almost at once; the timer one waits out its second plus a sweep.
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline && !store.list().is_empty() {
            std::thread::sleep(Duration::from_millis(50));
        }
        assert!(
            store.list().is_empty(),
            "both awaits should have fired; still pending: {:?}",
            store.list()
        );
        // …and each was claimed exactly once, so neither can fire again.
        assert!(!store.claim(&on_event.id));
        assert!(!store.claim(&on_timer.id));

        let _ = std::fs::remove_dir_all(store.dir());
    }

    /// The wake an action registers on its caller's behalf, put through the worker the app really
    /// runs. `adi.agents.run.finished` is published for every run on the machine, so a supervisor
    /// that launched one must not be woken — and told it was its own — by a stranger's ending.
    #[test]
    fn a_wake_scoped_to_one_run_survives_another_runs_ending() {
        let config = scratch("scoped");
        let agents = Agents::with_config(config.clone());
        let store = Awaits::with_config(config);

        let scoped = awaits::register(
            &store,
            "supervisor",
            "conv-1",
            &Request {
                note: "the agent you started ended".into(),
                events: vec!["adi.agents.run.finished".into()],
                when: [("run_id".to_string(), "r-42".to_string())]
                    .into_iter()
                    .collect(),
                ..Request::default()
            },
        )
        .expect("register");

        let finished = |run_id: &str| EventRecord {
            name: "adi.agents.run.finished".into(),
            payload: format!(r#"{{"agent":"worker","run_id":"{run_id}","is_error":false}}"#),
            emitted_at: 0,
        };
        let observer = start(agents);

        // Long enough to be sure the worker has been through it — the other test's event fires
        // almost at once — and the whole point is that nothing happens.
        observer(&finished("r-43"));
        std::thread::sleep(Duration::from_millis(500));
        assert_eq!(store.list().len(), 1, "a stranger's ending is not this run's");

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
