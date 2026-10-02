//! The reply to one dispatched message: watch the turn [`crate::dispatch`] just started or
//! continued, and post what the agent said once that turn ends (`docs/channels.md` §5).
//!
//! **Why not `adi.agents.run.finished`.** That event cannot carry this, for two reasons found live
//! in ADI-MONO-124. It is published *once per run, ever* — `SessionStore::record_outcome` writes
//! only where `outcome IS NULL` — so every message after a thread's first, which is a `reply_as`
//! turn on the same run, would never be answered. And it is published lazily, by whichever
//! process next *lists* the agent's runs; on a node where nothing happens to be looking (no panel
//! open), even the first answer sat unposted until somebody did. This watcher is that somebody,
//! for exactly the run it cares about: its own listing is also what records the run's ending, so
//! every other `run.finished` consumer still hears about it.

use std::collections::HashSet;
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

use adi_agents::store::Turn;
use adi_agents::{Agents, RunLifecycle};
use tracing::warn;

use crate::router_api::RouterApi;

/// First poll interval; doubles up to [`MAX_POLL`]. A short agent answer is back in a couple of
/// seconds, which is when a person in a chat is still looking at the "typing…" indicator.
const FIRST_POLL: Duration = Duration::from_millis(500);
const MAX_POLL: Duration = Duration::from_secs(5);
/// A turn that ran this long is still answered if it ever ends inside the window; past it the
/// watcher gives up rather than holding a thread forever on a run that wedged.
const GIVE_UP_AFTER: Duration = Duration::from_secs(6 * 60 * 60);
/// How long a run may sit stopped with no new assistant turn before that is taken as the answer
/// (a launch that failed without writing anything) rather than a turn that hasn't started yet —
/// a message queued behind a busy run reads exactly like this for its first moments.
const NO_ANSWER_GRACE: Duration = Duration::from_secs(60);

/// Everything one watch needs, owned, so it can move onto its own thread.
#[derive(Debug, Clone)]
pub struct Watch {
    pub agent: String,
    pub run_id: String,
    pub connection: String,
    pub thread: String,
    pub router_url: String,
    pub node_token: String,
    /// The message's own text, to find the user turn it became among others recorded since.
    pub text: String,
    /// Unix milliseconds the message was dispatched — no turn recorded before this is its own.
    pub since_ms: u64,
}

/// Assistant turns already posted, as `(run, recorded-at)`. Two messages a queue merged into one
/// turn both find that turn's answer; the first watcher to get here posts it, the other stays
/// quiet — one turn, one reply.
static POSTED: Mutex<Option<HashSet<(String, u64)>>> = Mutex::new(None);

/// Claim an answer for posting; `false` if another watcher already did.
fn claim(run_id: &str, at: u64) -> bool {
    POSTED
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .get_or_insert_with(HashSet::new)
        .insert((run_id.to_string(), at))
}

/// What one look at the run decided.
#[derive(Debug, PartialEq, Eq)]
enum Step {
    Wait,
    /// The turn is over; post this answer (text, and when it was recorded) if any, and stop.
    Done(Option<(String, u64)>),
}

/// One look. `state` is `None` when the run is no longer listed at all (deleted under us).
///
/// The answer is anchored on the message's *own* user turn — the first one recorded since dispatch
/// carrying its text, or failing that the first one recorded since dispatch at all (a queue that
/// merged messages) — and is the first settled assistant turn after it. Never "the newest
/// assistant turn": two quick messages are two turns, and by the time a backed-off watcher looks
/// again the second may have finished too (ADI-MONO-124, live: both replies carried the second
/// answer). The run's state only matters while there's no answer yet — once there is, the run may
/// well be `Running` again on the *next* message.
fn step(state: Option<RunLifecycle>, turns: &[Turn], since_ms: u64, text: &str) -> Step {
    let mine = |t: &&Turn| t.role == "user" && !t.pending && t.at >= since_ms;
    let own = turns
        .iter()
        .position(|t| mine(&t) && t.text.trim() == text.trim())
        .or_else(|| turns.iter().position(|t| mine(&t)));
    let answer = own.and_then(|i| {
        turns[i + 1..]
            .iter()
            .find(|t| t.role == "assistant" && !t.pending)
            .map(|t| (t.text.clone(), t.at))
    });
    match (answer, state) {
        (Some(answer), _) => Step::Done(Some(answer)),
        (None, None) => Step::Done(None),
        (None, Some(_)) => Step::Wait,
    }
}

/// Watch on a thread of its own and return at once — [`crate::dispatch::handle`] runs on the
/// socket's blocking pool, and the next inbound message must not queue behind a long turn.
pub fn spawn(agents: Agents, watch: Watch) {
    std::thread::spawn(move || run(&agents, &watch));
}

