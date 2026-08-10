//! What a run wants to know before it can go on: the `questions` table, one row per **ask**.
//!
//! A turn that needs a human decision has no way to sit and wait for one. It ends — like every
//! other turn — and what it leaves behind is a row here saying *this conversation is blocked on
//! you*. The answer arrives as an ordinary reply, the next turn replays the whole transcript, and
//! the run comes back with its own question and the answer to it both in front of it. Nothing is
//! held open in the meantime: no child process, no provider context, no slot against the
//! [run cap](crate::RunLimits).
//!
//! This is the same bargain [`awaits`](crate::awaits) makes with the clock, and it is made here for
//! the same reason — a turn is the unit that ends, so anything outliving one is a record rather
//! than a wait.
//!
//! # An ask, not a question
//!
//! One row holds **all** the questions a turn wanted answered, up to [`MAX_QUESTIONS`]. That is not
//! packaging: every answer costs a turn, and every turn replays the transcript, so four questions
//! asked one at a time cost four replays of a conversation that only grew longer each time. Asked
//! together they cost one, and the person answering sees the whole decision instead of being led
//! through it a click at a time.
//!
//! **At most one ask is pending per conversation.** A second one can only come from a turn that
//! started after the first was answered — so an attempt to register one while another is open is a
//! turn asking twice, and it is refused with a message telling the model to put its questions in a
//! single call.
//!
//! # Claiming, and why it is a conditional UPDATE
//!
//! Three things can settle an ask, and two of them can happen at the same instant: a person
//! answering in the app, the same person answering from the CLI in another process, and the
//! deadline sweep giving up and taking the default. Exactly one must win, because the winner is the
//! one that starts a turn. [`resolve`] is therefore an `UPDATE … WHERE answer IS NULL`: the row is
//! the lock, held by the database rather than by any one process's memory, and the loser sees zero
//! rows changed and does nothing.

use rusqlite::{Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

use super::db::{now_ms, sql_err};

/// The most questions one ask may carry. Four is what fits in a glance — past that a person is
/// filling in a form rather than making a decision, and a turn that needs a form is a turn that
/// should have asked for less.
pub const MAX_QUESTIONS: usize = 4;

/// The longest an option label may be before it stops being a button and starts being a paragraph.
const MAX_LABEL: usize = 80;

/// One thing a run wants to know.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Question {
    /// A two-or-three word tag for the decision — "Auth method", "Rollout". Shown as a chip beside
    /// the question so a listing of pending asks reads without opening any of them.
    #[serde(default)]
    pub header: String,
    /// The question itself, as it is put to the person.
    pub question: String,
    /// The answers worth offering as one tap. Empty means free text only.
    #[serde(default)]
    pub options: Vec<Choice>,
    /// Whether more than one option may be picked.
    #[serde(default)]
    pub multi_select: bool,
}

/// One offered answer: what the button says, and what it means.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Choice {
    pub label: String,
    /// What choosing this implies — the sentence under the button. Optional, and worth writing:
    /// it is the difference between picking a word and making a decision.
    #[serde(default)]
    pub description: String,
}

/// What settled an ask. Recorded because it changes how the answer should be read: a person meant
/// it, a default merely happened.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AnsweredBy {
    /// A person answered — in the app, from the CLI, or through whatever forwarded it to them.
    Human,
    /// Nobody did, and the ask named a default to fall back on when its deadline passed.
    Default,
}

/// How an ask was settled.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Answer {
    /// Unix milliseconds it was settled.
    pub at: u64,
    pub by: AnsweredBy,
    /// What was said back.
    ///
    /// Usually one reply per question, in the order they were asked — a question answered with
    /// nothing keeps its empty string rather than being dropped, so the two lists stay aligned.
    /// The exception is a person who ignored the card and simply typed into the chat: that is one
    /// entry answering the whole ask, because prose aimed at three questions cannot honestly be
    /// split across three slots. [`Ask::render`] pairs the first shape and
    /// [`Ask::render_reply`] carries the second.
    pub replies: Vec<String>,
}

