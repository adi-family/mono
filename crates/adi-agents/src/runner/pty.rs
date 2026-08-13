//! The pty runner: a live terminal session driven through [`adi_pty`].
//!
//! The other runners answer; this one is *typed into*. That difference is the whole shape of the
//! module: there is nothing structured to read off a tty, so it declares an empty
//! [`emits`](Runner::emits) and hands back an empty batch from [`events`](Runner::events); its
//! screen is a mutable snapshot rather than an append stream, so it is read through
//! [`Terminal::capture`] instead. A caller asks `as_terminal()` rather than testing the kind, which
//! is what lets a future `tmux-pty` or `cloud-pty` land without touching a single call site.
//!
//! One instance per engine, as with [`super::detached`] — `claude` and `codex` open a terminal the
//! same way but do not take the same flags.

use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::arguments::{PtyClaudeArguments, PtyCodexArguments};
use crate::backend::Backend;
use crate::backends::pty;
use crate::error::{Error, Result};
use crate::runner::detached::decode;
use crate::runner::prompt::{own_prompt, with_knowledge, with_tool_help, with_workspace};
use crate::runner::{
    EventBatch, EventKinds, RunEvent, RunSpec, Runner, RunnerKind, Session, Stopped, Terminal,
};

/// How often a stopping session is re-probed while its grace period runs down.
const POLL: Duration = Duration::from_millis(25);

/// The key an interrupt is sent as — a terminal's cooperative stop, and the only one it has.
const INTERRUPT: &str = "C-c";

/// The key a message is committed with. A line typed into a TUI and never submitted is not a
/// message; it is an unsent draft sitting in somebody's input box.
const SUBMIT: &str = "Enter";

/// A live terminal session for one engine.
#[derive(Debug, Clone)]
pub struct PtyRunner {
    backend: Backend,
}

impl PtyRunner {
    #[must_use]
    pub fn new(backend: Backend) -> Self {
        Self { backend }
    }

    /// The engine's command line. No message on it: a terminal engine is handed its prompt, not its
    /// task — what the run is asked to do is typed in afterwards.
    fn argv(&self, spec: &RunSpec, session: &dyn Session) -> Result<Vec<String>> {
        match &self.backend {
            Backend::PtyClaude => {
                let mut config = decode::<PtyClaudeArguments>(&spec.arguments)?;
                config.system_prompt = own_prompt(spec, config.system_prompt);
                config.append_system_prompt =
                    with_tool_help(spec, with_knowledge(spec, with_workspace(spec, config.append_system_prompt)));
                let tools = crate::backends::mcp::scope_tools(config.allowed_tools.as_deref());
                Ok(pty::claude::argv(
                    &config,
                    Some(&crate::backends::mcp::config(spec, session)),
                    &tools,
                ))
            }
            Backend::PtyCodex => {
                // Neither tool help nor the location block: Codex's `system_prompt` is pushed as its
                // opening *user* turn, so either would open the session with something to answer
                // instead of context.
                let mut config = decode::<PtyCodexArguments>(&spec.arguments)?;
                config.system_prompt = own_prompt(spec, config.system_prompt);
                // `--cd` is the run's directory told to Codex, because that directory scopes its
                // sandbox rather than merely being where the process starts.
                Ok(pty::codex::argv(&config, spec.cwd.to_str()))
            }
            other => Err(Error::NotRunnable(other.to_string())),
        }
    }
}

impl Runner for PtyRunner {
    fn kind(&self) -> RunnerKind {
        RunnerKind::new("pty")
    }

    fn check(&self, spec: &RunSpec) -> Result<()> {
        match &self.backend {
            Backend::PtyClaude => decode::<PtyClaudeArguments>(&spec.arguments).map(drop),
            Backend::PtyCodex => decode::<PtyCodexArguments>(&spec.arguments).map(drop),
            other => Err(Error::NotRunnable(other.to_string())),
        }
    }

