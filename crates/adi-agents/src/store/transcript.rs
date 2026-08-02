//! What a session said: `<id>.transcript.jsonl`, one JSON turn per line, oldest first.
//!
//! A runner never sees this. It answers one turn at a time and knows nothing about the ones before
//! it; what the session *is* — the sequence of questions and answers a reader scrolls through — is
//! durable, so it belongs here with the record and the queue.
//!
//! Append-only, and a line at a time rather than a rewritten array: a turn that has landed is final,
//! two writers appending never lose each other's line, and a crash mid-write costs the last line
//! instead of the file. That is also why a line that will not parse is skipped rather than failing
//! the read — one bad tail must not take a whole conversation out of the view.
//!
//! # Three kinds of turn, only one of them on disk
//!
//! What a reader renders is the persisted turns plus two things that exist nowhere but the view:
//!
//! - the **pending** answer, still being written — synthesized from the live content the caller
//!   hands in, so the answer streams into the view before it is committed;
//! - the **queued** questions, said but not yet asked — synthesized from the queue file, in the
//!   order they will be put.
//!
//! Neither is ever written down. A pending turn is by definition not final, and a queued message
//! becomes a real user turn only when its turn starts — persisting either would leave a duplicate
//! behind the moment it became real.
//!
//! # The store does not parse engine output
//!
//! The live content arrives as an already-parsed [`TurnContent`], produced by the runner whose
//! format it is. Nothing here knows what a `stream-json` event or an ADI loop event looks like, and
//! that is the whole point of the layering: adding an engine must not mean teaching the store a
//! wire format.

use std::io::{Read as _, Seek as _, SeekFrom, Write as _};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::memo;
use crate::progress::{Step, TurnContent, TurnMetrics};

/// The suffix of a session's transcript, as it reads *after* the session id — part of the same
/// `<id>.*` namespace every other file of the session lives in, so one prefix sweep still takes it.
pub(super) const TRANSCRIPT_SUFFIX: &str = "transcript.jsonl";

/// One message in a session's transcript.
///
/// Engine-agnostic on purpose: a turn is a role, some text, and what the engine reported doing to
/// produce it. Every backend's output is normalized into this shape by its own runner, so a reader
/// renders a `process:*` run and an `adi` conversation through one type.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Turn {
    /// `"user"` or `"assistant"`.
    pub role: String,
    pub text: String,
    /// Unix milliseconds the turn was recorded.
    #[serde(default)]
    pub at: u64,
    /// True only for the provisional, still-streaming assistant turn synthesized from the live log —
    /// never written to disk (a committed turn is settled and final).
    #[serde(default, skip_serializing_if = "is_false")]
    pub pending: bool,
    /// True only for a user message still waiting in the queue — said, but not yet asked. Synthesized
    /// from the queue file on read; it becomes a real transcript turn when its turn starts.
    #[serde(default, skip_serializing_if = "is_false")]
    pub queued: bool,
    /// The assistant turn's activity — tool calls and thinking — parsed from the engine's output.
    /// Empty for user turns and for engines that emit no structured progress.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub steps: Vec<Step>,
    /// The assistant turn's telemetry (tokens / cost / duration), when the engine reports it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metrics: Option<TurnMetrics>,
}

#[allow(clippy::trivially_copy_pass_by_ref)]
fn is_false(b: &bool) -> bool {
    !*b
}

/// The two roles a turn carries. Strings rather than an enum because [`Turn::role`] is one, and it
/// crosses the wire to readers that already expect these exact words.
pub(crate) const ROLE_USER: &str = "user";
pub(crate) const ROLE_ASSISTANT: &str = "assistant";

pub(super) fn path(dir: &Path, id: &str) -> PathBuf {
    dir.join(format!("{id}.{TRANSCRIPT_SUFFIX}"))
}

/// A question, ready to append. What the store records when a message is actually put to the agent —
/// not when it was typed, which is what the queue is for.
#[must_use]
pub fn user_turn(text: impl Into<String>) -> Turn {
    Turn {
        role: ROLE_USER.to_string(),
        text: text.into(),
        at: now_ms(),
        pending: false,
        queued: false,
        steps: Vec::new(),
        metrics: None,
    }
}

/// A finished answer, ready to append: its text, the timeline of what it did to get there, and
/// whatever telemetry the engine reported.
#[must_use]
pub fn assistant_turn(content: &TurnContent) -> Turn {
    Turn {
        role: ROLE_ASSISTANT.to_string(),
        text: content.text.clone(),
        at: now_ms(),
        pending: false,
        queued: false,
        steps: content.steps.clone(),
        metrics: content.metrics.clone(),
    }
}