/// A run's request for a human decision, pending or settled.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ask {
    /// This ask's id, unique within its conversation.
    pub id: String,
    pub agent: String,
    /// The conversation blocked on it — where the answer is delivered.
    pub conv: String,
    /// Unix milliseconds it was asked.
    pub asked_at: u64,
    /// What the run was doing when it stopped to ask, in its own words. Shown above the questions,
    /// because a question out of context is answered wrongly by people who would have got it right.
    #[serde(default)]
    pub note: String,
    pub questions: Vec<Question>,
    /// Unix milliseconds after which [`defaults`](Self::defaults) are taken instead. `None` means
    /// it waits indefinitely — which is right for work someone is watching, and wrong for anything
    /// a trigger launched at three in the morning.
    #[serde(default)]
    pub deadline: Option<u64>,
    /// What to answer with if the deadline passes, one per question. Empty means there is no
    /// default, and a deadline without one is not a deadline — [`register`] rejects the pair.
    #[serde(default)]
    pub defaults: Vec<String>,
    /// How it was settled, or `None` while it is still waiting on someone.
    #[serde(default)]
    pub answer: Option<Answer>,
}

impl Ask {
    /// Whether this ask is still waiting on someone.
    #[must_use]
    pub fn pending(&self) -> bool {
        self.answer.is_none()
    }

    /// The one-line version, for a rail badge or a notification: the first question's header and
    /// text, and how many more came with it.
    #[must_use]
    pub fn headline(&self) -> String {
        let Some(first) = self.questions.first() else {
            return "a question".to_string();
        };
        let mut line = if first.header.trim().is_empty() {
            first.question.clone()
        } else {
            format!("{}: {}", first.header, first.question)
        };
        let rest = self.questions.len().saturating_sub(1);
        if rest > 0 {
            let plural = if rest == 1 { "" } else { "s" };
            line.push_str(&format!(" (+{rest} more question{plural})"));
        }
        line
    }

    /// The user turn an answer is delivered as.
    ///
    /// Written to be read by the model that asked, and it repeats each question before its answer
    /// for a reason that is easy to miss: the reply lands in a transcript the model replays from
    /// the top, and "Postgres" on its own is an answer to whichever question the model *believes*
    /// it asked. Pairing them makes the transcript answer that for it.
    #[must_use]
    pub fn render(&self, answer: &Answer) -> String {
        use std::fmt::Write as _;

        let mut text = format!("[ask {} — answered", self.id);
        if answer.by == AnsweredBy::Default {
            text.push_str(" by default, because nobody answered in time");
        }
        text.push_str("]\n");
        for (index, question) in self.questions.iter().enumerate() {
            let reply = answer
                .replies
                .get(index)
                .map(String::as_str)
                .unwrap_or("")
                .trim();
            let _ = write!(text, "\n{}\n", question.question.trim());
            if reply.is_empty() {
                let _ = writeln!(text, "> (no answer)");
            } else {
                for line in reply.lines() {
                    let _ = writeln!(text, "> {line}");
                }
            }
        }
        text.push_str(
            "\nThis ask is settled. Carry on from here — ask again only if something new needs \
             deciding.",
        );
        text
    }

    /// The user turn a *typed* answer is delivered as: the marker, then what they wrote, verbatim.
    ///
    /// Nobody made this person fill the card in, so nothing here pretends they did. Prose written
    /// at three questions is not three answers, and splitting it across three slots would put words
    /// in their mouth — the marker says which ask is closed and the rest is theirs.
    #[must_use]
    pub fn render_reply(&self, text: &str) -> String {
        format!("[ask {} — answered]\n\n{text}", self.id)
    }

    /// The answer to fall back on when the deadline passes, or `None` if there is nothing to fall
    /// back to. Padded to one reply per question so [`render`](Self::render) stays aligned.
    #[must_use]
    fn default_answer(&self, now: u64) -> Option<Answer> {
        if self.defaults.iter().all(|d| d.trim().is_empty()) {
            return None;
        }
        let mut replies = self.defaults.clone();
        replies.resize(self.questions.len(), String::new());
        Some(Answer {
            at: now,
            by: AnsweredBy::Default,
            replies,
        })
    }
}

/// What a turn asked for, before it becomes a stored [`Ask`]. The tool builds one of these from the
/// model's arguments; [`register`] is what turns it into a row.
#[derive(Debug, Clone, Default)]
pub struct Request {
    pub note: String,
    pub questions: Vec<Question>,
    /// Take the defaults this many seconds from now.
    pub after_seconds: Option<u64>,
    pub defaults: Vec<String>,
}

