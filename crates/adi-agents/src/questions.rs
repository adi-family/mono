//! The deadline half of asking: what happens when nobody answers.
//!
//! An [ask](crate::store::Ask) that names `after_seconds` is a run saying *I need this decided, but
//! not at any price* — if the answer does not come, take my assumption and carry on. That promise
//! needs a clock, and this crate has none: like [`awaits`](crate::awaits), the store and the
//! decision live here and the app owns the tick, calling [`tick`] once a second.
//!
//! # Why this is not an await
//!
//! An ask *looks* like an await with a different trigger, and the first design here made it one.
//! It does not survive contact with the wake path. An await fires by replying into the
//! conversation, and a reply from a person is exactly what settles a question — so an unrelated
//! await firing while an ask was open would have answered it with its own wake note. Distinguishing
//! them meant a marker on the await record that only the question code could read, which is a
//! record pretending to be two things.
//!
//! What is actually shared is the *worker*, not the record: the app's await thread already wakes
//! once a second with a store handle, and this rides on it. The reuse is three lines at the call
//! site rather than a field nobody else understands.
//!
//! # Settle first, deliver second
//!
//! [`tick`] claims the ask before it says anything, exactly as a wake does. The claim is a
//! conditional `UPDATE`, so a person answering in the same instant either wins (and the sweep finds
//! nothing to do) or loses (and their answer is refused rather than becoming a second turn on a
//! question that has already moved on). Delivery is then [`Agents::deliver`], which settles nothing
//! — the ask is already settled, and going back through `reply` would look for a second question to
//! answer.

use crate::store::{AnsweredBy, now_ms};
use crate::{Agents, Sent};

/// One ask the deadline settled, reported back so the app can log it. `error` is set when the ask
/// was claimed but the message could not be delivered — the default is taken either way, which is
/// why it is worth saying out loud.
#[derive(Debug, Clone)]
pub struct Settled {
    /// The ask's id.
    pub id: String,
    pub agent: String,
    pub conv: String,
    /// The one-line version of what went unanswered.
    pub question: String,
    /// Whether the answer started a turn or joined the conversation's queue.
    pub queued: bool,
    pub error: Option<String>,
}

/// Settle every pending ask whose deadline has passed, taking the run's own default and delivering
/// it into the conversation. Called by the app on its own clock.
///
/// A deadline is in whole seconds, so once a second is as fine as it can be seen; a sweep with
/// nothing overdue is one indexed query.
#[must_use]
pub fn tick(agents: &Agents) -> Vec<Settled> {
    let store = agents.sessions();
    let now = now_ms();
    let mut settled = Vec::new();
    for (ask, default) in store.overdue_questions(now) {
        // Claimed before anything is said, so a person answering this instant either wins outright
        // or is refused — never both.
        match store.resolve_question(&ask.agent, &ask.conv, Some(&ask.id), &default) {
            Ok(Some(_)) => {}
            // Somebody answered between the query and here. Nothing to do, and nothing to report:
            // the conversation is moving again by the route it should have.
            Ok(None) => continue,
            // A store that will not take the claim will not take it on the next tick either;
            // reporting it as a settlement would be a lie, so it is left overdue and visible.
            Err(_) => continue,
        }
        let sent = agents.deliver(&ask.agent, &ask.conv, &ask.render(&default));
        agents.emit_answered(&ask, AnsweredBy::Default);
        settled.push(Settled {
            id: ask.id.clone(),
            agent: ask.agent.clone(),
            conv: ask.conv.clone(),
            question: ask.headline(),
            queued: matches!(sent, Ok(Sent::Queued { .. })),
            error: sent.err().map(|e| e.to_string()),
        });
    }
    settled
}

#[cfg(test)]
mod tests {
    use crate::store::{AskRequest, Question, SessionStore};

    fn one(text: &str) -> Question {
        Question {
            header: String::new(),
            question: text.to_string(),
            options: Vec::new(),
            multi_select: false,
        }
    }

    /// The sweep's own arithmetic, exercised against the store without an `Agents` to deliver
    /// through: what it finds, and that claiming it once leaves nothing for the next pass.
    ///
    /// Delivery is [`Agents::deliver`]'s job and is covered where that is — here the question is
    /// only whether a deadline that has passed produces exactly one settlement.
    #[test]
    fn a_passed_deadline_settles_once_and_then_has_nothing_left_to_do() {
        let dir = std::env::temp_dir().join(format!(
            "adi-question-tick-{}-{:?}",
            std::process::id(),
            std::thread::current().id(),
        ));
        let _ = std::fs::remove_dir_all(&dir);
        let store = SessionStore::new(&dir);
        let conv = store
            .create("chat", crate::Backend::HarnessAdi, "/tmp", "go")
            .expect("create")
            .id;
        store
            .ask(
                "chat",
                &conv,
                &AskRequest {
                    questions: vec![one("ship it?")],
                    after_seconds: Some(1),
                    defaults: vec!["no".to_string()],
                    ..AskRequest::default()
                },
            )
            .expect("ask");

        let deadline = store
            .pending_question("chat", &conv)
            .expect("pending")
            .deadline
            .expect("a deadline");
        let due = store.overdue_questions(deadline);
        assert_eq!(due.len(), 1, "one ask is overdue");

        let (ask, default) = &due[0];
        assert!(
            store
                .resolve_question("chat", &conv, Some(&ask.id), default)
                .expect("resolve")
                .is_some(),
            "the sweep claims it"
        );
        assert!(
            store.overdue_questions(deadline).is_empty(),
            "and the next pass finds nothing — a default is taken once"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }
}
