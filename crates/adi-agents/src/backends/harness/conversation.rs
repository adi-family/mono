//! Harness *conversations*: a run you can answer.
//!
//! A conversation reuses the detached-run slot (`<sessions>/harness/<agent>/<conv_id>.{pid,log,json}`)
//! and adds an append-only transcript, `<conv_id>.jsonl`, of user/assistant turns. The first turn is
//! the initial task; each [`reply`] spawns another detached child that continues the *same* thread and
//! prints its answer, one turn at a time. That gives the harness an in/out channel — send a message,
//! read the answer, reply again — instead of a single fire-and-forget `--print` run.
//!
//! Continuation state differs per engine but the conversation machinery does not:
//! - `harness:claude-sdk` mints a session id up front (`--session-id`) and resumes it on each reply
//!   (`--resume`), so the Claude CLI reconstructs the full history itself.
//! - `harness:adi` (future) replays the transcript into its own loop.
//!
//! An answer is captured as the turn child's stdout in `<conv_id>.log`. It is folded into the
//! transcript lazily, on the next read after the child exits ([`settle`]) — so a mid-turn app-server
//! restart never loses the last answer, and no reaper bookkeeping is needed beyond dropping the PID.

use std::io::{Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::Backend;
use crate::backends::detached;
use crate::error::{Error, Result};
use crate::progress::{self, Step, TurnMetrics};
use crate::run::Launch;
use crate::{StoredAgent, StoredAgentManifest};

use super::{HARNESS_DIR, engine_supported, engine_turn};

const ROLE_USER: &str = "user";
const ROLE_ASSISTANT: &str = "assistant";

/// One message in a conversation's transcript.
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
    /// The assistant turn's activity — tool calls and thinking — parsed from the engine's output.
    /// Empty for user turns and for engines that emit no structured progress.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub steps: Vec<Step>,
    /// The assistant turn's telemetry (tokens / cost / duration), when the engine reports it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metrics: Option<TurnMetrics>,
}