/// Write down a conversation's question and return the stored ask.
///
/// # Errors
/// [`Error::Arguments`] for a request that is empty, oversized, malformed, or arrives while another
/// ask is still pending — every one of them phrased for the model that will read it as a failed
/// tool result. Database errors otherwise.
pub(super) fn register(
    conn: &Connection,
    agent: &str,
    conv: &str,
    req: &Request,
) -> Result<Ask> {
    if req.questions.is_empty() {
        return Err(Error::Arguments(
            "an ask needs at least one question — say what you want decided".to_string(),
        ));
    }
    if req.questions.len() > MAX_QUESTIONS {
        return Err(Error::Arguments(format!(
            "{} questions is more than the {MAX_QUESTIONS} one ask may carry — ask for the \
             decisions you cannot proceed without, and work the rest out yourself",
            req.questions.len()
        )));
    }
    for question in &req.questions {
        if question.question.trim().is_empty() {
            return Err(Error::Arguments(
                "every question needs text — an empty one cannot be answered".to_string(),
            ));
        }
        for option in &question.options {
            if option.label.trim().is_empty() {
                return Err(Error::Arguments(
                    "every option needs a label — that is what the button says".to_string(),
                ));
            }
            if option.label.chars().count() > MAX_LABEL {
                return Err(Error::Arguments(format!(
                    "the option {:?} is too long to be a button — keep a label under {MAX_LABEL} \
                     characters and put the reasoning in its `description`",
                    option.label
                )));
            }
        }
    }
    // A deadline nobody set an answer for is a deadline that can only pass in silence.
    if req.after_seconds.is_some() && req.defaults.iter().all(|d| d.trim().is_empty()) {
        return Err(Error::Arguments(
            "`after_seconds` needs `defaults` — a deadline is what to do when nobody answers, so \
             it is only a deadline if you say what to assume"
                .to_string(),
        ));
    }

    let tx = conn
        .unchecked_transaction()
        .map_err(|e| sql_err("record a question in", e))?;
    // Read and insert under one lock: two turns of the same conversation cannot both find nothing
    // pending and both write. (They should not be able to run at once at all — but the store is
    // reached from three processes, and this is one line either way.)
    let open: i64 = tx
        .query_row(
            "SELECT COUNT(*) FROM questions
             WHERE agent = ?1 AND session = ?2 AND answer IS NULL",
            [agent, conv],
            |row| row.get(0),
        )
        .map_err(|e| sql_err("record a question in", e))?;
    if open > 0 {
        return Err(Error::Arguments(
            "you have already asked this conversation a question and it has not been answered — \
             end your turn and wait for it. Everything you need decided goes in one ask."
                .to_string(),
        ));
    }

    let now = now_ms();
    let ask = Ask {
        id: new_id(now),
        agent: agent.to_string(),
        conv: conv.to_string(),
        asked_at: now,
        note: req.note.trim().to_string(),
        questions: req.questions.clone(),
        deadline: req
            .after_seconds
            .map(|secs| now.saturating_add(secs.max(1).saturating_mul(1_000))),
        defaults: req.defaults.clone(),
        answer: None,
    };
    let json = encode(&ask)?;
    tx.execute(
        "INSERT INTO questions (agent, session, id, asked_at, deadline, json, answer)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, NULL)",
        rusqlite::params![agent, conv, ask.id, ask.asked_at, ask.deadline, json],
    )
    .map_err(|e| sql_err("record a question in", e))?;
    tx.commit()
        .map_err(|e| sql_err("record a question in", e))?;
    Ok(ask)
}

