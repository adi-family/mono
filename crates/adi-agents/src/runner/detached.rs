//! The detached-child runner: a vendor CLI or the ADI loop run headless as its own process group.
//!
//! One instance per engine. `DetachedRunner::new(Backend::ProcessClaude)` and
//! `DetachedRunner::new(Backend::HarnessAdi)` are two runners rather than one runner with a flag,
//! because everything that differs between them — which argv to build, which wire format the log is
//! in, what it can report — is decided once here instead of re-matched at every call site. That is
//! the whole point of the [`Runner`] trait: the caller holds a `dyn Runner` and never learns which
//! engine it got.
//!
//! Nothing durable lives here. The child's pid (and, for the Claude CLI, the engine session id it
//! resumes) go in the session's [state slot](Session::state); the log is the store's file, opened by
//! path because a spawned child needs a file descriptor. Everything else — the record, the queue,
//! the transcript — belongs to the store and this module never sees it.

use std::fs::File;
use std::io::{Read as _, Seek as _, SeekFrom};
use std::path::Path;
use std::time::{Duration, Instant};

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::arguments::{
    HarnessAdiArguments, HarnessClaudeSdkArguments, ProcessClaudeArguments, ProcessCodexArguments,
};
use crate::backend::Backend;
use crate::backends::detached::Spawned;
use crate::backends::harness::claude_sdk::Continuation;
use crate::backends::{adi_events, claude_stream, detached, harness, process};
use crate::error::{Error, Result};
use crate::progress::{self, MAX_PARSE_BYTES, TurnContent};
use crate::runner::prompt::{compose, own_prompt, with_knowledge, with_tool_help, with_workspace};
use crate::runner::{
    EventBatch, EventKinds, ImageDelivery, RunEvent, RunSpec, Runner, RunnerKind, Session,
    StateWriter, Stopped,
};

/// How often a stopping run is re-probed while its grace period runs down. Short enough that a
/// cooperative CLI's exit is noticed almost immediately, long enough not to spin.
const POLL: Duration = Duration::from_millis(25);

/// How long a KILL is given to land before [`Runner::stop`] returns. Not a second grace period —
/// the process is already condemned — just enough that a caller asking [`Runner::is_alive`] right
/// after gets the truth rather than a corpse the kernel has not reaped yet.
const REAP: Duration = Duration::from_millis(500);

/// Where the composed system prompt is handed to the `harness:adi` turn child.
///
/// The Claude engines take their prompt on the command line, so the runner simply builds the flag.
/// The adi loop has no such flag: its turn is a re-entry into this binary that reads the agent and
/// the transcript back off disk, so the run's environment is the only channel the runner has into
/// it — and [`adi_loop::system_prompt`](crate::backends::harness::adi_loop) reads it from there.
pub(crate) const SYSTEM_PROMPT_ENV: &str = "ADI_SYSTEM_PROMPT";

/// A headless run of one engine: spawn a detached child, watch its pid, read its log.
#[derive(Debug, Clone)]
pub struct DetachedRunner {
    backend: Backend,
}

impl DetachedRunner {
    #[must_use]
    pub fn new(backend: Backend) -> Self {
        Self { backend }
    }

    /// This turn's command line.
    ///
    /// The continuation is derived here and nowhere else: `harness:claude-sdk` establishes its
    /// engine session on the first turn (`--session-id`) and resumes it afterwards (`--resume`),
    /// decided purely from [`Session::has_started`]. No caller ever passes a "fresh or resume"
    /// flag, which is why [`Continuation`] can stay a private detail of this file.
    ///
    /// The other engines have no such pair to derive: a `process:*` run is a one-shot `--print` /
    /// `exec` that keeps no session, and the adi loop continues by replaying the transcript, so it
    /// is addressed by agent + conversation id alone.
    fn argv(
        &self,
        spec: &RunSpec,
        session: &dyn Session,
        message: &str,
        session_id: &str,
    ) -> Result<Vec<String>> {
        match &self.backend {
            Backend::ProcessClaude => {
                let mut config = decode::<ProcessClaudeArguments>(&spec.arguments)?;
                config.system_prompt = own_prompt(spec, config.system_prompt);
                config.append_system_prompt =
                    with_tool_help(spec, with_knowledge(spec, with_workspace(spec, config.append_system_prompt)));
                let tools = crate::backends::mcp::scope_tools(config.allowed_tools.as_deref());
                Ok(process::claude::argv(
                    &config,
                    message,
                    Some(&crate::backends::mcp::config(spec, session)),
                    &tools,
                ))
            }
            Backend::ProcessCodex => {
                let mut config = decode::<ProcessCodexArguments>(&spec.arguments)?;
                // No tool help: Codex has no system-prompt flag, so this value is pushed as the
                // opening *user* prompt — help there would arrive as a question to answer, and an
                // agent with no prompt of its own would open by reading a wall of usage text.
                config.system_prompt = own_prompt(spec, config.system_prompt);
                Ok(process::codex::argv(&config, message, spec.cwd.to_str()))
            }
            Backend::HarnessClaudeSdk => {
                let mut config = decode::<HarnessClaudeSdkArguments>(&spec.arguments)?;
                config.system_prompt = own_prompt(spec, config.system_prompt);
                config.append_system_prompt =
                    with_tool_help(spec, with_knowledge(spec, with_workspace(spec, config.append_system_prompt)));
                let cont = if session.has_started() {
                    Continuation::Resume { session_id }
                } else {
                    Continuation::First { session_id }
                };
                let tools = crate::backends::mcp::scope_tools(config.allowed_tools.as_deref());
                Ok(harness::claude_sdk::argv(
                    &config,
                    message,
                    &cont,
                    Some(&crate::backends::mcp::config(spec, session)),
                    &tools,
                ))
            }
            Backend::HarnessAdi => {
                let config = decode::<HarnessAdiArguments>(&spec.arguments)?;
                harness::adi_loop::validate(&config)?;
                Ok(harness::adi_loop::argv(session.agent(), session.id()))
            }
            other => Err(Error::NotRunnable(other.to_string())),
        }
    }

    /// The engine session id this turn runs under: minted once for the Claude CLI and kept in the
    /// state slot so every later turn resumes the same thread; empty for every other engine, which
    /// has nothing to resume.
    fn engine_session_id(&self, state: &State) -> String {
        if !matches!(self.backend, Backend::HarnessClaudeSdk) {
            return String::new();
        }
        match state.session_id.as_deref().map(str::trim) {
            // A recorded id is resumable only if a turn actually established it; otherwise this is
            // still the first turn and the same id is established now.
            Some(id) if !id.is_empty() => id.to_string(),
            _ => Uuid::new_v4().to_string(),
        }
    }