/// Which continuation flag a turn's engine command carries.
pub(super) enum Continuation<'a> {
    /// The conversation's first turn — establish the session under this id.
    First { session_id: &'a str },
    /// A follow-up — resume the established session.
    Resume { session_id: &'a str },
}

/// Start a new conversation with `message` as its first turn. Returns the launch of the first
/// answering child (its `run_id` is the conversation id, used to [`reply`] to it later).
pub(crate) fn start(
    agent: &StoredAgent,
    sessions_dir: &Path,
    base_dir: &Path,
    bin_dir: Option<&Path>,
    message: &str,
    secret_env: &[(String, String)],
) -> Result<Launch> {
    // Validate the backend before touching disk, so an unrunnable/unconfigured harness fails fast
    // without leaving a stray conversation behind.
    engine_supported(&agent.manifest)?;
    // The engine's continuation handle. `claude-sdk` needs a session id it can resume; `adi` replays
    // the transcript, so it carries none.
    let session_id = new_session_id(&agent.manifest);

    let dir = detached::agent_dir(sessions_dir, HARNESS_DIR, &agent.name);
    std::fs::create_dir_all(&dir)?;
    let conv_id = detached::new_run_id();
    // The turn command needs the conversation id (the `adi` loop replays that conversation).
    let (argv, working_dir) = engine_turn(
        agent,
        &conv_id,
        message,
        Continuation::First {
            session_id: &session_id,
        },
    )?;
    // Metadata sidecar: the run list reads `message` (the conversation title) and `started_at`; the
    // session id is what a reply resumes.
    let meta = serde_json::json!({
        "started_at": detached::started_at(&conv_id),
        "message": message,
        "session_id": session_id,
    });
    let _ = std::fs::write(detached::meta_path(&dir, &conv_id), meta.to_string());
    append_turn(&dir, &conv_id, ROLE_USER, message);

    let launch = spawn_turn(&dir, &conv_id, base_dir, bin_dir, &argv, working_dir, secret_env)?;
    detached::prune_old_runs(&dir);
    Ok(launch)
}

/// Answer into an existing conversation: append `message` as the next user turn and spawn a child
/// that continues the thread. Rejected while the previous answer is still being produced — one turn
/// runs at a time.
pub(crate) fn reply(
    agent: &StoredAgent,
    sessions_dir: &Path,
    base_dir: &Path,
    bin_dir: Option<&Path>,
    conv_id: &str,
    message: &str,
    secret_env: &[(String, String)],
) -> Result<Launch> {
    let dir = detached::agent_dir(sessions_dir, HARNESS_DIR, &agent.name);
    if !detached::meta_path(&dir, conv_id).exists() {
        return Err(Error::NotFound(format!("{}: no conversation {conv_id}", agent.name)));
    }
    if turn_running(&dir, conv_id) {
        return Err(Error::Busy(format!(
            "“{}” is still answering — wait for the current reply to finish before sending another.",
            agent.name
        )));
    }
    // Commit the previous turn's captured answer before appending the new question, so the
    // transcript stays a clean user/assistant/user/assistant sequence and the live log can be reused.
    settle(&dir, conv_id, &agent.manifest.backend);

    let session_id = read_session_id(&dir, conv_id);
    // Build the command before appending the question, so a failure doesn't strand a dangling
    // unanswered user turn in the transcript.
    let (argv, working_dir) = engine_turn(
        agent,
        conv_id,
        message,
        Continuation::Resume {
            session_id: &session_id,
        },
    )?;
    append_turn(&dir, conv_id, ROLE_USER, message);
    spawn_turn(&dir, conv_id, base_dir, bin_dir, &argv, working_dir, secret_env)
}

/// The conversation's transcript, oldest first. Folds a just-finished turn's captured stdout into a
/// committed assistant turn, and — while a turn is still in flight — appends a provisional assistant
/// turn from the live log so the answer streams into the view before it settles.
pub(crate) fn transcript(
    sessions_dir: &Path,
    agent_name: &str,
    conv_id: &str,
    backend: &Backend,
) -> Vec<Turn> {
    let dir = detached::agent_dir(sessions_dir, HARNESS_DIR, agent_name);
    settle(&dir, conv_id, backend);
    let mut turns = load_transcript(&dir, conv_id);
    // A user turn with no committed answer and a live child → stream the partial answer, parsing the
    // partial event log so its tool steps appear (running) before the turn settles.
    if turns.last().map(|t| t.role.as_str()) == Some(ROLE_USER) && turn_running(&dir, conv_id) {
        let content = progress::parse(backend, &read_log_bytes(&dir, conv_id));
        turns.push(Turn {
            role: ROLE_ASSISTANT.to_string(),
            text: content.text,
            at: 0,
            pending: true,
            steps: content.steps,
            metrics: content.metrics,
        });
    }
    turns
}

/// The committed transcript only — no settling, no in-flight streaming turn. This is what the `adi`
/// loop replays: called from *within* the running turn child, where the pending-aware [`transcript`]
/// would splice in this very turn's empty partial answer.
pub(super) fn committed(sessions_dir: &Path, agent_name: &str, conv_id: &str) -> Vec<Turn> {
    let dir = detached::agent_dir(sessions_dir, HARNESS_DIR, agent_name);
    load_transcript(&dir, conv_id)
}

// ---- turn spawning -----------------------------------------------------------------

/// Spawn a turn's answering child into the conversation's slot and wrap it as a [`Launch`].
fn spawn_turn(
    dir: &Path,
    conv_id: &str,
    base_dir: &Path,
    bin_dir: Option<&Path>,
    argv: &[String],
    working_dir: Option<String>,
    secret_env: &[(String, String)],
) -> Result<Launch> {
    let log = detached::log_path_in(dir, conv_id);
    let pid = detached::spawn_child(
        dir,
        conv_id,
        &log,
        base_dir,
        bin_dir,
        argv,
        working_dir.as_deref(),
        secret_env,
    )?;
    Ok(Launch::Process {
        command: detached::display_command(argv),
        pid,
        log,
        run_id: conv_id.to_string(),
    })
}

/// A conversation's continuation handle: a fresh session id for the Claude CLI to establish and
/// resume; empty for engines that replay the transcript instead.
fn new_session_id(manifest: &StoredAgentManifest) -> String {
    match manifest.backend {
        Backend::HarnessClaudeSdk => Uuid::new_v4().to_string(),
        _ => String::new(),
    }
}

/// Whether this conversation's current turn child is still alive.
fn turn_running(dir: &Path, conv_id: &str) -> bool {
    detached::read_pid(&detached::pid_path_in(dir, conv_id)).is_some_and(detached::pid_alive)
}

// ---- transcript persistence --------------------------------------------------------

fn transcript_path(dir: &Path, conv_id: &str) -> PathBuf {
    dir.join(format!("{conv_id}.jsonl"))
}

/// Append a plain (user) turn — text only, no steps or metrics.
fn append_turn(dir: &Path, conv_id: &str, role: &str, text: &str) {
    write_turn(
        dir,
        conv_id,
        &Turn {
            role: role.to_string(),
            text: text.to_string(),
            at: now_ms(),
            pending: false,
            steps: Vec::new(),
            metrics: None,
        },
    );
}

/// Commit a finished assistant turn parsed from its output — the answer text plus any tool/thinking
/// steps and telemetry — as one transcript line.
fn append_assistant(dir: &Path, conv_id: &str, content: &progress::TurnContent) {
    write_turn(
        dir,
        conv_id,
        &Turn {
            role: ROLE_ASSISTANT.to_string(),
            text: content.text.clone(),
            at: now_ms(),
            pending: false,
            steps: content.steps.clone(),
            metrics: content.metrics.clone(),
        },
    );
}

/// Append one turn as a JSON line to the transcript.
fn write_turn(dir: &Path, conv_id: &str, turn: &Turn) {
    let Ok(mut line) = serde_json::to_string(turn) else {
        return;
    };
    line.push('\n');
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(transcript_path(dir, conv_id))
    {
        let _ = file.write_all(line.as_bytes());
    }
}

/// The committed transcript, oldest first. Unparseable lines are skipped rather than failing.
fn load_transcript(dir: &Path, conv_id: &str) -> Vec<Turn> {
    let Ok(text) = std::fs::read_to_string(transcript_path(dir, conv_id)) else {
        return Vec::new();
    };
    text.lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|l| serde_json::from_str::<Turn>(l).ok())
        .collect()
}