/// Settle an ask, returning it with the answer on it — or `None` when there was nothing left to
/// settle, because someone else got there first.
///
/// `ask_id` names one; `None` takes whatever this conversation has pending, which is what a person
/// typing an ordinary reply into the chat means. Either way the claim is the `UPDATE`: see the
/// module note on why the row is the lock.
///
/// # Errors
/// Returns database and encoding errors.
pub(super) fn resolve(
    conn: &Connection,
    agent: &str,
    conv: &str,
    ask_id: Option<&str>,
    answer: &Answer,
) -> Result<Option<Ask>> {
    let tx = conn
        .unchecked_transaction()
        .map_err(|e| sql_err("answer a question in", e))?;
    let target: Option<String> = match ask_id {
        Some(id) => tx
            .query_row(
                "SELECT id FROM questions
                 WHERE agent = ?1 AND session = ?2 AND id = ?3 AND answer IS NULL",
                [agent, conv, id],
                |row| row.get(0),
            )
            .optional()
            .map_err(|e| sql_err("answer a question in", e))?,
        None => tx
            .query_row(
                "SELECT id FROM questions
                 WHERE agent = ?1 AND session = ?2 AND answer IS NULL
                 ORDER BY asked_at, id LIMIT 1",
                [agent, conv],
                |row| row.get(0),
            )
            .optional()
            .map_err(|e| sql_err("answer a question in", e))?,
    };
    let Some(target) = target else {
        return Ok(None);
    };
    let encoded = serde_json::to_string(answer)
        .map_err(|e| Error::Session(format!("couldn't encode an answer: {e}")))?;
    let changed = tx
        .execute(
            "UPDATE questions SET answer = ?4
             WHERE agent = ?1 AND session = ?2 AND id = ?3 AND answer IS NULL",
            rusqlite::params![agent, conv, target, encoded],
        )
        .map_err(|e| sql_err("answer a question in", e))?;
    if changed == 0 {
        return Ok(None);
    }
    let row = load_one(&tx, agent, conv, &target)?;
    tx.commit()
        .map_err(|e| sql_err("answer a question in", e))?;
    Ok(row)
}

/// The ask this conversation is waiting on, if it is waiting on one.
pub(super) fn pending(conn: &Connection, agent: &str, conv: &str) -> Option<Ask> {
    conn.query_row(
        "SELECT json, answer FROM questions
         WHERE agent = ?1 AND session = ?2 AND answer IS NULL
         ORDER BY asked_at, id LIMIT 1",
        [agent, conv],
        decode_row,
    )
    .optional()
    .ok()
    .flatten()
    .flatten()
}

/// Every ask this conversation has made, oldest first — the pending one and the settled ones alike.
pub(super) fn history(conn: &Connection, agent: &str, conv: &str) -> Vec<Ask> {
    let Ok(mut stmt) = conn.prepare_cached(
        "SELECT json, answer FROM questions
         WHERE agent = ?1 AND session = ?2 ORDER BY asked_at, id",
    ) else {
        return Vec::new();
    };
    stmt.query_map([agent, conv], decode_row)
        .map(|rows| rows.flatten().flatten().collect())
        .unwrap_or_default()
}

/// Every ask anywhere that is still waiting on someone, oldest first.
///
/// One query for the whole store, because the caller is an inbox: "what needs me?" is a question
/// about the machine, not about an agent, and asking it per conversation would be a round trip per
/// row to answer *nothing pending* in nearly every case.
pub(super) fn all_pending(conn: &Connection) -> Vec<Ask> {
    let Ok(mut stmt) = conn.prepare_cached(
        "SELECT json, answer FROM questions WHERE answer IS NULL ORDER BY asked_at, id",
    ) else {
        return Vec::new();
    };
    stmt.query_map([], decode_row)
        .map(|rows| rows.flatten().flatten().collect())
        .unwrap_or_default()
}