fn run(agents: &Agents, watch: &Watch) {
    let started = Instant::now();
    let mut poll = FIRST_POLL;
    let mut stopped_since: Option<Instant> = None;
    let answer = loop {
        std::thread::sleep(poll);
        poll = (poll * 2).min(MAX_POLL);
        if started.elapsed() > GIVE_UP_AFTER {
            warn!(run = %watch.run_id, "channel turn never ended inside the watch window; not posting");
            break None;
        }
        let agent = match agents.get(&watch.agent) {
            Ok(Some(agent)) => agent,
            Ok(None) => break None,
            Err(e) => {
                warn!(agent = %watch.agent, error = %e, "couldn't read the agent a channel turn runs on");
                continue;
            }
        };
        let state = agents
            .runs(&agent)
            .into_iter()
            .find(|r| r.run_id == watch.run_id)
            .map(|r| r.state);
        let turns = if state.is_some() {
            agents.transcript(&agent, &watch.run_id)
        } else {
            Vec::new()
        };
        match step(state, &turns, watch.since_ms, &watch.text) {
            Step::Done(answer) => break answer,
            Step::Wait if state.is_some_and(|s| s != RunLifecycle::Running) => {
                let since = *stopped_since.get_or_insert_with(Instant::now);
                if since.elapsed() > NO_ANSWER_GRACE {
                    break None;
                }
            }
            Step::Wait => stopped_since = None,
        }
    };
    let answer = answer.and_then(|(text, at)| claim(&watch.run_id, at).then_some(text));
    post(watch, answer.as_deref());
}

/// Clear "thinking…" and post the answer. The clear goes first and unconditionally: the turn is
/// over whether or not it left anything worth saying.
fn post(watch: &Watch, answer: Option<&str>) {
    let router = RouterApi::new(&watch.router_url);
    if let Err(e) = router.set_thinking(&watch.node_token, &watch.connection, &watch.thread, "") {
        warn!(connection = %watch.connection, error = %e, "couldn't clear the \"thinking…\" indicator");
    }
    let Some(text) = answer.filter(|t| !t.trim().is_empty()) else {
        return;
    };
    if let Err(e) = router.send(&watch.node_token, &watch.connection, Some(&watch.thread), text) {
        warn!(connection = %watch.connection, error = %e, "couldn't post a channel turn's answer back");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn turn(role: &str, text: &str, at: u64) -> Turn {
        serde_json::from_value(serde_json::json!({ "role": role, "text": text, "at": at })).unwrap()
    }

    fn done(text: &str, at: u64) -> Step {
        Step::Done(Some((text.to_string(), at)))
    }

    #[test]
    fn a_turn_with_no_answer_yet_is_waited_on() {
        let turns = [turn("user", "hi", 10)];
        assert_eq!(step(Some(RunLifecycle::Running), &turns, 5, "hi"), Step::Wait);
        // Stopped but nothing said yet still waits — the grace period in `run` decides when to quit.
        assert_eq!(step(Some(RunLifecycle::Finished), &turns, 5, "hi"), Step::Wait);
    }

    /// The bug the first live test of this watcher hit: "afafa" then "afa" a second apart, two
    /// turns, and both replies carried "afa" because the first watcher took the newest answer.
    #[test]
    fn two_quick_messages_in_one_thread_each_get_their_own_answer() {
        let turns = [
            turn("user", "aaa", 1),
            turn("assistant", "you said aaa", 2),
            turn("user", "afafa", 28_968),
            turn("assistant", "you said afafa", 30_154),
            turn("user", "afa", 31_113),
            turn("assistant", "you said afa", 32_651),
        ];
        // The first message's watcher, dispatched at 28_900, looking only after both turns ended —
        // and while a third turn may already be running.
        assert_eq!(
            step(Some(RunLifecycle::Running), &turns, 28_900, "afafa"),
            done("you said afafa", 30_154)
        );
        assert_eq!(
            step(Some(RunLifecycle::Finished), &turns, 31_050, "afa"),
            done("you said afa", 32_651)
        );
    }

    /// Dispatched while the first turn is still running: the second message's user turn is only
    /// recorded when its own turn starts, so until then it has no answer — even though an
    /// assistant turn newer than its dispatch (the first message's answer) already exists.
    #[test]
    fn a_queued_message_does_not_take_the_answer_of_the_turn_ahead_of_it() {
        let turns = [turn("user", "one", 100), turn("assistant", "you said one", 300)];
        assert_eq!(step(Some(RunLifecycle::Waiting), &turns, 200, "two"), Step::Wait);
    }

    #[test]
    fn an_answer_from_before_dispatch_is_not_this_messages_answer() {
        let turns = [turn("user", "first", 1), turn("assistant", "first answer", 2)];
        assert_eq!(step(Some(RunLifecycle::Finished), &turns, 20, "second"), Step::Wait);
    }

    #[test]
    fn a_still_streaming_turn_is_not_an_answer() {
        let mut streaming = turn("assistant", "half a sent", 30);
        streaming.pending = true;
        let turns = [turn("user", "hi", 25), streaming];
        assert_eq!(step(Some(RunLifecycle::Running), &turns, 20, "hi"), Step::Wait);
    }

    /// A queue that merged two messages into one turn: neither text matches the merged user turn
    /// exactly, both watchers anchor on it, both find the one answer — and only the first to claim
    /// it posts, so the chat gets one reply for one turn rather than two copies.
    #[test]
    fn a_merged_turn_is_answered_once() {
        let turns = [turn("user", "one\n\ntwo", 500), turn("assistant", "you said both", 600)];
        let first = step(Some(RunLifecycle::Finished), &turns, 400, "one");
        let second = step(Some(RunLifecycle::Finished), &turns, 450, "two");
        assert_eq!(first, done("you said both", 600));
        assert_eq!(second, first);
        let run = format!("merged-{}", std::process::id());
        assert!(claim(&run, 600), "first watcher posts");
        assert!(!claim(&run, 600), "second watcher stays quiet");
    }

    #[test]
    fn a_run_that_vanished_ends_the_watch_with_nothing_to_say() {
        assert_eq!(step(None, &[], 20, "hi"), Step::Done(None));
    }
}