    /// Open the terminal if it is not open, then type.
    ///
    /// Both halves of the one start verb: a session that does not exist is established with the
    /// engine's own command line, and one that does is continued by typing into it — for a
    /// terminal, "resume" *is* keystrokes. The caller never chooses.
    ///
    /// The message of the launching call is deliberately not typed: the engine's TUI is still
    /// drawing its first frame, and keys written into it before it starts reading are simply lost.
    /// A caller with something to say says it on the next `send`, or through [`Terminal::send_keys`]
    /// once the pane is up.
    fn send(&self, spec: &RunSpec, session: &dyn Session, message: &str) -> Result<()> {
        let name = session_name(session);
        if adi_pty::is_running(&name) {
            // A bare Enter into a live TUI submits an empty line, so an empty message is nothing
            // said rather than a blank one sent.
            if message.trim().is_empty() {
                return Ok(());
            }
            return self.send_keys(session, message, SUBMIT);
        }

        let argv = self.argv(spec, session)?;
        // Secrets and the agent's declared vars first, under their literal names; `PATH` last, so
        // nothing injected can shadow the agent's own `.bin` and tool directories.
        let mut env = spec.env.clone();
        env.push(("PATH".to_string(), spec.path.clone()));
        adi_pty::launch(&name, &argv, &spec.cwd, &env).map_err(|e| Error::Launch(e.to_string()))?;

        session.set_state(State { session: Some(name) }.into_value())
    }

    fn is_alive(&self, session: &dyn Session) -> bool {
        adi_pty::is_running(&session_name(session))
    }

    /// Interrupt, then kill once `grace` is spent.
    ///
    /// A terminal's cooperative stop is the one a person would use: `C-c`, and a moment to let the
    /// engine tear itself down. Past the grace the pty is closed under it, which is rude in exactly
    /// the way [`Stopped::forced`] exists to record. [`Duration::ZERO`] skips the asking.
    ///
    /// A session that interrupted itself successfully is left in the registry, dead: its final
    /// screen stays capturable, which is the last thing anybody wants to read after a stop.
    fn stop(&self, session: &dyn Session, grace: Duration) -> Result<Stopped> {
        let name = session_name(session);
        if !adi_pty::is_running(&name) {
            return Ok(Stopped::default());
        }
        if !grace.is_zero() {
            adi_pty::send_keys(&name, "", INTERRUPT).map_err(|e| Error::Session(e.to_string()))?;
            if wait_for_exit(&name, grace) {
                return Ok(Stopped {
                    was_running: true,
                    forced: false,
                });
            }
        }
        adi_pty::stop(&name).map_err(|e| Error::Session(e.to_string()))?;
        Ok(Stopped {
            was_running: true,
            forced: true,
        })
    }

    /// Nothing. A tty carries no structured events — every character on it is screen paint, and
    /// guessing at messages or tool calls from cursor movement would be inventing a stream the
    /// engine never wrote.
    fn emits(&self) -> EventKinds {
        EventKinds::default()
    }

    /// A live pane is the one thing that always continues: the session never went anywhere, and the
    /// next thing said is simply typed into it. That it is *reached* by keystrokes rather than by a
    /// reply box is a separate question, answered by [`Runner::as_terminal`].
    fn resumes(&self) -> bool {
        true
    }

    /// Always empty, and the cursor is handed straight back: there is nothing to be after. A reader
    /// that polls this instead of [`Terminal::capture`] gets silence rather than an error, because
    /// having no event stream is a property of terminals, not a failure of this one.
    fn events(&self, _session: &dyn Session, cursor: Option<&Value>) -> Result<EventBatch> {
        Ok(EventBatch {
            events: Vec::<RunEvent>::new(),
            cursor: cursor.cloned().unwrap_or(Value::Null),
        })
    }

    fn as_terminal(&self) -> Option<&dyn Terminal> {
        Some(self)
    }
}

impl Terminal for PtyRunner {
    /// Type into the live session. Refused when there is no session to type into — keystrokes
    /// dropped into nothing would otherwise read as success.
    fn send_keys(&self, session: &dyn Session, text: &str, key: &str) -> Result<()> {
        let name = session_name(session);
        if !adi_pty::is_running(&name) {
            return Err(Error::NotRunning(session.agent().to_string()));
        }
        adi_pty::send_keys(&name, text, key).map_err(|e| match e {
            adi_pty::Error::InvalidKey(key) => Error::InvalidKey(key),
            other => Error::Session(other.to_string()),
        })
    }