/// If the last turn is an unanswered question and the child has exited, parse its captured output
/// (per the backend's engine format) into a committed assistant turn. A no-op while a turn is running
/// or when the last turn is already an answer, so it is safe to call before every read.
fn settle(dir: &Path, conv_id: &str, backend: &Backend) {
    let turns = load_transcript(dir, conv_id);
    if turns.last().map(|t| t.role.as_str()) != Some(ROLE_USER) {
        return;
    }
    if turn_running(dir, conv_id) {
        return;
    }
    let content = progress::parse(backend, &read_log_bytes(dir, conv_id));
    append_assistant(dir, conv_id, &content);
}

/// The turn child's captured output, read whole from the start up to the parse cap. Reading from the
/// start (not a tail) keeps the beginning of a streamed event log, so early tool steps survive.
fn read_log_bytes(dir: &Path, conv_id: &str) -> Vec<u8> {
    let path = detached::log_path_in(dir, conv_id);
    let Ok(file) = std::fs::File::open(&path) else {
        return Vec::new();
    };
    let mut buf = Vec::new();
    let _ = file.take(progress::MAX_PARSE_BYTES).read_to_end(&mut buf);
    buf
}

// ---- meta ------------------------------------------------------------------------

/// The continuation handle recorded for a conversation, or empty when absent.
fn read_session_id(dir: &Path, conv_id: &str) -> String {
    let Ok(text) = std::fs::read_to_string(detached::meta_path(dir, conv_id)) else {
        return String::new();
    };
    serde_json::from_str::<serde_json::Value>(&text)
        .ok()
        .and_then(|v| {
            v.get("session_id")
                .and_then(serde_json::Value::as_str)
                .map(ToString::to_string)
        })
        .unwrap_or_default()
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

#[allow(clippy::trivially_copy_pass_by_ref)]
fn is_false(b: &bool) -> bool {
    !*b
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::*;

    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "adi-conv-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    /// Lay down a conversation's meta so it reads as existing, and return its agent dir.
    fn seed(sessions: &Path, agent: &str, conv: &str) -> PathBuf {
        let dir = detached::agent_dir(sessions, HARNESS_DIR, agent);
        std::fs::create_dir_all(&dir).unwrap();
        let meta = serde_json::json!({
            "started_at": detached::started_at(conv),
            "message": "hi",
            "session_id": "sid",
        });
        std::fs::write(detached::meta_path(&dir, conv), meta.to_string()).unwrap();
        dir
    }

    #[test]
    fn a_finished_turn_is_folded_into_a_committed_assistant_answer() {
        let sessions = scratch("fold");
        let conv = "0000000000001-0000";
        let dir = seed(&sessions, "chat", conv);
        append_turn(&dir, conv, ROLE_USER, "hello");
        // The turn child's captured stdout, with surrounding whitespace to be trimmed.
        std::fs::write(detached::log_path_in(&dir, conv), "  hi there  \n").unwrap();

        // No pid file → the turn has finished; its stdout folds into a committed assistant turn.
        // A plain-text log (no stream-json events) parses as text-only for any claude backend.
        let backend = Backend::HarnessClaudeSdk;
        let turns = transcript(&sessions, "chat", conv, &backend);
        assert_eq!(turns.len(), 2);
        assert_eq!(turns[0].role, ROLE_USER);
        assert_eq!(turns[0].text, "hello");
        assert_eq!(turns[1].role, ROLE_ASSISTANT);
        assert_eq!(turns[1].text, "hi there");
        assert!(!turns[1].pending);
        // Idempotent: re-reading doesn't fold a second time.
        assert_eq!(transcript(&sessions, "chat", conv, &backend).len(), 2);
        assert_eq!(load_transcript(&dir, conv).len(), 2);

        let _ = std::fs::remove_dir_all(&sessions);
    }

    /// End-to-end: a real `harness:claude-sdk` conversation must carry context across a reply — the
    /// whole point of being answerable. Ignored by default: it spawns the real `claude` CLI and hits
    /// the network. Run with `cargo test -p adi-agents -- --ignored claude_sdk_conversation`.
    #[test]
    #[ignore = "spawns the real `claude` CLI and hits the network"]
    fn claude_sdk_conversation_remembers_across_a_reply() {
        let tmp = std::env::temp_dir().join(format!("adi-harness-it-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let store = crate::Agents::with_config(adi_config::Config::with_root(&tmp));
        let spec = crate::AgentManifest {
            backend: "harness:claude-sdk".into(),
            arguments: crate::arguments::HarnessClaudeSdkArguments {
                model: Some("haiku".into()),
                ..Default::default()
            },
            ..Default::default()
        };
        store.save("chatty", spec).expect("save");
        let agent = store.get("chatty").expect("get").expect("exists");

        // Turn 1: establish the session and plant a codeword.
        let launch = store
            .run_with_message(
                "chatty",
                "Remember this codeword: PLATYPUS-42. Reply with only 'ok'.",
            )
            .expect("start");
        let conv_id = match launch {
            Launch::Process { run_id, .. } => run_id,
            other => panic!("expected a process launch, got {other:?}"),
        };
        wait_until_answered(&store, &agent, &conv_id);
        let t1 = store.transcript(&agent, &conv_id);
        assert_eq!(t1.len(), 2, "one question, one answer");
        assert_eq!(t1[0].role, ROLE_USER);
        assert_eq!(t1[1].role, ROLE_ASSISTANT);

        // Turn 2: resume the same conversation and ask for the codeword back.
        store
            .reply(
                "chatty",
                &conv_id,
                "What was the codeword I told you? Reply with only the codeword.",
            )
            .expect("reply");
        wait_until_answered(&store, &agent, &conv_id);
        let t2 = store.transcript(&agent, &conv_id);
        assert_eq!(t2.len(), 4, "two questions, two answers");
        assert!(
            t2[3].text.contains("PLATYPUS-42"),
            "the reply must recall the codeword from the first turn, got: {:?}",
            t2[3].text
        );

        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// Poll until the conversation's latest turn has settled into a committed assistant answer.
    fn wait_until_answered(store: &crate::Agents, agent: &StoredAgent, conv_id: &str) {
        let deadline = Instant::now() + Duration::from_secs(90);
        loop {
            let turns = store.transcript(agent, conv_id);
            let settled = turns
                .last()
                .is_some_and(|t| t.role == ROLE_ASSISTANT && !t.pending);
            if settled {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "turn did not settle in time; transcript so far: {turns:?}"
            );
            std::thread::sleep(Duration::from_millis(200));
        }
    }

    #[test]
    fn an_in_flight_turn_streams_a_pending_answer_without_committing() {
        let sessions = scratch("pending");
        let conv = "0000000000002-0000";
        let dir = seed(&sessions, "chat", conv);
        append_turn(&dir, conv, ROLE_USER, "hello");
        std::fs::write(detached::log_path_in(&dir, conv), "partial answer so far").unwrap();
        // A live pid (our own process) marks the turn as still answering.
        std::fs::write(
            detached::pid_path_in(&dir, conv),
            format!("{}\n", std::process::id()),
        )
        .unwrap();

        let turns = transcript(&sessions, "chat", conv, &Backend::HarnessClaudeSdk);
        assert_eq!(turns.len(), 2);
        assert!(turns[1].pending);
        assert_eq!(turns[1].text, "partial answer so far");
        // Nothing committed while it streams: the transcript file still holds only the question.
        assert_eq!(load_transcript(&dir, conv).len(), 1);

        let _ = std::fs::remove_dir_all(&sessions);
    }
}