    /// The child's environment: the spec's, plus the adi loop's prompt channel.
    ///
    /// The turn child prefers what is exported here over the prompt in its stored manifest, so the
    /// tool help composed at this layer is what the loop actually runs under.
    fn run_env(&self, spec: &RunSpec) -> Vec<(String, String)> {
        let mut env = spec.env.clone();
        if matches!(self.backend, Backend::HarnessAdi)
            && let Some(prompt) = compose(spec, None)
        {
            env.push((SYSTEM_PROMPT_ENV.to_string(), prompt));
        }
        env
    }

    /// This engine's log format, read into common content. A backend with no structured stream (or
    /// a log written by something that crashed before it could say anything) falls back to plain
    /// text, so this never fails — the worst case is "just the answer, no steps".
    fn parse(&self, log: &[u8]) -> TurnContent {
        match self.backend {
            Backend::ProcessClaude | Backend::HarnessClaudeSdk => claude_stream::parse(log),
            Backend::HarnessAdi => adi_events::parse(log),
            // UNIMPLEMENTED: `process:codex`. Codex emits a structured stream under `--json` and
            // nothing here reads it, so a Codex run's log arrives as plain text: the answer is
            // whole, the turn has no tool steps and no metrics. What it wants is a
            // `codex_stream::parse` beside `claude_stream::parse` and an arm above; the rest of
            // the runner is complete for that backend.
            //
            // Note what this does *not* line up with: `emits` below already claims `tool_call` and
            // `metrics` for `ProcessCodex`, so `crate::progress::capabilities` advertises steps a
            // reader will never be shown. The claim is about the engine, the gap is here — fixing
            // it is writing the parser, not lowering the flags.
            _ => TurnContent {
                text: progress::text_of(log),
                steps: Vec::new(),
                metrics: None,
            },
        }
    }
}

impl Runner for DetachedRunner {
    fn kind(&self) -> RunnerKind {
        RunnerKind::new("detached")
    }

    fn check(&self, spec: &RunSpec) -> Result<()> {
        match &self.backend {
            Backend::ProcessClaude => decode::<ProcessClaudeArguments>(&spec.arguments).map(drop),
            Backend::ProcessCodex => decode::<ProcessCodexArguments>(&spec.arguments).map(drop),
            Backend::HarnessClaudeSdk => {
                decode::<HarnessClaudeSdkArguments>(&spec.arguments).map(drop)
            }
            Backend::HarnessAdi => harness::adi_loop::validate(&decode(&spec.arguments)?),
            other => Err(Error::NotRunnable(other.to_string())),
        }
    }

    fn send(&self, spec: &RunSpec, session: &dyn Session, message: &str) -> Result<()> {
        let mut state = State::read(session);
        let session_id = self.engine_session_id(&state);
        // Build the command before spawning anything, so an unrunnable engine or a mistyped
        // argument fails with nothing started and nothing written.
        let argv = self.argv(spec, session, message, &session_id)?;

        let log = session.log_path();
        let dir = log.parent().unwrap_or_else(|| Path::new("."));
        let slot = log
            .file_stem()
            .and_then(|stem| stem.to_str())
            .ok_or_else(|| Error::Launch(format!("unusable log path {}", log.display())))?;
        // The reaper's half of the state slot, taken before the spawn so the thread has somewhere to
        // write the ending to the moment there is one.
        let writer = session.state_writer();
        let child = detached::spawn_child(
            dir,
            slot,
            log,
            &spec.cwd,
            &spec.path,
            &argv,
            &self.run_env(spec),
            move |child| {
                if let Some(writer) = writer {
                    forget_child(writer.as_ref(), child);
                }
            },
        )?;

        state.pid = Some(child.pid);
        state.started = child.started;
        state.session_id = (!session_id.is_empty()).then_some(session_id);
        session.set_state(state.into_value())
    }

    /// Whether this session's child is still running — asked of the pid **and** of the process's
    /// start time, never of the pid alone.
    ///
    /// A pid is a slot, not a name. Nothing rewrites the state slot when a turn ends (a one-shot
    /// run leaves it as it was; an app that was killed never got the chance), so a recorded pid
    /// routinely outlives its child by days — and the kernel reissues the number in the meantime.
    /// Asking `kill(pid, 0)` about it, which is all this used to do, then reports somebody else's
    /// process as this run: two finished runs sat "running" for three days on the strength of a
    /// browser tab and an audio helper that had inherited their numbers. A run stuck that way holds
    /// a concurrency slot nothing can free and swallows every reply into a queue no turn will ever
    /// drain, because the turn it is waiting behind finished last week.
    ///
    /// Records written before start times were kept have only the pid, and cannot be verified that
    /// way. They are held to the one thing that is still checkable: a process that started *after*
    /// the run's log was last written cannot be the child that holds that log open, since the log
    /// is created immediately before the child that writes into it. Both of the runs above are
    /// caught by it, and a genuine child — whose log was created microseconds before it started —
    /// is not.
    fn is_alive(&self, session: &dyn Session) -> bool {
        let state = State::read(session);
        let Some(pid) = state.pid else {
            return false;
        };
        match state.started {
            Some(started) => detached::pid_alive_as(pid, started),
            None => detached::pid_alive(pid) && started_before_the_log_stopped(pid, session),
        }
    }

    /// TERM, then KILL once `grace` is spent.
    ///
    /// The escalation is the point. A CLI that traps TERM — or one wedged in an uninterruptible
    /// read — used to survive being stopped: the old path signalled once, waited half a second, and
    /// returned success either way, leaving a run the human had stopped still burning tokens.
    /// `grace` is the whole budget for exiting cooperatively; after it, this is rude and says so.
    ///
    /// On Windows there is no cooperative signal for a headless child (it has no window to close),
    /// so the first signal is already a forced tree kill; the grace period simply never runs down.
    ///
    /// What it will not do is signal a pid it cannot confirm. The number in the slot outlives the
    /// child that owned it and the kernel reissues it, so "stop this finished run" was one recycled
    /// pid away from `kill -TERM` on somebody's whole process group — the browser, the audio
    /// daemon, whatever had been handed the number since. [`Self::is_alive`] is the same check the
    /// listing uses, so nothing can be stopped that was not shown as running either.
    fn stop(&self, session: &dyn Session, grace: Duration) -> Result<Stopped> {
        let Some(pid) = State::read(session).pid else {
            return Ok(Stopped::default());
        };
        if !self.is_alive(session) {
            return Ok(Stopped::default());
        }

        if grace.is_zero() {
            detached::signal_group(pid, "KILL")?;
            wait_for_exit(pid, REAP);
            return Ok(Stopped {
                was_running: true,
                forced: true,
            });
        }

        detached::signal_group(pid, "TERM")?;
        if wait_for_exit(pid, grace) {
            return Ok(Stopped {
                was_running: true,
                forced: false,
            });
        }
        detached::signal_group(pid, "KILL")?;
        wait_for_exit(pid, REAP);
        Ok(Stopped {
            was_running: true,
            forced: true,
        })
    }

