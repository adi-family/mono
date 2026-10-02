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
    /// Unix milliseconds the message was dispatched — an assistant turn recorded before this
    /// answered something earlier, not this message.
    pub since_ms: u64,
}

/// What one look at the run decided.
#[derive(Debug, PartialEq, Eq)]
enum Step {
    Wait,
    /// The turn is over; post this (if anything) and stop.
    Done(Option<String>),
}

/// One look. `state` is `None` when the run is no longer listed at all (deleted under us).
fn step(state: Option<RunLifecycle>, turns: &[Turn], since_ms: u64) -> Step {
    let Some(state) = state else {
        return Step::Done(None);
    };
    if state == RunLifecycle::Running {
        return Step::Wait;
    }
    turns
        .iter()
        .rev()
        .find(|t| t.role == "assistant" && !t.pending && t.at >= since_ms)
        .map_or(Step::Wait, |t| Step::Done(Some(t.text.clone())))
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
        let turns = match state {
            Some(RunLifecycle::Running) | None => Vec::new(),
            Some(_) => agents.transcript(&agent, &watch.run_id),
        };
        match step(state, &turns, watch.since_ms) {
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

    #[test]
    fn a_running_turn_is_waited_on() {
        let turns = [turn("assistant", "old answer", 10)];
        assert_eq!(step(Some(RunLifecycle::Running), &turns, 5), Step::Wait);
    }

    #[test]
    fn a_stopped_turn_posts_the_newest_assistant_text_since_dispatch() {
        let turns = [
            turn("user", "first", 1),
            turn("assistant", "first answer", 2),
            turn("user", "second", 20),
            turn("assistant", "second answer", 25),
        ];
        assert_eq!(
            step(Some(RunLifecycle::Finished), &turns, 20),
            Step::Done(Some("second answer".into()))
        );
        // `Waiting` (an await or a queued message holds the run) still ends *this* turn.
        assert_eq!(
            step(Some(RunLifecycle::Waiting), &turns, 20),
            Step::Done(Some("second answer".into()))
        );
    }

    /// The follow-up case `run.finished` could never answer: the run already has an answer from
    /// an earlier turn, and that one must not be re-posted for the new message.
    #[test]
    fn an_answer_from_before_dispatch_is_not_this_messages_answer() {
        let turns = [turn("user", "first", 1), turn("assistant", "first answer", 2)];
        assert_eq!(step(Some(RunLifecycle::Finished), &turns, 20), Step::Wait);
    }

    #[test]
    fn a_still_streaming_turn_is_not_an_answer() {
        let mut streaming = turn("assistant", "half a sent", 30);
        streaming.pending = true;
        assert_eq!(step(Some(RunLifecycle::Finished), &[streaming], 20), Step::Wait);
    }

    #[test]
    fn a_run_that_vanished_ends_the_watch_with_nothing_to_say() {
        assert_eq!(step(None, &[], 20), Step::Done(None));
    }
}