/// Append one turn as a JSON line.
///
/// The two view-only flags are cleared on the way in. A caller holding a turn it took *from* a view
/// (the pending answer it has just watched finish, say) would otherwise commit it still flagged, and
/// a persisted `pending` turn is a contradiction that never settles: every later read would splice a
/// second pending answer in beside it. The recorded moment is filled in for the same reason — a turn
/// on disk always says when it landed, whether or not its author remembered to.
///
/// # Errors
/// Returns the write error. Unlike the queue, a turn that could not be written down is worth failing
/// over: the transcript is the only record that this was ever said.
pub(super) fn append(dir: &Path, id: &str, mut turn: Turn) -> Result<()> {
    turn.pending = false;
    turn.queued = false;
    if turn.at == 0 {
        turn.at = now_ms();
    }
    let mut line = serde_json::to_string(&turn)
        .map_err(|e| Error::Session(format!("couldn't encode a turn of session {id}: {e}")))?;
    line.push('\n');
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path(dir, id))?;
    file.write_all(line.as_bytes())?;
    Ok(())
}

/// The persisted turns, oldest first.
///
/// Memoized on the file's identity ([`crate::memo`]): an open chat asks for this twice a second, and
/// re-deserializing a conversation that cannot have changed since the last poll is pure waste. The
/// file only ever grows, so any new turn moves the stamp.
pub(super) fn load(dir: &Path, id: &str) -> Arc<Vec<Turn>> {
    memo::transcript(&path(dir, id))
}

/// Where the search for the last line starts, doubling from there until it finds one.
///
/// Big enough that an ordinary turn is found in the first read, small enough that a listing walking
/// a hundred sessions is not reading a hundred megabytes to look at a hundred numbers.
const FIRST_WINDOW: u64 = 64 * 1024;

/// Just the moment off a turn's line. A listing asks when a conversation last spoke, not what it
/// said, and everything else on the line — a whole answer, and every tool call that produced it —
/// is skipped rather than built.
#[derive(Deserialize)]
struct At {
    #[serde(default)]
    at: u64,
}

/// When the last turn on this transcript was recorded, or `None` when there is no turn to read one
/// from (no file, an empty one, or a last line torn by a crash mid-append).
///
/// The tail alone, never [`load`]: this answers a *listing*, which asks it of every session an agent
/// has, where `load` is asked of the one conversation on screen. Its memo holds a handful of files;
/// reading whole transcripts here would parse every megabyte of every conversation on every poll and
/// evict the open chat's parse while doing it. Memoized in its own right, so a session that has said
/// nothing since the last poll costs a `stat`.
pub(super) fn last_at(dir: &Path, id: &str) -> Option<u64> {
    let path = path(dir, id);
    memo::last_turn_at(&path, || {
        let line = last_line(&path)?;
        serde_json::from_str::<At>(&line).ok().map(|turn| turn.at)
    })
}

/// The last whole line of `path`, or `None` when it has none.
///
/// Read backwards in windows that double until one holds a line break, rather than by reading the
/// file: a transcript is every turn of a conversation and runs to tens of megabytes, while what is
/// wanted is its last line. The doubling is why there is no fixed window — a single turn carries the
/// timeline that produced it, tool output and all, so real answers of a quarter-megabyte are
/// ordinary and any constant would eventually answer with the turn *before* the last one.
fn last_line(path: &Path) -> Option<String> {
    let mut file = std::fs::File::open(path).ok()?;
    let len = file.metadata().ok()?.len();
    let mut window = FIRST_WINDOW;
    loop {
        let from = len.saturating_sub(window);
        file.seek(SeekFrom::Start(from)).ok()?;
        let mut tail = Vec::new();
        file.read_to_end(&mut tail).ok()?;
        // Lossy is safe: a window that starts mid-file starts mid-line, and everything before the
        // last break — which is where any character it cut in half would be — is dropped here.
        let text = String::from_utf8_lossy(&tail);
        let text = text.trim_end();
        if let Some(end) = text.rfind('\n') {
            return Some(text[end + 1..].to_string());
        }
        if from == 0 {
            // No break anywhere in the file: it is a single line, and that line is the last turn.
            return (!text.is_empty()).then(|| text.to_string());
        }
        window *= 2;
    }
}