    /// Mirrors what each engine's stream actually carries. Codex reports no reasoning blocks and
    /// the adi loop does not surface the model's thinking, so neither claims `thinking` — a reader
    /// that drew an empty pane for them would be lying about the engine rather than about the turn.
    /// Only the harness engines. A `process:*` run is a one-shot `--print` / `exec` that establishes
    /// no session and carries no flag to resume one, so a second send would open a fresh thread with
    /// no memory of the first — which is exactly the offer this must not make.
    fn resumes(&self) -> bool {
        matches!(
            self.backend,
            Backend::HarnessClaudeSdk | Backend::HarnessAdi
        )
    }

    /// The adi loop puts the bytes in the request body, because it is the one engine here whose
    /// request body this crate writes. The other three are handed their message as a command-line
    /// argument (`claude --print -- <text>`, `codex exec <text>`) where a picture has no
    /// representation — so they are told **where the file is** instead, and open it themselves.
    ///
    /// That works because all three are coding agents with a file-reading tool that decodes images:
    /// what arrives is the same picture, one tool call later. See
    /// [`ImageDelivery`](crate::runner::ImageDelivery) for what each costs.
    fn image_delivery(&self) -> ImageDelivery {
        match self.backend {
            Backend::HarnessAdi => ImageDelivery::Inline,
            Backend::ProcessClaude | Backend::ProcessCodex | Backend::HarnessClaudeSdk => {
                ImageDelivery::Path
            }
            _ => ImageDelivery::None,
        }
    }

    fn emits(&self) -> EventKinds {
        match self.backend {
            Backend::ProcessClaude | Backend::HarnessClaudeSdk => EventKinds {
                message: true,
                tool_call: true,
                thinking: true,
                metrics: true,
            },
            Backend::ProcessCodex | Backend::HarnessAdi => EventKinds {
                message: true,
                tool_call: true,
                thinking: false,
                metrics: true,
            },
            _ => EventKinds::default(),
        }
    }

    /// The log after `cursor`, translated.
    ///
    /// The cursor is a byte offset, which is all a log-backed runner needs and survives any process
    /// reading it. Two consequences worth knowing:
    ///
    /// - Only **whole lines** are consumed while the child is still writing, so a half-flushed
    ///   event is re-read next time instead of being parsed as garbage. Once the child has exited
    ///   the remainder is taken as-is, or a log with no trailing newline would replay for ever.
    /// - Each batch is parsed **on its own**, so a tool result whose call started in an earlier
    ///   batch is folded as that engine's parser sees fit rather than reaching back. A reader that
    ///   wants the whole picture asks with `None`, which re-reads the log from the top.
    fn events(&self, session: &dyn Session, cursor: Option<&Value>) -> Result<EventBatch> {
        let mut cursor = cursor
            .and_then(|value| serde_json::from_value::<Cursor>(value.clone()).ok())
            .unwrap_or_default();
        // A pid that no longer names a living process is what makes the log complete. A session
        // with no pid at all was never started here, so there is nothing to have finished.
        let done = State::read(session)
            .pid
            .is_some_and(|pid| !detached::pid_alive(pid));

        let (bytes, offset) = read_after(session.log_path(), cursor.offset, done);
        cursor.offset = offset;

        let content = self.parse(&bytes);
        let failed = content.metrics.as_ref().is_some_and(|m| m.is_error);
        let error = failed
            .then(|| content.text.trim().to_string())
            .filter(|text| !text.is_empty());

        let mut events = translate(content);
        if done && !cursor.finished {
            cursor.finished = true;
            events.push(RunEvent::Finished { ok: !failed, error });
        }
        Ok(EventBatch {
            events,
            cursor: cursor.into_value(),
        })
    }
}

/// What this runner keeps about a session: the child it started, and the engine session that child
/// established. Nothing else — everything durable is the store's.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct State {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pid: Option<u32>,
    /// When the process at [`pid`](Self::pid) started, in unix milliseconds.
    ///
    /// The pid's other half. On its own a pid identifies a *slot* the kernel reissues once the
    /// child exits, and nothing rewrites this record at that moment — a one-shot run's slot is
    /// simply left as it was, and an app that was killed never got to write anything — so by the
    /// time a listing reads it the number may name a stranger. It did: two finished runs showed as
    /// running for days because their pids had been handed to a browser tab and an audio helper.
    ///
    /// Absent on a record written before this was kept, and on a platform that cannot report it.
    /// See [`DetachedRunner::is_alive`] for what is made of that.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    started: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    session_id: Option<String>,
}

impl State {
    /// The slot's contents, or an empty state. A slot written by a different runner is not an
    /// error: it reads as "nothing of mine here", which is exactly what it is.
    fn read(session: &dyn Session) -> Self {
        session
            .state()
            .and_then(|value| serde_json::from_value(value).ok())
            .unwrap_or_default()
    }

    /// The same read through an owned handle, for the reaper thread — which outlives every borrowed
    /// view by construction.
    fn read_owned(writer: &dyn StateWriter) -> Self {
        writer
            .state()
            .and_then(|value| serde_json::from_value(value).ok())
            .unwrap_or_default()
    }

    fn into_value(self) -> Value {
        serde_json::to_value(self).unwrap_or(Value::Null)
    }
}

/// Strike an exited child from the state slot, if it is still the child the slot is describing.
///
/// Called from the reaper thread the moment a child is gone, so a finished run reads as finished
/// without waiting for anybody to ask. The engine session id stays: it is what a later turn resumes
/// the same conversation with, and it did not die with the process.
///
/// The guard is the whole subtlety. A harness conversation spawns a fresh child per turn into the
/// same slot, so by the time this thread wakes up, turn N+1 may already have written its own pid
/// there — and clearing it would leave a live child that nothing lists and nothing can stop. So the
/// slot is re-read and only cleared if it still names *this* child.
fn forget_child(writer: &dyn StateWriter, child: Spawned) {
    let mut state = State::read_owned(writer);
    if state.pid != Some(child.pid) || state.started != child.started {
        return;
    }
    state.pid = None;
    state.started = None;
    let _ = writer.set_state(state.into_value());
}

/// How long after a run's log was last written a process may have started and still be believed to
/// be the child that holds it open.
///
/// A real child's log is created by its parent immediately before the spawn, so its start time is
/// *behind* the log's mtime by microseconds and this only has to absorb clock granularity. A
/// recycled pid is hours or days ahead of it.
const LOG_MTIME_SLACK_MILLIS: u64 = 60_000;

/// Whether the process now holding `pid` could be the child that has been writing this session's
/// log — the only check left for a record written before start times were kept.
///
/// The log is created immediately before the child that writes into it, and stays open for as long
/// as that child lives, so its mtime can never be *older* than the start of the process that owns
/// it. A pid inherited long after the log went quiet fails on that alone, which is exactly what a
/// recycled one looks like. Unreadable either way reads as not ours: unverifiable must never be the
/// answer that keeps a run alive.
fn started_before_the_log_stopped(pid: u32, session: &dyn Session) -> bool {
    let Some(started) = detached::process_start_millis(pid) else {
        return false;
    };
    let Ok(written) = std::fs::metadata(session.log_path()).and_then(|meta| meta.modified()) else {
        return false;
    };
    let Ok(written) = written.duration_since(std::time::UNIX_EPOCH) else {
        return false;
    };
    started <= written.as_millis().try_into().unwrap_or(u64::MAX) + LOG_MTIME_SLACK_MILLIS
}