/// Every pending ask whose deadline has passed, paired with the default answer to settle it with.
///
/// The default is computed here rather than by the caller so the sweep cannot invent one: an ask
/// with a deadline and nothing to fall back on never reaches this list, and [`register`] makes sure
/// that pair cannot be written in the first place.
pub(super) fn overdue(conn: &Connection, now: u64) -> Vec<(Ask, Answer)> {
    let Ok(mut stmt) = conn.prepare_cached(
        "SELECT json, answer FROM questions
         WHERE answer IS NULL AND deadline IS NOT NULL AND deadline <= ?1
         ORDER BY deadline, id",
    ) else {
        return Vec::new();
    };
    stmt.query_map([now], decode_row)
        .map(|rows| {
            rows.flatten()
                .flatten()
                .filter_map(|ask| {
                    let answer = ask.default_answer(now)?;
                    Some((ask, answer))
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Drop every *unanswered* ask belonging to an agent, returning how many went — what deleting the
/// agent takes with it.
///
/// Only the unanswered ones. A settled ask is history, and the transcript it belongs to outlives
/// the definition on purpose (see [`SessionStore::agents`](super::SessionStore::agents)); a pending
/// one is a thing to *do*, and there is no longer anything that could read the answer. Leaving them
/// would put a question in the inbox that cannot be answered — [`Agents::answer`](crate::Agents::answer)
/// looks the agent up first and would refuse it for ever.
pub(super) fn forget_agent(conn: &Connection, agent: &str) -> usize {
    conn.execute(
        "DELETE FROM questions WHERE agent = ?1 AND answer IS NULL",
        [agent],
    )
    .unwrap_or(0)
}

// ---- internals ---------------------------------------------------------------------

/// Read one row back by id, inside whatever transaction is open.
fn load_one(conn: &Connection, agent: &str, conv: &str, id: &str) -> Result<Option<Ask>> {
    conn.query_row(
        "SELECT json, answer FROM questions WHERE agent = ?1 AND session = ?2 AND id = ?3",
        [agent, conv, id],
        decode_row,
    )
    .optional()
    .map(Option::flatten)
    .map_err(|e| sql_err("read a question from", e))
}

/// Rebuild an ask from its two JSON columns.
///
/// A row that will not parse reads as `None` rather than failing the query: one unreadable record
/// must not stop every other conversation from showing what it is waiting on. The signature is
/// `Result<Option<_>>` because that is what `query_map` needs to keep the row-level error channel
/// for real database errors.
fn decode_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Option<Ask>> {
    let json: String = row.get(0)?;
    let answer: Option<String> = row.get(1)?;
    let Ok(mut ask) = serde_json::from_str::<Ask>(&json) else {
        return Ok(None);
    };
    // The answer column is the truth about whether this is settled — the copy inside `json` is the
    // ask as it was written, before anybody had answered it.
    ask.answer = answer.and_then(|text| serde_json::from_str(&text).ok());
    Ok(Some(ask))
}

fn encode(ask: &Ask) -> Result<String> {
    serde_json::to_string(ask)
        .map_err(|e| Error::Session(format!("couldn't encode the question: {e}")))
}

/// A unique, time-sortable ask id. The same shape as an await's, and for the same reason: it is
/// quoted back to the model and shown to a person, so it wants to be short and obviously an id.
fn new_id(now: u64) -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let seq = SEQ.fetch_add(1, Ordering::Relaxed);
    format!("q-{now:013}-{seq:04}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Backend;
    use crate::store::SessionStore;

    fn scratch(tag: &str) -> SessionStore {
        let dir = std::env::temp_dir().join(format!(
            "adi-questions-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id(),
        ));
        super::super::db::forget_connections();
        let _ = std::fs::remove_dir_all(&dir);
        SessionStore::new(dir)
    }

    fn one(text: &str) -> Question {
        Question {
            header: "Choice".to_string(),
            question: text.to_string(),
            options: vec![
                Choice {
                    label: "yes".to_string(),
                    description: String::new(),
                },
                Choice {
                    label: "no".to_string(),
                    description: String::new(),
                },
            ],
            multi_select: false,
        }
    }

    fn human(replies: &[&str]) -> Answer {
        Answer {
            at: now_ms(),
            by: AnsweredBy::Human,
            replies: replies.iter().map(|r| (*r).to_string()).collect(),
        }
    }

    fn open_session(store: &SessionStore, agent: &str) -> String {
        store
            .create(agent, Backend::HarnessAdi, "/tmp", "go")
            .expect("create")
            .id
    }

    /// The round trip, and the thing the whole design rests on: an ask outlives the turn that made
    /// it, so it has to come back exactly as it was written.
    #[test]
    fn an_ask_is_pending_until_it_is_answered_and_then_carries_the_answer() {
        let store = scratch("round-trip");
        let conv = open_session(&store, "chat");

        let ask = store
            .ask(
                "chat",
                &conv,
                &Request {
                    note: "picking a database".to_string(),
                    questions: vec![one("Postgres or SQLite?")],
                    ..Request::default()
                },
            )
            .expect("ask");
        assert!(ask.pending());
        assert_eq!(
            store.pending_question("chat", &conv).expect("pending").id,
            ask.id
        );

        let settled = store
            .resolve_question("chat", &conv, Some(&ask.id), &human(&["Postgres"]))
            .expect("resolve")
            .expect("claimed");
        assert_eq!(settled.answer.expect("answered").replies, ["Postgres"]);
        assert!(
            store.pending_question("chat", &conv).is_none(),
            "an answered ask no longer blocks the conversation"
        );
        assert_eq!(store.question_history("chat", &conv).len(), 1);

        let _ = std::fs::remove_dir_all(store.dir());
    }

    /// Two people answering the same question at the same moment is the ordinary case — the app and
    /// the CLI both reach this store, and the deadline sweep is a third. Exactly one may win,
    /// because winning is what starts a turn.
    #[test]
    fn only_the_first_answer_claims_an_ask() {
        let store = scratch("claim");
        let conv = open_session(&store, "chat");
        let ask = store
            .ask(
                "chat",
                &conv,
                &Request {
                    questions: vec![one("ship it?")],
                    ..Request::default()
                },
            )
            .expect("ask");

        let first = store
            .resolve_question("chat", &conv, Some(&ask.id), &human(&["yes"]))
            .expect("resolve");
        let second = store
            .resolve_question("chat", &conv, Some(&ask.id), &human(&["no"]))
            .expect("resolve");

        assert!(first.is_some(), "the first caller claims it");
        assert!(second.is_none(), "the second finds nothing left to settle");
        let history = store.question_history("chat", &conv);
        assert_eq!(
            history[0].answer.as_ref().expect("answered").replies,
            ["yes"],
            "and the answer stays the one that won"
        );

        let _ = std::fs::remove_dir_all(store.dir());
    }

    /// A person typing into the chat box names no ask id. That has to reach the question they are
    /// looking at, or the card sits there answered-but-still-showing.
    #[test]
    fn an_unaddressed_answer_settles_whatever_is_pending() {
        let store = scratch("unaddressed");
        let conv = open_session(&store, "chat");
        store
            .ask(
                "chat",
                &conv,
                &Request {
                    questions: vec![one("which region?")],
                    ..Request::default()
                },
            )
            .expect("ask");

        let settled = store
            .resolve_question("chat", &conv, None, &human(&["eu-west"]))
            .expect("resolve");
        assert!(settled.is_some());
        assert!(store.pending_question("chat", &conv).is_none());

        let _ = std::fs::remove_dir_all(store.dir());
    }

    /// One ask at a time. A turn that asks twice is a turn that should have asked once with two
    /// questions, and it hears exactly that.
    #[test]
    fn a_second_ask_is_refused_while_the_first_is_open() {
        let store = scratch("one-at-a-time");
        let conv = open_session(&store, "chat");
        let req = Request {
            questions: vec![one("first?")],
            ..Request::default()
        };
        store.ask("chat", &conv, &req).expect("first ask");

        let err = store.ask("chat", &conv, &req).expect_err("refused");
        assert!(
            err.to_string().contains("one ask"),
            "the model is told to batch its questions: {err}"
        );

        // Answering it clears the way for the next turn to ask something new.
        store
            .resolve_question("chat", &conv, None, &human(&["ok"]))
            .expect("resolve");
        store.ask("chat", &conv, &req).expect("second ask");

        let _ = std::fs::remove_dir_all(store.dir());
    }

    /// The unattended case: nobody answers, the deadline passes, and the run gets on with the
    /// assumption it named rather than waiting for ever.
    #[test]
    fn an_overdue_ask_comes_back_with_its_default() {
        let store = scratch("overdue");
        let conv = open_session(&store, "chat");
        let ask = store
            .ask(
                "chat",
                &conv,
                &Request {
                    questions: vec![one("deploy now?")],
                    after_seconds: Some(1),
                    defaults: vec!["no — wait for a human".to_string()],
                    ..Request::default()
                },
            )
            .expect("ask");
        let deadline = ask.deadline.expect("a deadline was named");

        assert!(
            store.overdue_questions(deadline - 1).is_empty(),
            "nothing is overdue before its deadline"
        );
        let due = store.overdue_questions(deadline);
        assert_eq!(due.len(), 1);
        let (overdue, default) = &due[0];
        assert_eq!(overdue.id, ask.id);
        assert_eq!(default.by, AnsweredBy::Default);
        assert_eq!(default.replies, ["no — wait for a human"]);

        // And the sweep settles it exactly as a person would.
        store
            .resolve_question("chat", &conv, Some(&overdue.id), default)
            .expect("resolve")
            .expect("claimed");
        assert!(store.overdue_questions(deadline).is_empty());

        let _ = std::fs::remove_dir_all(store.dir());
    }

    /// A deadline is a promise about what happens when it passes. Without a default there is
    /// nothing to promise, so the pair is refused rather than silently waiting for ever.
    #[test]
    fn a_deadline_without_a_default_is_refused() {
        let store = scratch("no-default");
        let conv = open_session(&store, "chat");
        let err = store
            .ask(
                "chat",
                &conv,
                &Request {
                    questions: vec![one("deploy now?")],
                    after_seconds: Some(60),
                    ..Request::default()
                },
            )
            .expect_err("refused");
        assert!(err.to_string().contains("defaults"), "{err}");

        let _ = std::fs::remove_dir_all(store.dir());
    }

    /// Every answer costs a turn and every turn replays the transcript — so the cap is on how much
    /// one ask may carry, not on how often a run may ask.
    #[test]
    fn an_ask_may_not_carry_more_questions_than_fit_in_a_glance() {
        let store = scratch("cap");
        let conv = open_session(&store, "chat");
        let err = store
            .ask(
                "chat",
                &conv,
                &Request {
                    questions: (0..=MAX_QUESTIONS).map(|n| one(&format!("q{n}?"))).collect(),
                    ..Request::default()
                },
            )
            .expect_err("refused");
        assert!(err.to_string().contains(&MAX_QUESTIONS.to_string()), "{err}");

        let _ = std::fs::remove_dir_all(store.dir());
    }

    /// The rendered turn is all the next turn gets, and it is read by a model replaying a long
    /// transcript — so an answer that arrived alone would be attached to whichever question it
    /// thinks it asked. Each answer travels with its question.
    #[test]
    fn the_rendered_answer_pairs_every_reply_with_its_question() {
        let store = scratch("render");
        let conv = open_session(&store, "chat");
        let ask = store
            .ask(
                "chat",
                &conv,
                &Request {
                    questions: vec![one("which database?"), one("which region?")],
                    ..Request::default()
                },
            )
            .expect("ask");

        let text = ask.render(&human(&["Postgres", ""]));
        assert!(text.contains("which database?"), "{text}");
        assert!(text.contains("> Postgres"), "{text}");
        assert!(text.contains("which region?"), "{text}");
        assert!(
            text.contains("(no answer)"),
            "a question left blank says so rather than vanishing: {text}"
        );

        let by_default = ask.render(&Answer {
            by: AnsweredBy::Default,
            ..human(&["Postgres", "eu-west"])
        });
        assert!(
            by_default.contains("nobody answered in time"),
            "a default says it is one — it is read differently: {by_default}"
        );

        let _ = std::fs::remove_dir_all(store.dir());
    }

    /// Deleting the *agent* takes the questions nobody can answer any more, and leaves the settled
    /// ones where the transcript that explains them still is.
    #[test]
    fn deleting_an_agent_forgets_only_what_is_still_waiting() {
        let store = scratch("forget-agent");
        let conv = open_session(&store, "chat");
        let settled = store
            .ask(
                "chat",
                &conv,
                &Request {
                    questions: vec![one("already decided?")],
                    ..Request::default()
                },
            )
            .expect("ask");
        store
            .resolve_question("chat", &conv, Some(&settled.id), &human(&["yes"]))
            .expect("resolve");
        store
            .ask(
                "chat",
                &conv,
                &Request {
                    questions: vec![one("still open?")],
                    ..Request::default()
                },
            )
            .expect("ask");

        assert_eq!(store.forget_questions("chat"), 1, "one was still waiting");
        assert!(store.all_pending_questions().is_empty());
        assert_eq!(
            store.question_history("chat", &conv).len(),
            1,
            "the settled one stays with the transcript that explains it"
        );

        let _ = std::fs::remove_dir_all(store.dir());
    }

    /// Deleting a conversation takes its questions with it, so the inbox cannot offer to answer
    /// something that no longer exists.
    #[test]
    fn deleting_a_session_takes_its_questions() {
        let store = scratch("cascade");
        let conv = open_session(&store, "chat");
        store
            .ask(
                "chat",
                &conv,
                &Request {
                    questions: vec![one("still there?")],
                    ..Request::default()
                },
            )
            .expect("ask");
        assert_eq!(store.all_pending_questions().len(), 1);

        store.delete("chat", &conv).expect("delete");
        assert!(store.all_pending_questions().is_empty());

        let _ = std::fs::remove_dir_all(store.dir());
    }
}