/// The full view: what was persisted, what is being said right now, and what is still waiting.
///
/// `live` is spliced in **only behind an unanswered question**. A reader polls, so it hands in the
/// current parse of the in-flight answer on every call — including the ones after that answer has
/// been committed, which it has no way to notice. Anchoring on the last persisted turn means the
/// same content shows once as a pending answer and then once as a settled one, never as both.
///
/// `running` decides only the flag, not the splice: an answer whose child has exited but which
/// nobody has committed yet is still the truest thing to show for that question — it just is not
/// streaming any more.
pub(super) fn view(
    dir: &Path,
    id: &str,
    live: Option<TurnContent>,
    running: bool,
    queued: Vec<String>,
) -> Vec<Turn> {
    let mut turns = (*load(dir, id)).clone();
    if let Some(content) = live
        && turns.last().map(|t| t.role.as_str()) == Some(ROLE_USER)
    {
        turns.push(Turn {
            role: ROLE_ASSISTANT.to_string(),
            text: content.text,
            // No recorded moment: nothing has been recorded. It gets one when it is appended.
            at: 0,
            pending: running,
            queued: false,
            steps: content.steps,
            metrics: content.metrics,
        });
    }
    turns.extend(queued.into_iter().map(|text| Turn {
        role: ROLE_USER.to_string(),
        text,
        at: 0,
        pending: false,
        queued: true,
        steps: Vec::new(),
        metrics: None,
    }));
    turns
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::progress::{Step, ToolStatus, TurnMetrics};

    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "adi-store-transcript-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id(),
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// A turn that used tools survives the jsonl round-trip whole — the timeline in order, the
    /// metrics with it — which is what makes a reload show the same conversation as the poll before.
    #[test]
    fn turns_round_trip_through_the_jsonl_in_the_order_they_were_appended() {
        let dir = scratch("roundtrip");
        let id = "0000000000001-0000";
        assert!(load(&dir, id).is_empty(), "no file is an empty transcript");

        append(&dir, id, user_turn("set the port")).expect("append");
        let content = TurnContent {
            text: "Moved it to 81.".into(),
            steps: vec![
                Step::Message {
                    text: "Checking the config.".into(),
                },
                Step::Tool {
                    name: "Read".into(),
                    input: "{}".into(),
                    status: ToolStatus::Ok,
                    output: "port = 80".into(),
                },
            ],
            metrics: Some(TurnMetrics {
                duration_ms: Some(12),
                ..TurnMetrics::default()
            }),
        };
        append(&dir, id, assistant_turn(&content)).expect("append");

        let turns = load(&dir, id);
        assert_eq!(turns.len(), 2);
        assert_eq!(turns[0].role, ROLE_USER);
        assert_eq!(turns[0].text, "set the port");
        assert!(turns[0].at > 1_600_000_000_000, "a persisted turn is dated");
        assert!(turns[0].steps.is_empty());
        assert_eq!(turns[1].role, ROLE_ASSISTANT);
        assert_eq!(turns[1].text, "Moved it to 81.");
        assert_eq!(turns[1].steps, content.steps, "the timeline stays in order");
        assert_eq!(turns[1].metrics, content.metrics);

        // The file really is one line per turn, appended — not a rewritten array.
        let text = std::fs::read_to_string(path(&dir, id)).unwrap();
        assert_eq!(text.lines().count(), 2);

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The flags that only exist in a view must never reach the file. A caller committing the very
    /// turn it read back from [`view`] is the case that matters: left flagged, the answer would stay
    /// "still being written" for ever and every later read would splice a second one in beside it.
    #[test]
    fn a_committed_turn_is_never_pending_or_queued_and_is_always_dated() {
        let dir = scratch("flags");
        let id = "0000000000002-0000";
        append(
            &dir,
            id,
            Turn {
                role: ROLE_ASSISTANT.to_string(),
                text: "half an answer".into(),
                at: 0,
                pending: true,
                queued: true,
                steps: Vec::new(),
                metrics: None,
            },
        )
        .expect("append");

        let turns = load(&dir, id);
        assert_eq!(turns.len(), 1);
        assert!(!turns[0].pending, "a turn on disk is settled by definition");
        assert!(!turns[0].queued, "and it has been asked, so it is not waiting");
        assert!(turns[0].at > 1_600_000_000_000, "an undated turn is dated on the way in");

        // A recorded moment the caller *did* set is left alone — a migration keeps the old one.
        append(
            &dir,
            id,
            Turn {
                at: 42,
                ..user_turn("from before")
            },
        )
        .expect("append");
        assert_eq!(load(&dir, id)[1].at, 42);

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A truncated tail (a crash mid-append) costs its own line and nothing else.
    #[test]
    fn an_unparseable_line_is_skipped_rather_than_failing_the_read() {
        let dir = scratch("torn");
        let id = "0000000000003-0000";
        append(&dir, id, user_turn("first")).expect("append");
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(path(&dir, id))
            .unwrap();
        file.write_all(b"{\"role\":\"assist\n\n").unwrap();
        drop(file);
        append(&dir, id, user_turn("after the tear")).expect("append");

        let turns = load(&dir, id);
        assert_eq!(turns.len(), 2, "the torn line is skipped, the rest reads");
        assert_eq!(turns[1].text, "after the tear");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// What a listing reads: the last turn's moment, off the tail of the file rather than out of a
    /// parse of the whole conversation — including when that turn is far bigger than the window the
    /// search starts at, which is the ordinary case for an answer that used tools.
    #[test]
    fn the_last_moment_is_read_off_the_tail_however_big_the_turn() {
        let dir = scratch("last-at");
        let id = "0000000000007-0000";
        assert!(last_at(&dir, id).is_none(), "no transcript, no moment");

        append(&dir, id, Turn { at: 1_000, ..user_turn("first") }).expect("append");
        assert_eq!(last_at(&dir, id), Some(1_000), "the only line is the last one");
        append(&dir, id, Turn { at: 2_000, ..user_turn("second") }).expect("append");
        assert_eq!(last_at(&dir, id), Some(2_000), "and later lines win");

        // A turn whose own line dwarfs the first window: the search widens to it rather than
        // answering with the turn before, which is what a fixed window would do.
        let huge = "x".repeat(usize::try_from(FIRST_WINDOW).expect("fits") * 3);
        append(&dir, id, Turn { at: 3_000, ..user_turn(huge) }).expect("append");
        assert_eq!(last_at(&dir, id), Some(3_000), "the window grew to the line");

        // A crash mid-append leaves a torn last line; that is no moment at all, and is said so
        // rather than guessed at — a caller sorts by this.
        let torn = scratch("last-at-torn");
        std::fs::write(path(&torn, id), "{\"role\":\"user\",\"at\":4\n").expect("write");
        assert!(last_at(&torn, id).is_none());

        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&torn);
    }

    /// The whole view in one place: what is on disk, the answer being written, and the questions
    /// still waiting — and the flags that tell a reader which is which.
    #[test]
    fn the_view_streams_the_live_answer_and_trails_the_queue() {
        let dir = scratch("view");
        let id = "0000000000004-0000";
        append(&dir, id, user_turn("first")).expect("append");

        let live = || TurnContent {
            text: "partial answer so far".into(),
            steps: vec![Step::Tool {
                name: "Read".into(),
                input: String::new(),
                status: ToolStatus::Running,
                output: String::new(),
            }],
            metrics: None,
        };
        let queued = vec!["second".to_string(), "third".to_string()];
        let turns = view(&dir, id, Some(live()), true, queued.clone());

        assert_eq!(turns.len(), 4);
        assert_eq!(turns[1].role, ROLE_ASSISTANT);
        assert!(turns[1].pending, "the answer is still being written");
        assert_eq!(turns[1].text, "partial answer so far");
        assert_eq!(turns[1].steps.len(), 1, "its tools show while they run");
        assert_eq!(
            turns[2..].iter().map(|t| t.text.as_str()).collect::<Vec<_>>(),
            ["second", "third"],
            "the queue trails it in the order it will be asked",
        );
        assert!(turns[2..].iter().all(|t| t.queued && !t.pending && t.role == ROLE_USER));
        assert!(
            turns.iter().all(|t| !(t.queued && t.pending)),
            "a queued message is not a streaming answer",
        );

        // None of it was written down: the file still holds only the question.
        assert_eq!(load(&dir, id).len(), 1);

        // The child has exited but nobody has committed yet: still the truest answer to show for
        // that question, just no longer streaming.
        let settling = view(&dir, id, Some(live()), false, Vec::new());
        assert_eq!(settling.len(), 2);
        assert!(!settling[1].pending);
        assert_eq!(settling[1].text, "partial answer so far");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The anchor that keeps a polling reader honest: once the answer is committed, the live content
    /// it is still handing in is the *same* answer, and must not show twice.
    #[test]
    fn live_content_behind_a_settled_answer_is_not_spliced_in_again() {
        let dir = scratch("settled");
        let id = "0000000000005-0000";
        let content = TurnContent {
            text: "the answer".into(),
            steps: Vec::new(),
            metrics: None,
        };
        append(&dir, id, user_turn("a question")).expect("append");
        append(&dir, id, assistant_turn(&content)).expect("append");

        let turns = view(&dir, id, Some(content.clone()), false, Vec::new());
        assert_eq!(turns.len(), 2, "the committed answer, once");
        assert!(!turns[1].pending);

        // An empty transcript has no question to answer either, so there is nothing to stream into.
        let empty = scratch("settled-empty");
        assert!(view(&empty, "0000000000006-0000", Some(content), true, Vec::new()).is_empty());

        // …but a queue alone still shows, so a message typed before the first turn ran is visible.
        let waiting = view(
            &empty,
            "0000000000006-0000",
            None,
            false,
            vec!["typed early".to_string()],
        );
        assert_eq!(waiting.len(), 1);
        assert!(waiting[0].queued);

        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&empty);
    }
}