/// How far into the log a reader has got, and whether it has already been told the run ended.
///
/// The `finished` flag is why [`RunEvent::Finished`] is delivered exactly once: a poll that finds
/// no new bytes must not keep announcing the same ending on every tick.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
struct Cursor {
    #[serde(default)]
    offset: u64,
    #[serde(default)]
    finished: bool,
}

impl Cursor {
    fn into_value(self) -> Value {
        serde_json::to_value(self).unwrap_or(Value::Null)
    }
}

/// Decode an engine's own argument struct out of a spec.
///
/// Kept here beside the runner that needs it most, and `pub(super)` so [`super::pty`] borrows it
/// rather than keeping a second copy that could drift. Prompt composition used to live here too and
/// now lives in [`super::prompt`], which is the one place that renders one.
///
/// A spec carrying no engine configuration at all reads as that engine's defaults rather than as a
/// decode failure — `arguments` is optional storage, not a required document.
pub(super) fn decode<T: DeserializeOwned>(arguments: &Value) -> Result<T> {
    let value = if arguments.is_null() {
        Value::Object(serde_json::Map::new())
    } else {
        arguments.clone()
    };
    serde_json::from_value(value).map_err(|e| Error::Arguments(e.to_string()))
}

/// One turn's content as an event sequence: its timeline in order, then its answer, then the
/// telemetry the engine closed with.
fn translate(content: TurnContent) -> Vec<RunEvent> {
    let mut events: Vec<RunEvent> = content.steps.into_iter().map(RunEvent::Step).collect();
    if !content.text.trim().is_empty() {
        events.push(RunEvent::Answer { text: content.text });
    }
    if let Some(metrics) = content.metrics {
        events.push(RunEvent::Metrics(metrics));
    }
    events
}

/// The log's bytes after `offset`, and the offset to resume from. `complete` says the writer has
/// exited, so the tail cannot be a half-written line.
///
/// A log that isn't there yet is not an error — a child that has been spawned but has not printed
/// anything is the normal first poll of every run.
fn read_after(path: &Path, offset: u64, complete: bool) -> (Vec<u8>, u64) {
    let Ok(mut file) = File::open(path) else {
        return (Vec::new(), offset);
    };
    let Ok(len) = file.metadata().map(|meta| meta.len()) else {
        return (Vec::new(), offset);
    };
    // A harness turn spawns into the same log slot and truncates it, so an offset past the end is a
    // cursor left by the previous turn: start over rather than reading nothing for ever.
    let from = if offset > len { 0 } else { offset };
    if file.seek(SeekFrom::Start(from)).is_err() {
        return (Vec::new(), from);
    }
    let mut buf = Vec::new();
    if file
        .by_ref()
        .take(MAX_PARSE_BYTES)
        .read_to_end(&mut buf)
        .is_err()
    {
        return (Vec::new(), from);
    }
    let consumed = if complete {
        buf.len()
    } else {
        buf.iter()
            .rposition(|byte| *byte == b'\n')
            .map_or(0, |at| at + 1)
    };
    buf.truncate(consumed);
    (buf, from + consumed as u64)
}