    /// The visible screen. Still readable after the engine exits — a session whose child is gone
    /// keeps its last frame until it is stopped or relaunched, which is exactly when somebody wants
    /// to see what it said.
    fn capture(&self, session: &dyn Session) -> Option<String> {
        adi_pty::capture(&session_name(session))
    }
}

/// What this runner keeps about a session: the name of the pty it opened. The session id would not
/// do — a pty session name is a flat token shared with everything else that lists `adi-agent-*`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct State {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    session: Option<String>,
}

impl State {
    fn read(session: &dyn Session) -> Self {
        session
            .state()
            .and_then(|value| serde_json::from_value(value).ok())
            .unwrap_or_default()
    }

    fn into_value(self) -> Value {
        serde_json::to_value(self).unwrap_or(Value::Null)
    }
}

/// The pty session a run drives.
///
/// Recorded in the state slot at launch and read back from there, so the name that was actually
/// opened is the name that is later typed into, captured, and killed. The derived form is the
/// fallback for a session no runner has started yet.
fn session_name(session: &dyn Session) -> String {
    State::read(session)
        .session
        .map(|name| name.trim().to_string())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| pty::session_name(session.agent()))
}

/// Wait up to `budget` for the session's child to exit. Returns whether it did.
fn wait_for_exit(name: &str, budget: Duration) -> bool {
    let deadline = Instant::now() + budget;
    loop {
        if !adi_pty::is_running(name) {
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
    use std::path::{Path, PathBuf};
    use std::sync::Mutex;

    use adi_tools::ToolHelp;
    use serde_json::json;

    use super::*;

    /// The same four answers a runner asks a session for, and nothing else — see the note on the
    /// detached runner's copy for why these tests do not reach for the real store.
    #[derive(Debug)]
    struct FakeSession {
        id: String,
        agent: String,
        state: Mutex<Option<Value>>,
        log: PathBuf,
    }

    impl FakeSession {
        fn new(agent: &str) -> Self {
            Self {
                id: format!("sess-{agent}"),
                agent: agent.to_string(),
                state: Mutex::new(None),
                log: std::env::temp_dir().join(format!("adi-runner-pty-{agent}.log")),
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
            false
        }
        fn state(&self) -> Option<Value> {
            self.state.lock().unwrap().clone()
        }
        fn set_state(&self, value: Value) -> Result<()> {
            *self.state.lock().unwrap() = Some(value);
            Ok(())
        }
        fn log_path(&self) -> &Path {
            &self.log
        }
    }

    fn spec(arguments: Value) -> RunSpec {
        RunSpec {
            cwd: std::env::temp_dir(),
            path: "/usr/bin:/bin".to_string(),
            env: Vec::new(),
            arguments,
            tools: Vec::new(),
            tool_help: None,
            system_prompt: None,
            workspace_note: None,
            knowledge_note: None,
        }
    }

    /// A pty session name unique to this test run, so concurrently running tests (the registry is
    /// process-global) never collide.
    fn unique(tag: &str) -> String {
        format!("{tag}-{}-{:?}", std::process::id(), std::thread::current().id())
            .replace(['(', ')', '#'], "-")
    }

    /// The capability question a caller is supposed to ask, and the two answers that make
    /// `as_terminal()` worth having.
    #[test]
    fn a_pty_is_a_terminal_and_reports_no_events() {
        let runner = PtyRunner::new(Backend::PtyClaude);
        assert_eq!(runner.kind().as_str(), "pty");
        assert!(runner.as_terminal().is_some());

        assert!(!runner.emits().any(), "a tty has nothing structured to read");
        let session = FakeSession::new("solver");
        let batch = runner.events(&session, None).expect("events");
        assert!(batch.events.is_empty());
        // The cursor is handed back untouched — there is nothing to be after.
        let batch = runner
            .events(&session, Some(&json!({ "offset": 7 })))
            .expect("events");
        assert_eq!(batch.cursor, json!({ "offset": 7 }));
    }

    #[test]
    fn check_parses_each_engines_own_arguments() {
        let claude = PtyRunner::new(Backend::PtyClaude);
        assert!(claude.check(&spec(json!({ "model": "opus" }))).is_ok());
        assert!(claude.check(&spec(Value::Null)).is_ok());
        // `output_format` is a process-only knob; a pty agent carrying one is a save-time mistake.
        assert!(matches!(
            claude.check(&spec(json!({ "output_format": "json" }))),
            Err(Error::Arguments(_))
        ));
        assert!(matches!(
            PtyRunner::new(Backend::ProcessClaude).check(&spec(Value::Null)),
            Err(Error::NotRunnable(_))
        ));
    }

    /// The same prompt rule as every other runner: Claude takes the tool help, Codex must not,
    /// because its prompt is really its opening user turn.
    #[test]
    fn tool_help_reaches_claudes_system_prompt_and_never_codexs() {
        let mut spec = spec(json!({}));
        spec.system_prompt = Some("You are a solver.".to_string());
        spec.tools = vec![ToolHelp {
            name: "adi-db".to_string(),
            description: None,
            help: Some("Usage: adi-db query <SQL>".to_string()),
        }];

        let claude = PtyRunner::new(Backend::PtyClaude)
            .argv(&spec, &FakeSession::new("solver"))
            .expect("argv");
        let at = claude
            .iter()
            .position(|arg| arg == "--append-system-prompt")
            .expect("claude takes a system prompt");
        assert!(claude[at + 1].starts_with("You are a solver."));
        assert!(claude[at + 1].contains("Usage: adi-db query <SQL>"));

        let codex = PtyRunner::new(Backend::PtyCodex)
            .argv(&spec, &FakeSession::new("solver"))
            .expect("argv");
        assert!(codex.iter().all(|arg| !arg.contains("adi-db")), "{codex:?}");
        assert_eq!(codex.last().map(String::as_str), Some("You are a solver."));
        // Codex is told the run's directory, because it is what scopes its sandbox.
        let cd = codex.iter().position(|arg| arg == "--cd").expect("--cd");
        assert_eq!(codex[cd + 1], spec.cwd.display().to_string());
    }

    /// A pane gets the same tool surface a headless run does: ADI's shell in, the engine's own out.
    /// Codex is untouched — it reads none of these flags.
    #[test]
    fn a_claude_pane_is_launched_with_our_tools_and_without_the_engines_shell() {
        let spec = spec(json!({}));
        let claude = PtyRunner::new(Backend::PtyClaude)
            .argv(&spec, &FakeSession::new("paned"))
            .expect("argv");
        let at = claude
            .iter()
            .position(|arg| arg == "--mcp-config")
            .unwrap_or_else(|| panic!("a pane takes an mcp config: {claude:?}"));
        let config: Value = serde_json::from_str(&claude[at + 1]).expect("mcp config is json");
        assert_eq!(
            config["mcpServers"][crate::backends::mcp::SERVER]["args"][0],
            "mcp"
        );
        assert!(
            claude.iter().any(|arg| arg == "--strict-mcp-config"),
            "{claude:?}"
        );

        let flag = |name: &str| -> String {
            let at = claude
                .iter()
                .position(|arg| arg == name)
                .unwrap_or_else(|| panic!("{name} is set: {claude:?}"));
            claude[at + 1].clone()
        };
        assert!(flag("--allowed-tools").split(',').any(|t| t == "mcp__adi"));
        // Deny-by-default: this agent named no tools, so the engine's own built-in set is empty.
        assert_eq!(flag("--tools"), "");

        let codex = PtyRunner::new(Backend::PtyCodex)
            .argv(&spec, &FakeSession::new("paned"))
            .expect("argv");
        assert!(
            codex.iter().all(|arg| arg != "--mcp-config"),
            "codex reads no such flag: {codex:?}"
        );
    }

    /// The state slot is the truth about which pane this run drives; the agent's name is only the
    /// fallback for one that has never been launched.
    #[test]
    fn the_pty_session_name_is_derived_then_remembered() {
        // A name with a dot in it is still one flat token on the pty side.
        let session = FakeSession::new("a.b");
        assert_eq!(session_name(&session), "adi-agent-a-b");

        session
            .set_state(json!({ "session": "adi-agent-elsewhere" }))
            .expect("state");
        assert_eq!(session_name(&session), "adi-agent-elsewhere");
    }

    #[test]
    fn stopping_a_session_that_was_never_opened_is_not_a_stop() {
        let runner = PtyRunner::new(Backend::PtyClaude);
        let session = FakeSession::new("never-opened");
        assert!(!runner.is_alive(&session));
        assert_eq!(
            runner.stop(&session, Duration::ZERO).expect("stop"),
            Stopped::default()
        );
        // With no pane there is nothing to type into, and saying so beats dropping the keystrokes.
        assert!(matches!(
            runner.as_terminal().expect("a terminal").send_keys(&session, "hi", "Enter"),
            Err(Error::NotRunning(_))
        ));
    }

    /// The terminal half, against a real pty. `/bin/cat` stands in for a vendor CLI: it echoes what
    /// is typed (so the screen proves the keys landed) and dies on `C-c` (so the cooperative stop
    /// is a real one rather than a kill dressed up as one).
    #[cfg(unix)]
    #[test]
    fn typing_into_a_live_pane_shows_up_on_its_screen() {
        let runner = PtyRunner::new(Backend::PtyClaude);
        let session = FakeSession::new("echoer");
        let name = unique("adi-agent-echoer");
        session
            .set_state(json!({ "session": name }))
            .expect("state");
        adi_pty::launch(
            &name,
            &["/bin/cat".to_string()],
            &std::env::temp_dir(),
            &[],
        )
        .expect("launch a pane");
        assert!(runner.is_alive(&session));

        let terminal = runner.as_terminal().expect("a pty is a terminal");
        terminal
            .send_keys(&session, "hello pane", SUBMIT)
            .expect("send keys");
        let mut screen = String::new();
        for _ in 0..100 {
            screen = terminal.capture(&session).unwrap_or_default();
            if screen.contains("hello pane") {
                break;
            }
            std::thread::sleep(POLL);
        }
        assert!(screen.contains("hello pane"), "{screen:?}");

        // C-c reaches `cat`, so the stop is cooperative and never escalates.
        let stopped = runner
            .stop(&session, Duration::from_secs(5))
            .expect("stop");
        assert_eq!(
            stopped,
            Stopped {
                was_running: true,
                forced: false
            }
        );
        assert!(!runner.is_alive(&session));
        // The pane it left behind is still readable — that is the point of not clearing it.
        assert!(terminal.capture(&session).is_some());

        let _ = adi_pty::stop(&name);
    }

    /// A pane that will not take the hint is closed under it, and `forced` says so.
    #[cfg(unix)]
    #[test]
    fn a_pane_that_ignores_the_interrupt_is_closed() {
        let runner = PtyRunner::new(Backend::PtyClaude);
        let session = FakeSession::new("deaf-pane");
        let name = unique("adi-agent-deaf");
        session
            .set_state(json!({ "session": name }))
            .expect("state");
        adi_pty::launch(
            &name,
            &[
                "/bin/sh".to_string(),
                "-c".to_string(),
                // The shell ignores the interrupt and the loop outlives the `sleep` that each one
                // kills — a pane that genuinely will not take the hint. It announces itself first,
                // because an interrupt that arrives before `trap` runs would kill it for real and
                // this test would pass by accident.
                "trap '' INT; echo ready; while :; do sleep 1; done".to_string(),
            ],
            &std::env::temp_dir(),
            &[],
        )
        .expect("launch a pane");
        let mut ready = false;
        for _ in 0..200 {
            ready = adi_pty::capture(&name).is_some_and(|screen| screen.contains("ready"));
            if ready {
                break;
            }
            std::thread::sleep(POLL);
        }
        assert!(ready, "the pane never came up");

        let stopped = runner
            .stop(&session, Duration::from_millis(200))
            .expect("stop");
        assert_eq!(
            stopped,
            Stopped {
                was_running: true,
                forced: true
            }
        );
        assert!(!runner.is_alive(&session));
        assert_eq!(
            runner.stop(&session, Duration::ZERO).expect("stop"),
            Stopped::default(),
            "stopping it again finds nothing running",
        );
    }
}