/// Wait up to `budget` for `pid` to go away. Returns whether it did.
fn wait_for_exit(pid: u32, budget: Duration) -> bool {
    let deadline = Instant::now() + budget;
    loop {
        if !detached::pid_alive(pid) {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(POLL);
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::{Arc, Mutex};

    use adi_tools::ToolHelp;
    use serde_json::json;

    use super::*;
    use crate::progress::{Step, ToolStatus};

    /// A session that is nothing but the four answers a runner asks for. Deliberately not the real
    /// store: a runner must work against any `Session`, and these tests must not depend on a
    /// storage layer landing beside them.
    #[derive(Debug)]
    struct FakeSession {
        id: String,
        agent: String,
        started: bool,
        state: Arc<Mutex<Option<Value>>>,
        log: PathBuf,
    }

    impl FakeSession {
        fn new(tag: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "adi-runner-detached-{tag}-{}-{:?}",
                std::process::id(),
                std::thread::current().id()
            ));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).expect("scratch dir");
            Self {
                id: "conv-1".to_string(),
                agent: "solver".to_string(),
                started: false,
                state: Arc::new(Mutex::new(None)),
                log: dir.join("conv-1.log"),
            }
        }

        fn started(mut self) -> Self {
            self.started = true;
            self
        }

        fn with_state(self, state: Value) -> Self {
            *self.state.lock().unwrap() = Some(state);
            self
        }

        fn write_log(&self, text: &str) {
            std::fs::write(&self.log, text).expect("write log");
        }
    }

    impl Drop for FakeSession {
        fn drop(&mut self) {
            if let Some(dir) = self.log.parent() {
                let _ = std::fs::remove_dir_all(dir);
            }
        }
    }

    impl Session for FakeSession {
        fn id(&self) -> &str {
            &self.id
        }
        fn agent(&self) -> &str {
            &self.agent
        }
        fn has_started(&self) -> bool {
            self.started
        }
        fn state(&self) -> Option<Value> {
            self.state.lock().unwrap().clone()
        }
        fn set_state(&self, value: Value) -> Result<()> {
            *self.state.lock().unwrap() = Some(value);
            Ok(())
        }
        fn state_writer(&self) -> Option<Box<dyn StateWriter>> {
            Some(Box::new(SharedState(Arc::clone(&self.state))))
        }
        fn log_path(&self) -> &Path {
            &self.log
        }
    }

    /// The owned half of a [`FakeSession`]'s slot, as the store's own view hands out.
    struct SharedState(Arc<Mutex<Option<Value>>>);

    impl StateWriter for SharedState {
        fn state(&self) -> Option<Value> {
            self.0.lock().unwrap().clone()
        }
        fn set_state(&self, value: Value) -> Result<()> {
            *self.0.lock().unwrap() = Some(value);
            Ok(())
        }
    }

    fn spec(arguments: Value) -> RunSpec {
        RunSpec {
            cwd: std::env::temp_dir(),
            path: "/usr/bin".to_string(),
            env: Vec::new(),
            arguments,
            tools: Vec::new(),
            tool_help: None,
            system_prompt: None,
            workspace_note: None,
            knowledge_note: None,
        }
    }

    /// A pid that certainly names nothing: a child run to completion and reaped. Not a made-up
    /// number — pid 1 is `launchd`/`init` and answers "alive", which would quietly invert every
    /// assertion about a finished run.
    fn dead_pid() -> u32 {
        #[cfg(unix)]
        let mut child = std::process::Command::new("/bin/sh")
            .args(["-c", "exit 0"])
            .spawn()
            .expect("spawn");
        #[cfg(not(unix))]
        let mut child = std::process::Command::new("cmd")
            .args(["/c", "exit"])
            .spawn()
            .expect("spawn");
        let pid = child.id();
        let _ = child.wait();
        pid
    }

    fn tool(name: &str) -> ToolHelp {
        ToolHelp {
            name: name.to_string(),
            description: Some("Work the task tree.".to_string()),
            help: Some(format!("Usage: {name} <CMD>")),
        }
    }

    fn argv_of(backend: Backend, spec: &RunSpec, session: &dyn Session, message: &str) -> Vec<String> {
        DetachedRunner::new(backend)
            .argv(spec, session, message, "sid-1")
            .expect("argv")
    }

    #[test]
    fn a_detached_run_is_not_a_terminal() {
        let runner = DetachedRunner::new(Backend::ProcessClaude);
        assert_eq!(runner.kind().as_str(), "detached");
        assert!(runner.as_terminal().is_none());
    }

    /// `check` is the fail-fast gate: it parses the engine's own arguments and, for the adi loop,
    /// insists a provider was picked. No side effects, so it is safe on a read path.
    #[test]
    fn check_parses_each_engines_own_arguments() {
        let claude = DetachedRunner::new(Backend::HarnessClaudeSdk);
        assert!(claude.check(&spec(json!({ "model": "opus" }))).is_ok());
        assert!(matches!(
            claude.check(&spec(json!({ "temperature": 0.2 }))),
            Err(Error::Arguments(_))
        ));
        assert!(claude.check(&spec(Value::Null)).is_ok());

        let adi = DetachedRunner::new(Backend::HarnessAdi);
        assert!(matches!(
            adi.check(&spec(json!({ "model": "qwen3.6" }))),
            Err(Error::NotRunnable(backend)) if backend == "harness:adi"
        ));
        assert!(adi.check(&spec(json!({ "provider": "ollama" }))).is_ok());

        assert!(matches!(
            DetachedRunner::new(Backend::PtyClaude).check(&spec(Value::Null)),
            Err(Error::NotRunnable(_))
        ));
    }

    /// The continuation nobody passes: whether the Claude CLI is told to establish a session or to
    /// resume it comes only from whether a turn has already run here.
    #[test]
    fn a_claude_session_is_established_once_and_resumed_after() {
        let spec = spec(json!({}));
        let first = argv_of(Backend::HarnessClaudeSdk, &spec, &FakeSession::new("first"), "go");
        assert!(first.windows(2).any(|w| w == ["--session-id", "sid-1"]), "{first:?}");
        assert!(!first.iter().any(|arg| arg == "--resume"));

        let again = argv_of(
            Backend::HarnessClaudeSdk,
            &spec,
            &FakeSession::new("again").started(),
            "and now a test",
        );
        assert!(again.windows(2).any(|w| w == ["--resume", "sid-1"]), "{again:?}");
        assert!(!again.iter().any(|arg| arg == "--session-id"));
    }

    /// An id already in the state slot is the thread to continue; a session that has never run
    /// mints one. Either way the caller said nothing about it.
    #[test]
    fn the_engine_session_id_comes_from_the_state_slot() {
        let runner = DetachedRunner::new(Backend::HarnessClaudeSdk);
        let known = FakeSession::new("known")
            .started()
            .with_state(json!({ "pid": 1, "session_id": "kept" }));
        assert_eq!(runner.engine_session_id(&State::read(&known)), "kept");

        let fresh = FakeSession::new("fresh");
        let minted = runner.engine_session_id(&State::read(&fresh));
        assert_eq!(minted.len(), 36, "a v4 uuid: {minted}");

        let adi = DetachedRunner::new(Backend::HarnessAdi);
        assert!(adi.engine_session_id(&State::read(&fresh)).is_empty());
    }

    /// A `working_dir` decides where the child is *spawned* — [`RunSpec::cwd`], resolved once by the
    /// agent layer — so it must never also leak into the argv. No engine has such a flag, and a
    /// second answer to the same question is how the two drift apart.
    #[test]
    fn a_working_dir_never_reaches_the_engines_command() {
        let spec = spec(json!({ "working_dir": "/repo/main" }));
        for backend in [
            Backend::ProcessClaude,
            Backend::ProcessCodex,
            Backend::HarnessClaudeSdk,
        ] {
            let argv = argv_of(backend.clone(), &spec, &FakeSession::new("workdir"), "go");
            assert!(
                !argv.iter().any(|arg| arg.contains("/repo/main")),
                "{backend}: {argv:?}"
            );
        }
    }

    /// Where the run starts has to be *stated*, not merely exported.
    ///
    /// `ADI_WORKDIR` serves a script; it does nothing for the agent itself, which reads prose — and
    /// one left to infer its directory re-derives it from the task and prefixes `cd …` onto command
    /// after command, writing to the wrong place the first time one forgets. This is the assertion
    /// that keeps the note in the prompt: it went missing once already, when prompt composition moved
    /// out of the store and only tool help came with it.
    #[test]
    fn the_run_is_told_where_it_starts_before_it_is_told_about_its_tools() {
        let mut located = spec(json!({}));
        located.system_prompt = Some("You are a planner.".to_string());
        located.workspace_note = Some("# Where you are\n\nYou start in /repo/main.".to_string());
        located.tools = vec![tool("adi-tasks")];

        let claude = argv_of(
            Backend::ProcessClaude,
            &located,
            &FakeSession::new("claude-located"),
            "go",
        );
        let at = claude
            .iter()
            .position(|arg| arg == "--append-system-prompt")
            .expect("claude takes a system prompt");
        let prompt = &claude[at + 1];
        let note = prompt.find("# Where you are").expect("the location is stated");
        let tools = prompt.find("# Your tools").expect("the tools are listed");
        assert!(note < tools, "{prompt}");
        assert!(prompt.starts_with("You are a planner."), "{prompt}");

        let codex = argv_of(
            Backend::ProcessCodex,
            &located,
            &FakeSession::new("codex-located"),
            "go",
        );
        assert!(
            codex.iter().all(|arg| !arg.contains("# Where you are")),
            "codex must not be handed the location block: {codex:?}"
        );
    }


    /// Both Claude engines are pointed at this run's own MCP server, scoped to the conversation it
    /// belongs to and to the directory the run is about. Codex is not: it reads none of these flags.
    #[test]
    fn the_claude_engines_are_launched_with_this_runs_mcp_server() {
        for backend in [Backend::ProcessClaude, Backend::HarnessClaudeSdk] {
            let argv = argv_of(
                backend.clone(),
                &spec(json!({})),
                &FakeSession::new("served"),
                "go",
            );
            let at = argv
                .iter()
                .position(|arg| arg == "--mcp-config")
                .unwrap_or_else(|| panic!("{backend:?} takes an mcp config: {argv:?}"));
            let config: Value = serde_json::from_str(&argv[at + 1]).expect("mcp config is json");
            let server = &config["mcpServers"][crate::backends::mcp::SERVER];
            let spawn_args: Vec<&str> = server["args"]
                .as_array()
                .expect("args")
                .iter()
                .map(|a| a.as_str().expect("string"))
                .collect();
            assert_eq!(spawn_args[0], "mcp");
            assert!(spawn_args.contains(&"solver"), "{spawn_args:?}");
            assert!(spawn_args.contains(&"conv-1"), "{spawn_args:?}");
            let dir = spawn_args
                .iter()
                .position(|a| *a == "--dir")
                .expect("the run directory is passed");
            assert_eq!(spawn_args[dir + 1], std::env::temp_dir().to_string_lossy());

            assert!(
                argv.iter().any(|arg| arg == "--strict-mcp-config"),
                "{argv:?}"
            );
            let terminator = argv
                .iter()
                .position(|arg| arg == "--")
                .unwrap_or_else(|| panic!("{backend:?} ends option parsing: {argv:?}"));
            assert!(at < terminator, "the mcp config must precede `--`: {argv:?}");
        }

        let codex = argv_of(
            Backend::ProcessCodex,
            &spec(json!({})),
            &FakeSession::new("codex-mcp"),
            "go",
        );
        assert!(
            codex.iter().all(|arg| arg != "--mcp-config"),
            "codex reads no such flag: {codex:?}"
        );
    }

    /// The two halves that make the MCP server usable rather than merely present: the engine's own
    /// built-ins are off — the shell for good — and ours is granted. Granting is what is easy to
    /// forget: an ungranted MCP tool is advertised to the model and then refused when it calls it.
    #[test]
    fn a_run_starts_with_no_engine_tools_and_the_grant_for_ours() {
        let flag = |argv: &[String], name: &str| -> String {
            let at = argv
                .iter()
                .position(|arg| arg == name)
                .unwrap_or_else(|| panic!("{name} is set: {argv:?}"));
            argv[at + 1].clone()
        };

        for backend in [Backend::ProcessClaude, Backend::HarnessClaudeSdk] {
            let argv = argv_of(
                backend.clone(),
                &spec(json!({})),
                &FakeSession::new("scoped"),
                "go",
            );
            let allowed = flag(&argv, "--allowed-tools");
            assert!(allowed.split(',').any(|t| t == "mcp__adi"), "{allowed}");
            assert_eq!(flag(&argv, "--tools"), "", "{argv:?}");
        }

        let opinionated = spec(json!({ "allowed_tools": "Read,mcp__adi,Bash" }));
        let argv = argv_of(
            Backend::ProcessClaude,
            &opinionated,
            &FakeSession::new("opinionated"),
            "go",
        );
        let allowed = flag(&argv, "--allowed-tools");
        assert_eq!(flag(&argv, "--tools"), "Read", "{argv:?}");
        assert!(allowed.split(',').any(|t| t == "Read"), "{allowed}");
        assert_eq!(
            allowed.split(',').filter(|t| *t == "mcp__adi").count(),
            1,
            "a grant already present is not repeated: {allowed}"
        );
        for engine_tool in crate::backends::mcp::ENGINE_SHELL_TOOLS {
            assert!(
                !flag(&argv, "--tools").split(',').any(|t| t == *engine_tool),
                "{engine_tool} is never grantable: {argv:?}"
            );
        }
    }

    /// Tool help is prompt material for the Claude engines and poison for Codex, whose
    /// `system_prompt` is really its opening user turn.
    #[test]
    fn tool_help_reaches_claudes_system_prompt_and_never_codexs() {
        let mut with_tools = spec(json!({}));
        with_tools.tools = vec![tool("adi-tasks")];
        with_tools.system_prompt = Some("You are a planner.".to_string());

        let claude = argv_of(
            Backend::ProcessClaude,
            &with_tools,
            &FakeSession::new("claude-tools"),
            "go",
        );
        let at = claude
            .iter()
            .position(|arg| arg == "--append-system-prompt")
            .expect("claude takes a system prompt");
        let prompt = &claude[at + 1];
        assert!(prompt.starts_with("You are a planner."), "{prompt}");
        assert!(prompt.contains("## adi-tasks"), "{prompt}");
        assert!(prompt.contains("Usage: adi-tasks <CMD>"), "{prompt}");

        let codex = argv_of(
            Backend::ProcessCodex,
            &with_tools,
            &FakeSession::new("codex-tools"),
            "fix the tests",
        );
        assert!(
            codex.iter().all(|arg| !arg.contains("adi-tasks")),
            "codex must not be handed tool help: {codex:?}"
        );
        assert_eq!(
            codex.last().map(String::as_str),
            Some("You are a planner.\n\nfix the tests")
        );
    }

    /// The adi loop's turn is a re-entry into this binary addressed by agent + conversation, and
    /// its prompt travels in the environment because there is no flag to put it on.
    #[test]
    fn the_adi_loop_reenters_this_binary_and_carries_its_prompt_in_the_environment() {
        let runner = DetachedRunner::new(Backend::HarnessAdi);
        let session = FakeSession::new("adi");
        let mut spec = spec(json!({ "provider": "ollama", "model": "qwen3.6" }));
        spec.tools = vec![tool("adi-db")];
        spec.system_prompt = Some("You are an operator.".to_string());

        let argv = runner.argv(&spec, &session, "hi", "").expect("argv");
        assert_eq!(
            std::path::Path::new(&argv[0])
                .file_name()
                .and_then(|n| n.to_str()),
            Some("adi-mono")
        );
        assert!(argv.iter().any(|arg| arg == "harness-turn"));
        assert!(argv.iter().any(|arg| arg == "solver"), "{argv:?}");
        assert!(argv.iter().any(|arg| arg == "conv-1"), "{argv:?}");

        let env = runner.run_env(&spec);
        let prompt = env
            .iter()
            .find(|(key, _)| key == SYSTEM_PROMPT_ENV)
            .map(|(_, value)| value.as_str())
            .expect("the composed prompt is exported");
        assert!(prompt.starts_with("You are an operator."), "{prompt}");
        assert!(prompt.contains("## adi-db"), "{prompt}");

        let claude = DetachedRunner::new(Backend::HarnessClaudeSdk);
        assert!(claude.run_env(&spec).iter().all(|(key, _)| key != SYSTEM_PROMPT_ENV));
    }

    /// A Claude turn's `stream-json` log becomes the timeline, the answer, and the telemetry — and
    /// a second read from the returned cursor finds nothing new to say.
    #[test]
    fn a_claude_log_translates_to_steps_an_answer_and_metrics() {
        let session = FakeSession::new("claude-events").with_state(json!({ "pid": dead_pid() }));
        session.write_log(&format!(
            "{}\n{}\n{}\n{}\n",
            json!({"type": "assistant", "message": {"content": [
                {"type": "thinking", "thinking": "let me look"},
                {"type": "tool_use", "id": "t1", "name": "Read", "input": {"path": "a.rs"}}
            ]}}),
            json!({"type": "user", "message": {"content": [
                {"type": "tool_result", "tool_use_id": "t1", "content": "fn main() {}"}
            ]}}),
            json!({"type": "assistant", "message": {"content": [{"type": "text", "text": "done"}]}}),
            json!({"type": "result", "result": "done", "usage": {"input_tokens": 12, "output_tokens": 3}}),
        ));

        let runner = DetachedRunner::new(Backend::HarnessClaudeSdk);
        let batch = runner.events(&session, None).expect("events");
        assert!(matches!(batch.events[0], RunEvent::Step(Step::Thinking { .. })));
        assert!(matches!(
            &batch.events[1],
            RunEvent::Step(Step::Tool { name, status, output, .. })
                if name == "Read" && *status == ToolStatus::Ok && output == "fn main() {}"
        ));
        assert!(matches!(&batch.events[2], RunEvent::Answer { text } if text == "done"));
        assert!(matches!(&batch.events[3], RunEvent::Metrics(m) if m.input_tokens == Some(12)));

        assert_eq!(batch.events.len(), 5, "{:?}", batch.events);
        assert!(matches!(
            batch.events[4],
            RunEvent::Finished { ok: true, error: None }
        ));
        let next = runner.events(&session, Some(&batch.cursor)).expect("events");
        assert!(next.events.is_empty(), "{:?}", next.events);
        assert_eq!(next.cursor, batch.cursor);
    }

    /// The adi loop's own event lines, and the cursor doing its job: a turn still writing hands
    /// back only what it has finished a line of, and the next read picks up exactly there.
    #[test]
    fn the_adi_loops_events_are_read_incrementally_from_the_cursor() {
        // A live pid: this process. The turn reads as still running, so a partial trailing line is
        // left for the next poll rather than parsed in half.
        let session = FakeSession::new("adi-events")
            .with_state(json!({ "pid": std::process::id() }));
        session.write_log(&format!(
            "{}\n{}",
            json!({"kind": "tool", "id": "c1", "name": "Grep", "input": "{}", "status": "running"}),
            r#"{"kind": "answer", "text": "half a li"#,
        ));

        let runner = DetachedRunner::new(Backend::HarnessAdi);
        let first = runner.events(&session, None).expect("events");
        assert_eq!(first.events.len(), 1, "{:?}", first.events);
        assert!(matches!(
            &first.events[0],
            RunEvent::Step(Step::Tool { name, status, .. })
                if name == "Grep" && *status == ToolStatus::Running
        ));
        assert!(
            !first.events.iter().any(|e| matches!(e, RunEvent::Finished { .. })),
            "a running turn has not finished",
        );

        session.write_log(&format!(
            "{}\n{}\n",
            json!({"kind": "tool", "id": "c1", "name": "Grep", "input": "{}", "status": "running"}),
            json!({"kind": "answer", "text": "found it"}),
        ));
        let second = runner
            .events(&session, Some(&first.cursor))
            .expect("events");
        assert_eq!(second.events.len(), 1, "{:?}", second.events);
        assert!(matches!(&second.events[0], RunEvent::Answer { text } if text == "found it"));
    }

    /// A log replaced under a stale cursor (the next turn spawns into the same slot and truncates
    /// it) is read from the top rather than skipped for ever.
    #[test]
    fn a_cursor_past_the_end_starts_over() {
        let session = FakeSession::new("truncated").with_state(json!({ "pid": dead_pid() }));
        session.write_log("short\n");
        let batch = DetachedRunner::new(Backend::ProcessCodex)
            .events(&session, Some(&json!({ "offset": 9_000, "finished": true })))
            .expect("events");
        assert!(
            matches!(&batch.events[0], RunEvent::Answer { text } if text == "short"),
            "{:?}",
            batch.events
        );
    }

    #[test]
    fn what_each_engine_can_ever_report() {
        let claude = DetachedRunner::new(Backend::ProcessClaude).emits();
        assert!(claude.message && claude.tool_call && claude.thinking && claude.metrics);
        for backend in [Backend::ProcessCodex, Backend::HarnessAdi] {
            let emits = DetachedRunner::new(backend).emits();
            assert!(emits.message && emits.tool_call && emits.metrics);
            assert!(!emits.thinking);
        }
    }

    /// The command is built before anything is spawned, so a spec this engine cannot run leaves no
    /// child, no state, and no log behind — `send` is all-or-nothing.
    #[test]
    fn a_send_that_cannot_build_a_command_starts_nothing() {
        let runner = DetachedRunner::new(Backend::HarnessAdi);
        let session = FakeSession::new("no-provider");
        assert!(matches!(
            runner.send(&spec(json!({ "model": "qwen3.6" })), &session, "go"),
            Err(Error::NotRunnable(_))
        ));
        assert!(session.state().is_none(), "nothing was recorded");
        assert!(!session.log_path().exists(), "no log was opened");
        assert!(!runner.is_alive(&session));
    }

    #[test]
    fn stopping_something_that_was_never_started_is_not_a_stop() {
        let runner = DetachedRunner::new(Backend::ProcessClaude);
        let session = FakeSession::new("nothing");
        assert_eq!(runner.stop(&session, Duration::ZERO).expect("stop"), Stopped::default());
        assert!(!runner.is_alive(&session));
    }

    /// The bug this runner exists to fix. A CLI that traps TERM used to survive being stopped
    /// outright; here the grace runs out and the kill lands.
    #[cfg(unix)]
    #[test]
    fn a_child_that_ignores_term_is_killed_once_the_grace_is_spent() {
        let runner = DetachedRunner::new(Backend::ProcessClaude);

        let deaf = FakeSession::new("deaf");
        spawn_probe(&deaf, "trap '' TERM; sleep 30");
        let stopped = runner
            .stop(&deaf, Duration::from_millis(200))
            .expect("stop");
        assert_eq!(
            stopped,
            Stopped { was_running: true, forced: true },
            "a deaf child has to be killed",
        );
        assert!(!runner.is_alive(&deaf));

        let polite = FakeSession::new("polite");
        spawn_probe(&polite, "sleep 30");
        let stopped = runner.stop(&polite, Duration::from_secs(5)).expect("stop");
        assert_eq!(stopped, Stopped { was_running: true, forced: false });
        assert!(!runner.is_alive(&polite));

        let now = FakeSession::new("now");
        spawn_probe(&now, "trap '' TERM; sleep 30");
        let stopped = runner.stop(&now, Duration::ZERO).expect("stop");
        assert_eq!(stopped, Stopped { was_running: true, forced: true });
        assert!(!runner.is_alive(&now));

        assert_eq!(runner.stop(&now, Duration::ZERO).expect("stop"), Stopped::default());
    }

    /// The bug this whole pairing exists for. A pid is a slot the kernel reissues, so a run that
    /// finished last week can be holding a number that now belongs to a browser tab — and the tab
    /// is genuinely alive, so asking only `kill(pid, 0)` reports the run as still going. Two runs
    /// sat "running" for three days on exactly that.
    ///
    /// Same live pid throughout; only the recorded start time differs. That is the whole test: the
    /// number cannot tell these apart and the pair can.
    #[test]
    fn a_pid_that_was_handed_to_somebody_else_is_not_this_run() {
        let pid = std::process::id();
        let started = adi_osext::process_start_millis(pid).expect("this platform can say");
        let runner = DetachedRunner::new(Backend::HarnessAdi);

        let ours = FakeSession::new("reuse-ours")
            .with_state(json!({ "pid": pid, "started": started }));
        assert!(runner.is_alive(&ours), "our own child, correctly named");

        // The same pid, recorded when a different process held it. Alive, and not ours.
        let theirs = FakeSession::new("reuse-theirs")
            .with_state(json!({ "pid": pid, "started": started - 86_400_000 }));
        assert!(
            !runner.is_alive(&theirs),
            "a live pid recorded against another process is not this run"
        );
    }

    /// A run recorded before start times were kept has only a pid, and it is the same pid the
    /// kernel may since have reissued. What is still checkable is the log: a child holds it open
    /// for as long as it lives, and the log is created immediately *before* the child that writes
    /// into it — so a process that started well after the log fell silent was never that child.
    #[test]
    fn a_record_from_before_start_times_is_judged_by_its_log() {
        let pid = std::process::id();
        let runner = DetachedRunner::new(Backend::HarnessAdi);

        // A log this process could plausibly be writing: it was touched after we started.
        let live = FakeSession::new("legacy-live").with_state(json!({ "pid": pid }));
        live.write_log("still going");
        assert!(runner.is_alive(&live), "a pid whose log is still being written");

        // The same pid against a log that went quiet days before this process existed — which is
        // what both of the stuck runs looked like.
        let stale = FakeSession::new("legacy-stale").with_state(json!({ "pid": pid }));
        stale.write_log("what it said, days ago");
        let long_ago = std::time::SystemTime::now() - Duration::from_secs(3 * 86_400);
        std::fs::File::options()
            .write(true)
            .open(stale.log_path())
            .expect("open the log")
            .set_times(std::fs::FileTimes::new().set_modified(long_ago))
            .expect("age the log");
        assert!(
            !runner.is_alive(&stale),
            "a process that started after the log went quiet never wrote it"
        );

        let bare = FakeSession::new("legacy-bare").with_state(json!({ "pid": pid }));
        assert!(!runner.is_alive(&bare));
    }

    /// Stopping is where a recycled pid stops being a display bug and becomes a loaded gun: the
    /// signal goes to a whole process *group*. A run that cannot be confirmed is not signalled at
    /// all — here the innocent bystander is a real child of this test, and it has to survive.
    #[cfg(unix)]
    #[test]
    fn a_run_that_cannot_be_confirmed_is_never_signalled() {
        let bystander = FakeSession::new("no-signal");
        spawn_probe(&bystander, "sleep 30");
        let pid = State::read(&bystander).pid.expect("the probe recorded a pid");

        // Rewrite the slot as a record from another incarnation of that pid — alive, wrong process.
        let started = State::read(&bystander).started.expect("a start time");
        bystander
            .set_state(json!({ "pid": pid, "started": started - 86_400_000 }))
            .expect("recycle the record");

        let runner = DetachedRunner::new(Backend::HarnessAdi);
        assert!(!runner.is_alive(&bystander));
        assert_eq!(
            runner.stop(&bystander, Duration::ZERO).expect("stop"),
            Stopped::default(),
            "an unconfirmable run reports nothing was running",
        );
        assert!(
            detached::pid_alive(pid),
            "and above all does not kill the process that now holds the number",
        );

        let _ = detached::signal_group(pid, "KILL");
    }

    /// The ending, written down at the moment it happens rather than inferred later. The engine
    /// session id survives it: the process died, the conversation did not, and the next turn
    /// resumes the same thread with it.
    #[test]
    fn an_ending_clears_the_child_and_keeps_the_thread() {
        let session = FakeSession::new("forget").with_state(json!({
            "pid": 4321,
            "started": 111,
            "session_id": "engine-thread",
        }));
        let writer = session.state_writer().expect("an owned slot");

        forget_child(writer.as_ref(), Spawned { pid: 4321, started: Some(111) });

        let state = State::read(&session);
        assert_eq!(state.pid, None, "the child is struck from the record");
        assert_eq!(state.started, None);
        assert_eq!(
            state.session_id.as_deref(),
            Some("engine-thread"),
            "the conversation outlives the process that was answering it",
        );
    }

    /// A harness conversation spawns a fresh child per turn into the same slot, so a reaper thread
    /// can wake up to find turn N+1 already recorded. Clearing it then would leave a live child
    /// that nothing lists and nothing can stop — the same class of orphan this all set out to fix,
    /// only inverted.
    #[test]
    fn an_ending_that_arrives_after_the_next_turn_started_is_ignored() {
        let session = FakeSession::new("forget-late").with_state(json!({
            "pid": 999,
            "started": 222,
            "session_id": "engine-thread",
        }));
        let writer = session.state_writer().expect("an owned slot");

        // Turn N's reaper, arriving after turn N+1 wrote its own child into the slot.
        forget_child(writer.as_ref(), Spawned { pid: 4321, started: Some(111) });

        let state = State::read(&session);
        assert_eq!(state.pid, Some(999), "the turn in flight is left alone");
        assert_eq!(state.started, Some(222));

        // A pid that matches but an incarnation that does not is still somebody else's ending.
        forget_child(writer.as_ref(), Spawned { pid: 999, started: Some(111) });
        assert_eq!(State::read(&session).pid, Some(999));
    }

    /// Put a real child of `script` behind `session`, as `send` would — the engines' own argv would
    /// need the vendor CLIs installed, and what is under test is the signalling, not the command.
    #[cfg(unix)]
    fn spawn_probe(session: &FakeSession, script: &str) {
        let log = session.log_path();
        let dir = log.parent().expect("a log dir");
        let child = detached::spawn_child(
            dir,
            "conv-1",
            log,
            dir,
            "/usr/bin:/bin",
            &["/bin/sh".to_string(), "-c".to_string(), script.to_string()],
            &[],
            |_| {},
        )
        .expect("spawn");
        session
            .set_state(json!({ "pid": child.pid, "started": child.started }))
            .expect("record the child");
        // `trap` is only installed once the shell has started; signalling before that would test
        // nothing.
        for _ in 0..100 {
            if detached::pid_alive(child.pid) {
                break;
            }
            std::thread::sleep(POLL);
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}
