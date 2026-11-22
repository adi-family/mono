//! The interpreters a conversation keeps between `PyCode` and `TsCode` calls.
//!
//! `Bash` carries a conversation's *location* from call to call — a directory and the variables it
//! exported (see [`super::shell`]). Everything else a command worked out is gone the moment it
//! returns, so an agent that parsed a file, fetched a listing or built a table has two choices:
//! print all of it into the transcript, or do the work again next call. A code tool is the third:
//! its interpreter outlives the call, so what a cell computed is still in a variable when the next
//! cell runs.
//!
//! There are two, and they are one design in two languages ([`Language`]): Python under
//! `python3`, and TypeScript under `bun` — the one JavaScript runtime the platform itself puts on
//! every machine (`adi_core::bun`). Everything in this file is common to both; what differs is the
//! interpreter each embeds (`pycode.py`, `tscode.ts`) and a handful of words in a message.
//!
//! # A process that outlives whoever started it
//!
//! Whoever serves a tool call is short-lived — the adi loop's turn child, or the MCP server an
//! engine's CLI spawned (the same fact [`super::jobs`] is built around). An interpreter held by
//! either would die with the turn. So it is a detached process of its own, one per conversation
//! and language, reached over a Unix socket: each call connects, sends one request, reads one
//! reply.
//!
//! A few files in the session's own `<id>.*` namespace, where the session's deletion sweeps them
//! without this module being told (`py` below is `ts` for the other language):
//!
//! - `<id>.py-kernel` — who is serving: a pid and where its socket is. The socket itself lives in
//!   a private temp directory, because a socket path has about a hundred bytes to fit in and a
//!   session directory is often longer than that on its own.
//! - `<id>.pycode.py` — the interpreter's own source, written from this binary.
//! - `<id>.shared.<name>.json` — what a cell handed to `share(name, value)`: a value kept as JSON
//!   so that it can be read where a variable cannot reach — by a background cell, by the other
//!   language's interpreter, and by this one's successor when it is restarted or lost. One file
//!   per name rather than a directory, because the session's sweep removes files.
//! - `<id>.py-kernel.log` — the interpreter process's own stdout and stderr. For Python that is
//!   only what it says when it will not start; the TypeScript interpreter also reads what its
//!   cells printed back out of it, because a JavaScript process cannot point its own stdout
//!   somewhere else the way a Python one can.
//!
//! The state file is also how the interpreter is told to stop. It checks every couple of seconds
//! that the file still names it: gone means the session was deleted, another pid means it was
//! replaced, and either way it exits. It also exits after [`IDLE_SECONDS`] without a call, leaving
//! a tombstone so the next call can say the interpreter it found is a new one.
//!
//! # One conversation state, however many tools
//!
//! An interpreter reads and writes the *same* two files `Bash` carries its state in. Before each
//! cell it stands where the conversation stands — the caller's launch environment, the exported
//! variables on top, the recorded directory — and afterwards it records where the cell left
//! things. So a `chdir` in either language moves `Bash` too, `export` in `Bash` shows up in both
//! environments, and a variable the runner refreshes between turns reaches a cell as the runner
//! meant it, exactly as it reaches a command. A second, private notion of "where am I" is what
//! this avoids: the tools are three languages over one conversation, not three conversations.
//!
//! Only variables, imports and definitions are an interpreter's alone — a Python name is not a
//! TypeScript one — and they are what is lost when it goes. A value that has to cross either
//! line is handed over on purpose, with `share`, as JSON; nothing is synchronized behind a cell's
//! back, because a variable that changes because another language assigned it is not a variable. That is said to the model when it
//! happens ([`Ran::replaced`]) rather than left to be discovered as a `NameError`.
//!
//! # Stopping a cell
//!
//! The interpreter enforces the call's own deadline, and does the same when the caller hangs up,
//! so a cancelled turn does not leave a cell holding it. How much that can do is the one real
//! difference between the two. Python is interrupted where it stands — a `KeyboardInterrupt` —
//! which leaves its variables intact whatever it was doing. Running JavaScript cannot be
//! interrupted from inside its own process, so the TypeScript interpreter stops the *commands* the
//! cell is waiting on, which ends most cells that are slow; one that is waiting on something else
//! is not going to stop, and the interpreter says so and exits ([`Ended`] has no case for it — it
//! is an error, because the conversation's variables went with it).
//!
//! This side is only the backstop for a cell that never even lets its interpreter notice the
//! deadline (a C loop, a `while (true) {}`): it is given [`STOP_GRACE`] and then the interpreter
//! is killed.
//!
//! Unix only, like the session shell: the socket, the signals and the process group are all
//! things Windows spells differently, and an untested second spelling would be worse than none.
//! The tools are simply not declared there.

use std::io::{Read as _, Write as _};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use super::shell::Shell;

/// What both halves were built against — the wire, and what an interpreter does with a cell. An
/// interpreter started by an older binary announces an older number and is replaced rather than
/// spoken to in a dialect it does not have.
///
/// 2: a Python cell shows its last expression rather than every one, and both interpreters have
/// `share`/`shared`. Nothing on the wire moved, but a conversation whose interpreter outlived an
/// upgrade would otherwise keep the old behaviour under a description of the new one.
///
/// 3: a request may carry `tools` — where the cell's tool functions are answered, for a code-only
/// agent (see [`super::code_only`]). An interpreter that predates it would run such a cell with
/// none of the functions its description promises.
///
/// 4: in TypeScript `$` is the conversation's shell — a `cd` or an `export` in it stays, and
/// what is interpolated is passed as data — where it used to be Bun's. Nothing on the wire moved;
/// like 2, it is the description that would otherwise be about an interpreter that is not there.
///
/// 5: a code-only agent's functions describe themselves (`tools()`, what each says when asked
/// what it is), which its instructions now tell it to rely on; and a command that printed
/// nothing shows nothing.
const PROTOCOL: u64 = 5;

/// How long an interpreter nobody calls stays up. An hour is a conversation still in progress —
/// a build awaited, a person answering — while anything longer is most likely a conversation that
/// is over and a process holding memory for nobody.
const IDLE_SECONDS: u64 = 3600;

/// Overrides [`IDLE_SECONDS`], for a deployment that wants interpreters kept longer or shorter.
const IDLE_ENV: &str = "ADI_CODE_IDLE_SECONDS";

/// How long a new interpreter is given to come up. Its start is an import of a standard library
/// and a `bind`, tens of milliseconds warm — this is the budget for a cold disk, not an
/// expectation.
const START_WAIT: Duration = Duration::from_secs(20);

/// How long past its deadline a cell is given to stop on its own before the interpreter is killed.
/// The interpreter acts at the deadline and needs a moment to kill what the cell was running. Past
/// this it is not going to stop.
const STOP_GRACE: Duration = Duration::from_secs(15);

/// The longest one read of a reply waits before the deadline is looked at again.
const READ_SLICE: Duration = Duration::from_secs(1);

/// What every way of losing an interpreter leaves true, said once.
const GONE: &str = "every variable and import it held is gone, and your next call starts a new \
                    one. Whatever the code had already changed on disk stays changed.";

/// Which language an interpreter speaks — and so which tool it is behind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Language {
    Python,
    TypeScript,
}

impl Language {
    /// The language, as a sentence names it.
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Python => "Python",
            Self::TypeScript => "TypeScript",
        }
    }

    /// The two letters that tell this language's sidecars from the other's.
    fn tag(self) -> &'static str {
        match self {
            Self::Python => "py",
            Self::TypeScript => "ts",
        }
    }

    /// The interpreter, as shipped inside this binary.
    fn source(self) -> &'static str {
        match self {
            Self::Python => include_str!("pycode.py"),
            Self::TypeScript => include_str!("tscode.ts"),
        }
    }

    /// What that source is called on disk. The extension is how `bun` knows what it is reading.
    fn script(self) -> &'static str {
        match self {
            Self::Python => "pycode.py",
            Self::TypeScript => "tscode.ts",
        }
    }

    /// The environment variable naming the program to run instead of the first one on `PATH`.
    fn program_env(self) -> &'static str {
        match self {
            Self::Python => "ADI_PYCODE_PYTHON",
            Self::TypeScript => "ADI_TSCODE_BUN",
        }
    }

    /// Which program runs the interpreter.
    fn program(self) -> String {
        std::env::var(self.program_env())
            .ok()
            .map(|named| named.trim().to_string())
            .filter(|named| !named.is_empty())
            .unwrap_or_else(|| {
                match self {
                    Self::Python => "python3",
                    Self::TypeScript => "bun",
                }
                .to_string()
            })
    }

    /// What to tell the model when that program is not on the machine.
    fn missing(self, program: &str) -> String {
        match self {
            Self::Python => format!(
                "PyCode needs a Python 3 interpreter and there is no `{program}` on this \
                 machine's PATH. Use Bash instead."
            ),
            Self::TypeScript => format!(
                "TsCode runs on Bun and there is no `{program}` on this machine's PATH. Use Bash \
                 instead."
            ),
        }
    }
}

/// One conversation's interpreter for one language, as the files that say where it is.
#[derive(Debug)]
pub(crate) struct Interpreter {
    language: Language,
    /// Who is serving, or a tombstone saying nobody is any more.
    state: PathBuf,
    /// The interpreter's source, on disk where its program can run it.
    script: PathBuf,
    /// The interpreter process's own stdout and stderr.
    log: PathBuf,
    /// The session directory, for the scripts background cells are written to.
    dir: PathBuf,
    /// The session id, the stem of everything above.
    id: String,
}

impl Interpreter {
    /// The path prefix `share(name, value)` writes under: `<dir>/<id>.shared.`, to which the
    /// interpreter adds the name and `.json`. The same for both languages, which is the point.
    fn shared_prefix(&self) -> PathBuf {
        self.dir.join(format!("{}.shared.", self.id))
    }
}

/// How a cell ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Ended {
    /// Ran to its last line.
    Completed,
    /// An exception got out. The traceback is the end of the output.
    Raised,
    /// Stopped at the call's deadline. The interpreter, and what it held, are intact.
    TimedOut,
    /// Stopped some other way — the caller of an earlier cell went away mid-run.
    Interrupted,
}

/// What one call produced.
#[derive(Debug)]
pub(crate) struct Ran {
    /// What the cell printed, already cut to the size a result may be.
    pub(crate) output: String,
    pub(crate) ended: Ended,
    /// Whether this call found the conversation's interpreter gone and ran in a new one — so every
    /// variable the model believes it has is not there. Never set on a conversation's first call,
    /// nor after a restart it asked for: both are new interpreters nobody is surprised by.
    pub(crate) replaced: bool,
}

/// A reachable interpreter: an open connection to it, and what killing it needs.
struct Live {
    stream: UnixStream,
    pid: u32,
    socket: PathBuf,
}

impl Interpreter {
    /// The interpreter belonging to one session, as sidecars of that session's files — the same
    /// keying and the same naming rule as [`Shell::new`], for the same reasons.
    pub(crate) fn new(language: Language, agent_dir: &Path, session_id: &str) -> Self {
        let tag = language.tag();
        Self {
            language,
            state: agent_dir.join(format!("{session_id}.{tag}-kernel")),
            script: agent_dir.join(format!("{session_id}.{}", language.script())),
            log: agent_dir.join(format!("{session_id}.{tag}-kernel.log")),
            dir: agent_dir.to_path_buf(),
            id: session_id.to_string(),
        }
    }

    /// Run `code` in the conversation's interpreter, starting one if there is none.
    ///
    /// `home` is the run's own directory, where a conversation that has not moved stands.
    /// `max_output` is how many bytes of what the cell printed may come back. `tools` is what a
    /// code-only agent's cell is told about the functions it may call — where they are answered,
    /// and what they are called (see [`super::code_only::serving`]); `None` for every other
    /// agent, whose cells have none.
    ///
    /// # Errors
    /// A message for the model when there is no interpreter to be had (its program is not on the
    /// machine, or it would not start), or when the one that ran the cell had to be ended or died
    /// under it. A cell that *raised* is not an error here — it ran, and [`Ran::ended`] says how
    /// it went.
    #[allow(clippy::too_many_arguments)] // one cell's whole request, each part from somewhere else
    pub(crate) fn run(
        &self,
        shell: &Shell,
        home: &Path,
        code: &str,
        timeout_ms: u64,
        max_output: usize,
        restart: bool,
        tools: Option<&Value>,
    ) -> Result<Ran, String> {
        let name = self.language.name();
        // Asked before anything is started or stopped: a state file with nobody behind it is the
        // one sign that this conversation had an interpreter and lost it.
        let had = self.state.exists();
        if restart {
            self.stop();
        }
        let (mut live, started) = match self.reach() {
            Some(live) => (live, false),
            None => (self.start(shell, home)?, true),
        };
        let replaced = started && had && !restart;

        let mut request = json!({
            "code": code,
            "timeout_ms": timeout_ms,
            "max_output": max_output,
            "env": launch_environment(),
        });
        if let Some(tools) = tools {
            request["tools"] = tools.clone();
        }
        let sent = writeln!(live.stream, "{request}").and_then(|()| live.stream.flush());
        if let Err(e) = sent {
            return Err(format!("couldn't reach the {name} interpreter: {e}"));
        }

        let deadline = Instant::now() + Duration::from_millis(timeout_ms) + STOP_GRACE;
        let reply = match read_reply(&mut live.stream, deadline) {
            Ok(reply) => reply,
            Err(Unanswered::TimedOut) => {
                kill(&live);
                return Err(format!(
                    "the code was still running after {timeout_ms}ms and could not be \
                     interrupted, so its interpreter was killed — {GONE} Set `background` for \
                     work that long."
                ));
            }
            Err(Unanswered::Gone(why)) => {
                return Err(format!(
                    "the {name} interpreter went away while running this ({why}) — it exited or \
                     was killed, so {GONE}"
                ));
            }
        };

        let output = reply
            .get("output")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let ended = match (
            reply.get("status").and_then(Value::as_str),
            reply.get("stopped_by").and_then(Value::as_str),
        ) {
            (Some("ok"), _) => Ended::Completed,
            (Some("interrupted"), Some("timeout")) => Ended::TimedOut,
            (Some("interrupted"), _) => Ended::Interrupted,
            // The interpreter's own word for a cell it could not stop: it has said so and gone.
            // What the cell printed up to then is the only account of what it was doing.
            (Some("stuck"), _) => {
                // It is on its way out by itself. Made certain here, so that a call arriving in
                // the next instant starts a new interpreter instead of reaching a dying one.
                kill(&live);
                let printed = output.trim_end();
                let printed = if printed.is_empty() {
                    String::new()
                } else {
                    format!("{printed}\n\n")
                };
                return Err(format!(
                    "{printed}the code was still running after {timeout_ms}ms and was not \
                     waiting on a command that could be stopped — running {name} cannot be \
                     interrupted from outside — so its interpreter was ended: {GONE} Set \
                     `background` for work that long."
                ));
            }
            _ => Ended::Raised,
        };
        Ok(Ran {
            output,
            ended,
            replaced,
        })
    }

    /// The shell command that runs `code` as a script of its own — what a background cell hands
    /// to [`super::jobs`].
    ///
    /// A fresh interpreter, deliberately, and for [`Shell::inherit`]'s reason: a job outlives the
    /// call that started it, and the conversation's interpreter runs one cell at a time, so a job
    /// inside it would hold every later call behind a build. It gets the same helpers and the same
    /// echoing of top-level expressions; it does not get the conversation's variables, and the
    /// tool's description says so.
    ///
    /// `absent` names the tool functions the script does *not* have — a code-only agent's, which
    /// exist only while a cell runs (see [`super::code_only`]). They are named so that calling
    /// one says why it is not there, rather than that nothing by that name was ever defined.
    ///
    /// # Errors
    /// A message for the model if the script cannot be written.
    pub(crate) fn job_command(&self, code: &str, absent: &[&str]) -> Result<String, String> {
        self.install()
            .map_err(|e| format!("couldn't write the interpreter's source: {e}"))?;
        let tag = self.language.tag();
        let cell = self
            .dir
            .join(format!("{}.{tag}job-{}.{tag}", self.id, stamp()));
        std::fs::write(&cell, code)
            .map_err(|e| format!("couldn't write {}: {e}", cell.display()))?;
        let program = quote(&self.language.program());
        let script = quote(&self.script.display().to_string());
        let cell = quote(&cell.display().to_string());
        // A job cannot see the conversation's variables, so what it shares is how its result
        // gets back: it is given the same place to put it.
        let shared = quote(&self.shared_prefix().display().to_string());
        let absent = if absent.is_empty() {
            String::new()
        } else {
            format!(" --absent {}", quote(&absent.join(",")))
        };
        Ok(match self.language {
            // `-u`: a job's output is a log somebody may read while it runs, so it is written
            // through as it happens rather than when a buffer fills.
            Language::Python => {
                format!("exec {program} -u {script} run {cell} --shared {shared}{absent}")
            }
            Language::TypeScript => {
                format!("exec {program} {script} run {cell} --shared {shared}{absent}")
            }
        })
    }

    /// End this conversation's interpreter, if it has one, and forget it was there.
    pub(crate) fn stop(&self) {
        if let Some(live) = self.reach() {
            kill(&live);
        }
        let _ = std::fs::remove_file(&self.state);
    }

    /// Connect to the interpreter the state file names, if it is there and speaks this binary's
    /// protocol.
    ///
    /// A successful connect is also what makes the pid safe to signal later: the socket sits in a
    /// directory only this user can enter and is bound by that process, so somebody answering on
    /// it means that process is alive — which a pid read from a file, on its own, never proves.
    fn reach(&self) -> Option<Live> {
        let text = std::fs::read_to_string(&self.state).ok()?;
        let record: Value = serde_json::from_str(&text).ok()?;
        let socket = PathBuf::from(record.get("socket")?.as_str()?);
        let pid = u32::try_from(record.get("pid")?.as_u64()?).ok()?;
        let stream = UnixStream::connect(&socket).ok()?;
        // Bounded from the first read on, and set here rather than only where the reply is read:
        // by then the interpreter may have answered and hung up, and a socket whose peer is gone
        // refuses to have an option set at all (see `read_reply`).
        let _ = stream.set_read_timeout(Some(READ_SLICE));
        let live = Live {
            stream,
            pid,
            socket,
        };
        if record.get("protocol").and_then(Value::as_u64) != Some(PROTOCOL) {
            // Started by another build of this binary. Its variables are lost either way; ending
            // it here is what keeps two interpreters from serving one conversation.
            kill(&live);
            return None;
        }
        Some(live)
    }

    /// Write the interpreter's source where its program can run it, unless that is already what
    /// is there.
    fn install(&self) -> std::io::Result<()> {
        std::fs::create_dir_all(&self.dir)?;
        let source = self.language.source();
        if std::fs::read_to_string(&self.script).is_ok_and(|have| have == source) {
            return Ok(());
        }
        std::fs::write(&self.script, source)
    }

    /// Start an interpreter and wait until it answers.
    fn start(&self, shell: &Shell, home: &Path) -> Result<Live, String> {
        let name = self.language.name();
        self.install()
            .map_err(|e| format!("couldn't write the interpreter's source: {e}"))?;
        // Whatever was recorded is about an interpreter that is not there. Removed before the new
        // one starts, so the file that appears next can only be the new one's.
        let _ = std::fs::remove_file(&self.state);

        // Emptied, not appended to: the TypeScript interpreter reads what its cells printed back
        // out of this file, and what an interpreter that is gone left in it is nobody's output.
        let open_log = || {
            std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&self.log)
        };
        let _ = std::fs::write(&self.log, "");
        let (out, err) = open_log()
            .and_then(|out| Ok((out.try_clone()?, out)))
            .map_err(|e| format!("couldn't open {}: {e}", self.log.display()))?;

        let program = self.language.program();
        let (cwd_file, env_file) = shell.files();
        let idle = std::env::var(IDLE_ENV)
            .ok()
            .and_then(|v| v.trim().parse::<u64>().ok())
            .unwrap_or(IDLE_SECONDS);
        let mut cmd = Command::new(&program);
        cmd.arg(&self.script)
            .arg("kernel")
            .arg("--state")
            .arg(&self.state)
            .arg("--capture")
            .arg(&self.log)
            .arg("--shell-cwd")
            .arg(cwd_file)
            .arg("--shell-env")
            .arg(env_file)
            .arg("--shared")
            .arg(self.shared_prefix())
            .arg("--home")
            .arg(home)
            .arg("--idle")
            .arg(idle.to_string())
            .arg("--protocol")
            .arg(PROTOCOL.to_string())
            // Started in the session's own directory rather than the run's: a runtime reads its
            // configuration from where it starts (`bunfig.toml`, to begin with), and a project's
            // preload script has no business inside the interpreter. Every cell is moved to where
            // the conversation stands before it runs.
            .current_dir(&self.dir)
            .stdin(Stdio::null())
            .stdout(Stdio::from(out))
            .stderr(Stdio::from(err));
        // Its own process group, for the reason a job gets one: the process starting it is about
        // to end, and a signal to that process must not take the interpreter with it.
        adi_osext::detach_process_group(&mut cmd);

        let mut child = cmd.spawn().map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                self.language.missing(&program)
            } else {
                format!("couldn't start `{program}`: {e}")
            }
        })?;

        let deadline = Instant::now() + START_WAIT;
        loop {
            if let Some(live) = self.reach() {
                // Reaped on a thread of its own for as long as this process lasts, as a job is:
                // an unreaped child is a zombie, and a zombie's socket is gone but its pid is not.
                std::thread::spawn(move || {
                    let _ = child.wait();
                });
                return Ok(live);
            }
            if let Ok(Some(status)) = child.try_wait() {
                return Err(format!(
                    "the {name} interpreter exited as it started ({status}). Use Bash instead.\n{}",
                    tail(&self.log)
                ));
            }
            if Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!(
                    "the {name} interpreter did not come up within {}s. Use Bash instead.\n{}",
                    START_WAIT.as_secs(),
                    tail(&self.log)
                ));
            }
            std::thread::sleep(Duration::from_millis(15));
        }
    }
}

/// Why a call got no reply.
enum Unanswered {
    /// The deadline, and the grace after it, passed.
    TimedOut,
    /// The connection ended or broke first.
    Gone(String),
}

/// Read the interpreter's one reply — a line of JSON — or give up at `deadline`.
fn read_reply(stream: &mut UnixStream, deadline: Instant) -> Result<Value, Unanswered> {
    let mut reply = Vec::new();
    let mut chunk = [0_u8; 8192];
    while !reply.ends_with(b"\n") {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Err(Unanswered::TimedOut);
        }
        // Never zero: a zero read timeout means "block for ever" to the socket API, which is the
        // one thing this loop exists to rule out.
        let wait = left.clamp(Duration::from_millis(1), READ_SLICE);
        // Not an error when it fails. A peer that has already answered and closed makes this
        // `EINVAL` on macOS, with its whole reply sitting unread in the buffer — and calling
        // that "the interpreter went away" threw away the result of a cell that had run. The
        // read below is what finds out how things stand; the timeout set at `reach` still
        // bounds it.
        let _ = stream.set_read_timeout(Some(wait));
        match stream.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => reply.extend_from_slice(&chunk[..n]),
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::WouldBlock
                        | std::io::ErrorKind::TimedOut
                        | std::io::ErrorKind::Interrupted
                ) => {}
            Err(e) => return Err(Unanswered::Gone(e.to_string())),
        }
    }
    serde_json::from_slice(&reply)
        .map_err(|_| Unanswered::Gone("it closed the connection without answering".to_string()))
}

/// Kill an interpreter outright, and clear away the socket it can no longer clear away itself.
///
/// The whole process group, which the interpreter leads: threads and anything it forked directly
/// go with it. A command started with `sh()` leads a session of its own so that an interrupt can
/// end exactly that command; one still running when its interpreter is killed is left to finish.
fn kill(live: &Live) {
    if let Ok(pid) = i32::try_from(live.pid)
        && pid > 1
    {
        // SAFETY: `kill(2)` takes two integers and touches no memory of ours. The pid is one whose
        // owner answered on its own socket a moment ago (see `Interpreter::reach`), negated to
        // name the group it leads.
        #[allow(unsafe_code)]
        unsafe {
            libc::kill(-pid, libc::SIGKILL);
        }
    }
    let _ = std::fs::remove_file(&live.socket);
    if let Some(dir) = live.socket.parent() {
        let _ = std::fs::remove_dir(dir);
    }
}

/// The environment this process was launched with, as the interpreter is to see it for this call.
///
/// Sent with every request rather than inherited once at start, which is the difference between
/// an interpreter that knows a secret rotated between turns and one still holding the value from
/// the turn that happened to start it.
fn launch_environment() -> Value {
    Value::Object(
        std::env::vars_os()
            .map(|(key, value)| {
                (
                    key.to_string_lossy().into_owned(),
                    Value::String(value.to_string_lossy().into_owned()),
                )
            })
            .collect(),
    )
}

/// The end of the interpreter's own log, for a failure to start that would otherwise say nothing.
fn tail(log: &Path) -> String {
    let text = std::fs::read_to_string(log).unwrap_or_default();
    let mut from = text.len().saturating_sub(2_000);
    while !text.is_char_boundary(from) {
        from += 1;
    }
    text[from..].trim().to_string()
}

/// A stem no two background cells of one conversation share.
fn stamp() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or_default();
    format!("{millis}-{:03}", SEQ.fetch_add(1, Ordering::Relaxed) % 1000)
}

/// `word` as one single-quoted `sh` word.
fn quote(word: &str) -> String {
    format!("'{}'", word.replace('\'', r"'\''"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A scratch session directory with one conversation's interpreter and shell in it, ended and
    /// swept when the test is.
    struct Conversation {
        home: PathBuf,
        shell: Shell,
        code: Interpreter,
    }

    impl Conversation {
        /// A conversation working in Python.
        fn new(tag: &str) -> Self {
            Self::of(Language::Python, tag)
        }

        /// A conversation working in TypeScript.
        fn ts(tag: &str) -> Self {
            Self::of(Language::TypeScript, tag)
        }

        fn of(language: Language, tag: &str) -> Self {
            let home = std::env::temp_dir().join(format!(
                "adi-code-{}-{tag}-{}",
                language.tag(),
                std::process::id()
            ));
            let _ = std::fs::remove_dir_all(&home);
            std::fs::create_dir_all(&home).expect("scratch dir");
            Self {
                shell: Shell::new(&home, "conv"),
                code: Interpreter::new(language, &home, "conv"),
                home,
            }
        }

        fn run(&self, code: &str) -> Ran {
            self.run_for(code, 30_000)
        }

        fn run_for(&self, code: &str, timeout_ms: u64) -> Ran {
            self.code
                .run(&self.shell, &self.home, code, timeout_ms, 32_000, false, None)
                .expect("the interpreter runs the cell")
        }

        /// A cell through the interpreter's background door: the command a job would run, run.
        fn job(&self, code: &str) -> std::process::Output {
            let command = self.code.job_command(code, &[]).expect("command");
            Command::new("sh")
                .arg("-c")
                .arg(command)
                .current_dir(&self.home)
                .output()
                .expect("sh")
        }

        /// One command through the conversation's shell, the way the `Bash` tool runs it.
        fn bash(&self, command: &str) -> String {
            let out = Command::new("sh")
                .arg("-c")
                .arg(self.shell.script(command))
                .current_dir(self.shell.start_dir(&self.home))
                .output()
                .expect("sh");
            String::from_utf8_lossy(&out.stdout).into_owned()
        }
    }

    impl Drop for Conversation {
        fn drop(&mut self) {
            self.code.stop();
            let _ = std::fs::remove_dir_all(&self.home);
        }
    }

    /// The whole point: what one call computed is there for the next, without being printed and
    /// without being computed again.
    #[test]
    fn a_variable_a_function_and_an_import_reach_the_next_call() {
        let conv = Conversation::new("carry");
        let first =
            conv.run("import math\nrows = [3, 4]\ndef hyp(a, b):\n    return math.hypot(a, b)");
        assert_eq!(first.ended, Ended::Completed);
        assert_eq!(
            first.output, "",
            "nothing was printed, so nothing comes back"
        );
        assert!(!first.replaced, "a first call is not a replacement");

        let second = conv.run("hyp(*rows)");
        assert_eq!(second.output.trim(), "5.0", "a bare expression is echoed");
        assert!(!second.replaced);
    }

    /// The last expression is shown whatever it is, and every bare command where it stands, in
    /// order with what is printed — but not the byte count, the popped item or the `True` that
    /// Python returns from things called for what they do. The first agent to use the tool for
    /// real is why: a `write_text` in the middle of its cell printed `418`.
    #[test]
    fn python_shows_the_last_expression_and_every_command_but_not_what_a_call_returned() {
        let conv = Conversation::new("echo");
        let ran = conv.run(
            "sh('echo one')\n\
             Path('note.txt').write_text('four')\n\
             [1, 2].pop()\n\
             print('two')\n\
             sh('echo three >&2')\n\
             x = sh('echo quiet')\n\
             len(x)",
        );
        assert_eq!(ran.output, "one\ntwo\nthree\n5\n");

        // A command on the last line is shown once, not twice; a statement there shows nothing.
        assert_eq!(conv.run("sh('echo last')").output, "last\n");
        assert_eq!(conv.run("y = 1 + 1").output, "");
        assert_eq!(conv.run("_").output, "5\n", "`_` is the last value shown");

        // A dict is shown in the order it is in. Sorted, the display would contradict the dict.
        assert_eq!(
            conv.run("{'numbers': [1, 2], 'sum': 3, 'mean': 1.5}")
                .output,
            "{'numbers': [1, 2], 'sum': 3, 'mean': 1.5}\n"
        );
    }

    /// A value handed over with `share` is there for whatever a variable cannot reach: the other
    /// language's interpreter, a background cell, and this language's own interpreter once the
    /// one that shared it is gone. Each direction, and a job's way of returning a result.
    #[test]
    fn a_shared_value_reaches_the_other_language_a_background_cell_and_a_new_interpreter() {
        let conv = Conversation::new("share");
        let ts = Interpreter::new(Language::TypeScript, &conv.home, "conv");
        let in_ts = |code: &str| {
            ts.run(&conv.shell, &conv.home, code, 30_000, 32_000, false, None)
                .expect("typescript")
        };

        // Python to TypeScript, as JSON: a tuple arrives as an array, and that is said nowhere
        // but here because it is what JSON is.
        let quiet = conv.run("share('totals', {'adi-agents': 59409, 'top': ('a', 'b')})");
        assert_eq!(quiet.output, "", "sharing is not something to print");
        assert_eq!(
            in_ts("const totals = shared('totals') as any;\nconsole.log(totals['adi-agents'], totals.top.join('+'));").output,
            "59409 a+b\n"
        );

        // TypeScript to Python.
        in_ts("share('deps', { shared: ['react', 'vite'], mismatched: 0 });");
        assert_eq!(
            conv.run("d = shared('deps')\n(d['shared'], d['mismatched'])")
                .output
                .trim(),
            "(['react', 'vite'], 0)"
        );
        assert_eq!(conv.run("shared()").output.trim(), "['deps', 'totals']");
        assert_eq!(in_ts("shared();").output.trim(), "[ \"deps\", \"totals\" ]");

        // A background cell sees no variables — what it shares is how its answer gets back.
        let job = conv.job(
            "share('from-job', {'seen': shared('deps')['shared'], 'has_d': 'd' in globals()})",
        );
        assert!(
            job.status.success(),
            "{}",
            String::from_utf8_lossy(&job.stderr)
        );
        assert_eq!(
            conv.run("shared('from-job')").output.trim(),
            "{'seen': ['react', 'vite'], 'has_d': False}"
        );
        let ts_job = Command::new("sh")
            .arg("-c")
            .arg(
                ts.job_command("share('from-ts-job', (shared('totals') as any).top);", &[])
                    .expect("command"),
            )
            .current_dir(&conv.home)
            .output()
            .expect("sh");
        assert!(
            ts_job.status.success(),
            "{}",
            String::from_utf8_lossy(&ts_job.stderr)
        );
        assert_eq!(
            conv.run("shared('from-ts-job')").output.trim(),
            "['a', 'b']"
        );

        // …and it outlives the interpreter that shared it, which a variable does not.
        let after = conv
            .code
            .run(
                &conv.shell,
                &conv.home,
                "('d' in globals(), shared('totals')['adi-agents'])",
                30_000,
                32_000,
                true,
                None,
            )
            .expect("run");
        assert_eq!(after.output.trim(), "(False, 59409)");

        // The values are the session's own files, so the session's sweep takes them.
        assert!(conv.home.join("conv.shared.totals.json").is_file());
        ts.stop();
    }

    /// What cannot be shared says so at the `share`, in terms of what to do about it — and a
    /// name nobody shared is answered with the names somebody did.
    #[test]
    fn sharing_what_json_cannot_hold_or_reading_what_nobody_shared_explains_itself() {
        let conv = Conversation::new("share-errors");
        let ts = Interpreter::new(Language::TypeScript, &conv.home, "conv");
        let in_ts = |code: &str| {
            ts.run(&conv.shell, &conv.home, code, 30_000, 32_000, false, None)
                .expect("typescript")
        };

        let refused = conv.run("share('s', {1, 2})");
        assert_eq!(refused.ended, Ended::Raised);
        assert!(
            refused
                .output
                .contains("TypeError: share('s'): only what JSON can hold"),
            "{}",
            refused.output
        );
        assert!(
            refused.output.contains("a set to a list"),
            "{}",
            refused.output
        );
        assert!(
            conv.run("share('../escape', 1)")
                .output
                .contains("ValueError: a shared name is")
        );

        let refused = in_ts("share('f', () => 1);");
        assert_eq!(refused.ended, Ended::Raised);
        assert!(
            refused.output.contains("only what JSON can hold"),
            "{}",
            refused.output
        );
        assert!(
            in_ts("share('../escape', 1);")
                .output
                .contains("a shared name is")
        );

        conv.run("share('kept', [1])");
        let missing = conv.run("shared('nope')");
        assert!(
            missing
                .output
                .contains("nothing is shared as 'nope'. What is: kept"),
            "{}",
            missing.output
        );
        let missing = in_ts("shared('nope');");
        assert!(
            missing
                .output
                .contains("nothing is shared as \"nope\". What is: kept"),
            "{}",
            missing.output
        );
        ts.stop();
    }

    /// An exception ends the cell, not the interpreter: the traceback names the model's own line,
    /// what ran before it stays done, and the next call still has its variables.
    #[test]
    fn an_exception_is_reported_and_the_interpreter_survives_it() {
        let conv = Conversation::new("raise");
        let ran = conv.run("kept = 7\nprint('before')\n1 / 0\nprint('never')");
        assert_eq!(ran.ended, Ended::Raised);
        assert!(ran.output.starts_with("before\n"), "{}", ran.output);
        assert!(ran.output.contains("line 3"), "{}", ran.output);
        assert!(ran.output.contains("ZeroDivisionError"), "{}", ran.output);
        assert!(!ran.output.contains("never"), "{}", ran.output);
        assert!(
            !ran.output.contains("pycode.py"),
            "the interpreter's own frames are not the model's business: {}",
            ran.output
        );

        assert_eq!(conv.run("kept").output.trim(), "7");
    }

    /// A failing command is an exception unless the cell said failing was an answer — so a script
    /// of several steps stops at the one that broke instead of running the rest on its wreckage.
    #[test]
    fn a_failing_command_raises_unless_told_it_may_fail() {
        let conv = Conversation::new("check");
        let ran = conv.run("sh('echo nope >&2; exit 3')\nprint('never')");
        assert_eq!(ran.ended, Ended::Raised);
        assert!(
            ran.output.contains("ShError: command exited 3"),
            "{}",
            ran.output
        );
        assert!(ran.output.contains("nope"), "{}", ran.output);
        assert!(!ran.output.contains("never"), "{}", ran.output);

        let ran = conv.run("r = sh('exit 3', check=False)\nr.code, r.ok");
        assert_eq!(ran.ended, Ended::Completed);
        assert_eq!(ran.output.trim(), "(3, False)");
    }

    /// A program that is not there ends the same way whether the command was a string for the
    /// shell or a list run as written: a failed command, not a traceback through `subprocess`.
    #[test]
    fn a_command_that_cannot_be_started_fails_like_any_other() {
        let conv = Conversation::new("missing");
        let ran = conv.run("sh(['adi-no-such-program', '--flag'])");
        assert_eq!(ran.ended, Ended::Raised);
        assert!(
            ran.output
                .contains("ShError: couldn't run adi-no-such-program --flag"),
            "{}",
            ran.output
        );
        assert!(!ran.output.contains("subprocess.py"), "{}", ran.output);

        let ran = conv.run(
            "[sh(c, check=False).code for c in (['adi-no-such-program'], 'adi-no-such-program')]",
        );
        assert_eq!(
            ran.output.trim(),
            "[127, 127]",
            "the list and the string agree"
        );
    }

    /// One conversation, one location: a directory or a variable set through either tool is where
    /// the other one stands.
    #[test]
    fn the_directory_and_exported_variables_are_shared_with_bash() {
        let conv = Conversation::new("shared");
        let sub = conv.home.join("checkout");
        std::fs::create_dir_all(&sub).expect("mkdir");
        let sub = std::fs::canonicalize(&sub).expect("canonicalize");

        // Python moves and exports; Bash sees both.
        conv.run("os.chdir('checkout')\nos.environ['FROM_PY'] = \"it's here\"");
        let seen = conv.bash("pwd -P; echo \"$FROM_PY\"");
        assert_eq!(seen, format!("{}\nit's here\n", sub.display()));

        // Bash moves and exports; Python sees both, and still has what it had.
        conv.bash("cd ..; export FROM_SH='two words'");
        let ran = conv.run("os.getcwd(), os.environ['FROM_SH'], os.environ['FROM_PY']");
        let home = std::fs::canonicalize(&conv.home).expect("canonicalize");
        assert_eq!(
            ran.output.trim(),
            format!("('{}', 'two words', \"it's here\")", home.display())
        );
    }

    /// The values are the shell's to quote and the shell's to read back, so nothing in one is
    /// special on the way through — quotes, a dollar, a backslash, a line break — in either
    /// direction, and however many times Bash has rewritten the file in between.
    #[test]
    fn an_awkward_value_survives_the_trip_in_both_directions() {
        let conv = Conversation::new("awkward");
        let awkward = r#"it's "quoted" $HOME \back"#;

        conv.run(&format!(
            "os.environ['PY_SET'] = {awkward:?} + '\\nsecond line'"
        ));
        // Twice: the second command reads the file as Bash itself rewrote it.
        conv.bash("true");
        let seen = conv.bash("printf '%s' \"$PY_SET\"");
        assert_eq!(seen, format!("{awkward}\nsecond line"));

        conv.bash(&format!(
            "export SH_SET='{}'",
            awkward.replace('\'', r"'\''")
        ));
        conv.bash("true");
        let ran = conv.run("print(os.environ['SH_SET'])\nprint(os.environ['PY_SET'])");
        assert_eq!(ran.output, format!("{awkward}\n{awkward}\nsecond line\n"));
    }

    /// The deadline interrupts the cell where it stands, says where that was, kills the command it
    /// was waiting on — and leaves the conversation's variables alone.
    #[test]
    fn a_cell_past_its_deadline_is_interrupted_and_keeps_its_variables() {
        let conv = Conversation::new("deadline");
        conv.run("kept = 'still here'");

        let began = Instant::now();
        let ran = conv.run_for("print('started')\nsh('sleep 60')\nprint('never')", 400);
        assert_eq!(ran.ended, Ended::TimedOut);
        assert!(
            began.elapsed() < Duration::from_secs(10),
            "{:?}",
            began.elapsed()
        );
        assert!(ran.output.starts_with("started\n"), "{}", ran.output);
        assert!(ran.output.contains("line 2"), "{}", ran.output);
        assert!(ran.output.contains("KeyboardInterrupt"), "{}", ran.output);

        assert_eq!(conv.run("kept").output.trim(), "'still here'");
    }

    /// A cancelled turn takes its tool server with it mid-call. The cell it left running is
    /// stopped rather than left holding the interpreter, so the conversation's next call is
    /// answered at once — by the same interpreter, with what it held.
    #[test]
    fn a_cell_whose_caller_hung_up_is_stopped_and_the_next_call_is_not_kept_waiting() {
        let conv = Conversation::new("abandoned");
        conv.run("kept = 'still here'");

        let mut live = conv.code.reach().expect("it is running");
        let request = json!({
            "code": "import time\ntime.sleep(60)",
            "timeout_ms": 120_000,
            "max_output": 32_000,
            "env": launch_environment(),
        });
        writeln!(live.stream, "{request}").expect("send");
        // Long enough for the cell to be inside its sleep, then gone without reading a reply.
        std::thread::sleep(Duration::from_millis(300));
        drop(live);

        let began = Instant::now();
        let next = conv.run("kept");
        assert_eq!(next.output.trim(), "'still here'");
        assert!(!next.replaced, "the same interpreter answered");
        assert!(
            began.elapsed() < Duration::from_secs(10),
            "{:?}",
            began.elapsed()
        );
    }

    /// A lost interpreter is said, not left to surface as a `NameError` three calls later — but
    /// only when it was lost. A restart the model asked for is not news to it.
    #[test]
    fn a_new_interpreter_is_announced_only_when_the_old_one_was_lost() {
        let conv = Conversation::new("replaced");
        conv.run("x = 1");

        let live = conv.code.reach().expect("it is running");
        kill(&live);
        // The state file still names it — which is exactly what a crash leaves behind.
        let after = conv.run("'x' in globals()");
        assert!(after.replaced, "the model is told its variables are gone");
        assert_eq!(after.output.trim(), "False");

        conv.run("y = 2");
        let asked = conv
            .code
            .run(
                &conv.shell,
                &conv.home,
                "'y' in globals()",
                30_000,
                32_000,
                true,
                None,
            )
            .expect("run");
        assert!(!asked.replaced, "it asked for this one");
        assert_eq!(asked.output.trim(), "False");
    }

    /// Output past the limit loses its middle, not its end: the end is where the traceback is.
    #[test]
    fn long_output_keeps_both_ends() {
        let conv = Conversation::new("long");
        let ran = conv
            .code
            .run(
                &conv.shell,
                &conv.home,
                "for i in range(5000): print('row', i)\nraise ValueError('at the end')",
                30_000,
                2_000,
                false,
                None,
            )
            .expect("run");
        assert!(ran.output.starts_with("row 0\n"), "{}", ran.output);
        assert!(ran.output.contains("bytes omitted here"), "{}", ran.output);
        assert!(
            ran.output.trim_end().ends_with("ValueError: at the end"),
            "{}",
            ran.output
        );
        assert!(ran.output.len() < 3_000, "{}", ran.output.len());
    }

    /// A deleted session takes its interpreter with it, by the only signal it sends: its files are
    /// gone. Nothing here tells the process to stop.
    #[test]
    fn an_interpreter_whose_session_is_deleted_exits_on_its_own() {
        let conv = Conversation::new("swept");
        conv.run("x = 1");
        let live = conv.code.reach().expect("it is running");
        let pid = live.pid;
        drop(live);

        std::fs::remove_file(&conv.code.state).expect("sweep the state file");
        let deadline = Instant::now() + Duration::from_secs(10);
        while adi_osext::pid_alive(pid) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(100));
        }
        assert!(
            !adi_osext::pid_alive(pid),
            "it noticed its session was gone"
        );
    }

    /// A background cell is a script of its own: the same helpers, a fresh namespace, and an exit
    /// status that says whether it raised.
    #[test]
    fn a_background_cell_runs_as_a_script_with_the_same_helpers() {
        let conv = Conversation::new("job");

        let ok = conv.job("sh('echo from a job')\n'x' in globals()");
        assert!(ok.status.success());
        assert_eq!(String::from_utf8_lossy(&ok.stdout), "from a job\nFalse\n");

        let failed = conv.job("raise SystemExit(4)");
        assert_eq!(
            failed.status.code(),
            Some(1),
            "an exception is a failed job"
        );
    }

    // ── TypeScript: the same promises, kept by a different interpreter ──

    /// What one cell declared — a `const`, a function, a class, an import — is there for the
    /// next, and declaring it again is not an error: a cell is something to run twice.
    #[test]
    fn typescript_keeps_what_a_cell_declared_and_lets_it_be_declared_again() {
        let conv = Conversation::ts("carry");
        let first = conv.run(
            "import { basename } from 'node:path';\n\
             const rows: number[] = [3, 4];\n\
             function hyp(a: number, b: number): number { return Math.hypot(a, b); }\n\
             class Named { constructor(public name: string) {} }",
        );
        assert_eq!(first.ended, Ended::Completed, "{}", first.output);
        assert_eq!(
            first.output, "",
            "nothing was printed, so nothing comes back"
        );
        assert!(!first.replaced, "a first call is not a replacement");

        let second = conv.run("[hyp(...rows), basename('/a/b.txt'), new Named('n').name];");
        assert_eq!(
            second.output.trim(),
            "[ 5, \"b.txt\", \"n\" ]",
            "the last expression is echoed"
        );

        let again = conv.run("const rows = ['declared', 'again'];\nrows.length;");
        assert_eq!(again.ended, Ended::Completed, "{}", again.output);
        assert_eq!(again.output.trim(), "2");
    }

    /// A function a later cell redefines is the one earlier functions call — which is what
    /// fixing a helper and running the thing again relies on.
    #[test]
    fn typescript_calls_the_newest_definition_of_a_function() {
        let conv = Conversation::ts("redefine");
        conv.run("function helper() { return 'old'; }\nfunction main() { return helper(); }");
        let ran = conv.run("function helper() { return 'new'; }\nmain();");
        assert_eq!(ran.output.trim(), "\"new\"");
    }

    /// The last expression is shown whatever it is, and every bare command where it stands — but
    /// not the timer, the length or the promise that JavaScript returns from nearly everything.
    #[test]
    fn typescript_shows_the_last_expression_and_every_command() {
        let conv = Conversation::ts("echo");
        let ran = conv.run(
            "setTimeout(() => {}, 1);\n\
             [1, 2].push(3);\n\
             await sh('echo one');\n\
             console.log('two');\n\
             await sh('echo three >&2');\n\
             const quiet = await sh('echo quiet');\n\
             quiet.lines;",
        );
        assert_eq!(ran.output, "one\ntwo\nthree\n[ \"quiet\" ]\n");
    }

    /// Text that looks like code is still text. A template literal holding a file's worth of
    /// source is what an agent writing that file sends, and none of it may be read as the cell's.
    #[test]
    fn typescript_leaves_code_inside_a_template_literal_alone() {
        let conv = Conversation::ts("template");
        let ran = conv.run(
            "const source = `import x from \"y\";\nconst inside = 1;\nfunction no() {}\n${[1, 2].map((n) => `<${n}>`).join('')}`;\n\
             [source.split('\\n').length, typeof inside, typeof no];",
        );
        assert_eq!(ran.ended, Ended::Completed, "{}", ran.output);
        assert_eq!(ran.output.trim(), "[ 4, \"undefined\", \"undefined\" ]");
    }

    /// An exception ends the cell, not the interpreter, and names the model's own line.
    #[test]
    fn typescript_reports_an_exception_with_the_line_and_survives_it() {
        let conv = Conversation::ts("raise");
        let ran = conv.run(
            "const kept = 7;\nconsole.log('before');\n(null as any).x;\nconsole.log('never');",
        );
        assert_eq!(ran.ended, Ended::Raised);
        assert!(ran.output.starts_with("before\n"), "{}", ran.output);
        assert!(ran.output.contains("TypeError"), "{}", ran.output);
        assert!(
            ran.output.contains("at <cell 1>: null.x;"),
            "{}",
            ran.output
        );
        assert!(!ran.output.contains("never"), "{}", ran.output);
        assert!(!ran.output.contains("tscode.ts"), "{}", ran.output);

        assert_eq!(conv.run("kept;").output.trim(), "7");

        let unparsed = conv.run("const broken = 1 +");
        assert_eq!(unparsed.ended, Ended::Raised);
        assert!(
            unparsed.output.contains("SyntaxError"),
            "{}",
            unparsed.output
        );
        assert!(unparsed.output.contains("line 1"), "{}", unparsed.output);
    }

    /// A failing command rejects unless the cell said failing was an answer, in both spellings,
    /// and the error points at the line of the cell that ran it.
    #[test]
    fn typescript_a_failing_command_raises_unless_told_it_may_fail() {
        let conv = Conversation::ts("check");
        let ran = conv.run("await sh('echo nope >&2; exit 3');\nconsole.log('never');");
        assert_eq!(ran.ended, Ended::Raised);
        assert!(
            ran.output.contains("ShError: command exited 3"),
            "{}",
            ran.output
        );
        assert!(ran.output.contains("nope"), "{}", ran.output);
        assert!(
            ran.output.contains("at <cell 1>: await sh("),
            "{}",
            ran.output
        );
        assert!(!ran.output.contains("never"), "{}", ran.output);

        let ran = conv.run(
            "const codes = [];\n\
             for (const c of ['exit 3', ['adi-no-such-program'], 'adi-no-such-program']) codes.push((await sh(c, { check: false })).code);\n\
             codes;",
        );
        assert_eq!(ran.output.trim(), "[ 3, 127, 127 ]");
    }

    /// One conversation, one location — and that includes the third tool: what TypeScript,
    /// Python and Bash each move or export is where the other two stand.
    #[test]
    fn typescript_shares_the_directory_and_exports_with_bash_and_python() {
        let conv = Conversation::ts("shared");
        let python = Interpreter::new(Language::Python, &conv.home, "conv");
        let sub = conv.home.join("checkout");
        std::fs::create_dir_all(&sub).expect("mkdir");
        let sub = std::fs::canonicalize(&sub).expect("canonicalize");
        let awkward = r#"it's "quoted" $HOME \back"#;

        conv.run(&format!(
            "process.chdir('checkout');\nprocess.env.FROM_TS = {awkward:?};"
        ));
        assert_eq!(
            conv.bash("pwd -P; printf '%s' \"$FROM_TS\""),
            format!("{}\n{awkward}", sub.display())
        );

        conv.bash("cd ..; export FROM_SH='two words'");
        let seen = python
            .run(
                &conv.shell,
                &conv.home,
                "os.environ['FROM_PY'] = 'py'\nprint(os.environ['FROM_TS'])",
                30_000,
                32_000,
                false,
                None,
            )
            .expect("python");
        assert_eq!(seen.output, format!("{awkward}\n"));

        let home = std::fs::canonicalize(&conv.home).expect("canonicalize");
        let ran = conv.run(
            "console.log([process.cwd(), process.env.FROM_SH, process.env.FROM_PY].join(' | '));",
        );
        assert_eq!(
            ran.output.trim(),
            format!("{} | two words | py", home.display())
        );
        python.stop();
    }

    /// JavaScript cannot be interrupted, but the command it is waiting on can be stopped — which
    /// ends the cell where it stands and leaves the conversation's variables alone.
    #[test]
    fn typescript_past_its_deadline_stops_the_command_and_keeps_its_variables() {
        let conv = Conversation::ts("deadline");
        conv.run("const kept = 'still here';");

        let began = Instant::now();
        let ran = conv.run_for(
            "console.log('started');\nawait sh('echo partial; sleep 60');\nconsole.log('never');",
            400,
        );
        assert_eq!(ran.ended, Ended::TimedOut, "{}", ran.output);
        assert!(
            began.elapsed() < Duration::from_secs(10),
            "{:?}",
            began.elapsed()
        );
        assert!(
            ran.output.starts_with("started\npartial\n"),
            "{}",
            ran.output
        );
        assert!(
            ran.output.contains("ShError: command was stopped"),
            "{}",
            ran.output
        );
        assert!(!ran.output.contains("never"), "{}", ran.output);

        assert_eq!(conv.run("kept;").output.trim(), "\"still here\"");
    }

    /// `$` is the conversation's shell, not a command runner: where a command ends and what it
    /// exported are where the code stands afterwards, and where `Bash` starts — an `unset` and a
    /// move back included. A call that was pointed somewhere else on purpose moves nothing.
    #[test]
    fn typescript_dollar_is_the_conversations_shell() {
        let conv = Conversation::ts("dollar-shell");
        std::fs::create_dir_all(conv.home.join("sub")).expect("mkdir");

        let ran = conv.run(
            r#"await $`cd sub && export FROM_DOLLAR='two words' MULTI="l1
l2"`.quiet();
console.log(JSON.stringify([process.cwd().endsWith('/sub'), process.env.FROM_DOLLAR, process.env.MULTI, (await $`pwd`).text.endsWith('/sub'), (await $`echo "$FROM_DOLLAR"`).text]));"#,
        );
        assert_eq!(
            ran.output.trim(),
            r#"[true,"two words","l1\nl2",true,"two words"]"#,
            "the code moved with the command, and the next command starts there"
        );
        let seen = conv.bash("pwd; echo \"$FROM_DOLLAR\"");
        assert!(seen.ends_with("/sub\ntwo words\n"), "and so does Bash: {seen}");

        let ran = conv.run(
            r"await $`unset FROM_DOLLAR; cd ..`.quiet();
const there = (await $`pwd`.cwd('sub')).text;
console.log(JSON.stringify([process.env.FROM_DOLLAR ?? null, process.cwd().endsWith('/sub'), there.endsWith('/sub')]));",
        );
        assert_eq!(ran.output.trim(), "[null,false,true]");
        assert_eq!(conv.bash("echo ${FROM_DOLLAR-gone}"), "gone\n");
    }

    /// What is interpolated is data, wherever in the command it lands: standing bare it is one
    /// argument (and an array several), inside the template's own quotes it is part of that
    /// string, in a heredoc part of the body — and in none of them is anything in it run.
    #[test]
    fn typescript_dollar_passes_what_is_interpolated_as_data() {
        let conv = Conversation::ts("dollar-data");
        let hostile = r#"two words; echo PWNED $(echo SUB) `echo TICK` "q" 'a' \n $HOME *"#;
        let ran = conv.run(&format!(
            r#"const hostile = {hostile};
const out = [
  (await $`printf '[%s]' ${{hostile}} ${{['a b', 7, []]}}`).text,
  (await $`printf '[%s]' "x ${{hostile}} y"`).text,
  (await $`printf '[%s]' 'x ${{hostile}} y'`).text,
  (await $`printf '%s' "nested $(printf '%s' "${{hostile}}") end"`).text,
  (await $`# a comment's apostrophe is not a quote
printf '[%s]' ${{hostile}}`).text,
  (await $`cat <<EOF
body ${{hostile}}
EOF`).text,
  (await $`printf '%s|' ${{$.raw('a   b')}} ${{'a   b'}}`).text,
  (await $`printf 'x\ny' | grep -c '\by'`).text,
];
console.log(JSON.stringify(out));"#,
            hostile = serde_json::to_string(hostile).expect("a string"),
        ));
        assert_eq!(ran.ended, Ended::Completed, "{}", ran.output);
        let out: Vec<String> = serde_json::from_str(ran.output.trim()).expect("json");
        assert_eq!(
            out,
            [
                format!("[{hostile}][a b][7]"),
                format!("[x {hostile} y]"),
                format!("[x {hostile} y]"),
                format!("nested {hostile} end"),
                format!("[{hostile}]"),
                format!("body {hostile}"),
                "a|b|a   b|".to_string(),
                "1".to_string(),
            ],
            "each value arrived whole and none of it ran; what is typed between the backticks \
             reaches the shell as typed"
        );

        // A value that is not there, or is not something a command can hold, is refused before
        // anything runs — and where the result is awaited, like any other failure of the call.
        let refused = conv.run(
            r"const errors = [];
for (const make of [() => $`touch ran-anyway ${undefined}`, () => $`touch ran-anyway ${{ a: 1 }}`, () => $`cat <<'EOF'
${'x'}
EOF`, () => $.background`echo x`]) {
  errors.push(await Promise.resolve().then(make).then(() => 'ran', (error) => error.name + ': ' + error.message.slice(0, 38)));
}
console.log(JSON.stringify([...errors, fs.existsSync('ran-anyway')]));",
        );
        assert_eq!(
            refused.output.trim(),
            r#"["TypeError: $: a value put into the command is und","TypeError: $: an object cannot be put into a comm","TypeError: $: a value cannot be put into a heredo","Error: $.background starts a job through the ",false]"#
        );
    }

    /// A `$` call ends the way a command does and reads the way one reads: a non-zero exit
    /// rejects unless told it may fail, the helpers go straight to a value, and a call on a
    /// line of its own shows its output unless it was asked to be quiet.
    #[test]
    fn typescript_dollar_fails_resolves_and_shows_like_a_command() {
        let conv = Conversation::ts("dollar-result");
        let ran = conv.run(
            r#"const failed = await $`echo out; exit 3`.nothrow();
let thrown = '';
try { await $`echo before; echo why >&2; exit 4`; } catch (error) { thrown = `${error.name} ${error.code} ${error.message.split('\n')[0]}`; }
console.log(JSON.stringify([failed.code, failed.exitCode, failed.ok, failed.text, thrown, await $`echo hi`.text(), await $`printf 'a\nb\n'`.lines(), await $`echo '{"a":1}'`.json(), (await $`tr a-z A-Z`.input('shout')).text]));"#,
        );
        assert_eq!(
            ran.output.trim(),
            r#"[3,3,false,"out","ShError 4 command exited 4: echo before; echo why >&2; exit 4","hi",["a","b"],{"a":1},"SHOUT"]"#
        );

        let shown = conv.run(
            "await $`echo one`;\nawait $`echo hidden`.quiet();\nconst kept = await $`echo kept`;\nawait $`true`;\nawait sh('true');\nawait $`echo three >&2`;",
        );
        assert_eq!(
            shown.output, "one\nthree\n",
            "a command that said nothing adds nothing — no line apiece for the silent ones"
        );

        let forgot = conv.run("$`echo never-read`;");
        assert!(
            forgot.output.contains("not awaited"),
            "a call nobody awaited says what is missing: {}",
            forgot.output
        );
    }

    /// A `$` command past the cell's deadline is stopped like any other, the variables survive —
    /// and nothing is taken from a command that did not finish: the conversation stays where it
    /// stood, not where the command had got to.
    #[test]
    fn typescript_dollar_past_its_deadline_is_stopped_and_moves_nothing() {
        let conv = Conversation::ts("dollar-deadline");
        std::fs::create_dir_all(conv.home.join("sub")).expect("mkdir");
        conv.run("const kept = 'still here';");

        let began = Instant::now();
        let ran = conv.run_for("await $`cd sub; echo partial; sleep 60`;\nconsole.log('never');", 400);
        assert_eq!(ran.ended, Ended::TimedOut, "{}", ran.output);
        assert!(began.elapsed() < Duration::from_secs(10), "{:?}", began.elapsed());
        assert!(ran.output.starts_with("partial\n"), "{}", ran.output);
        assert!(
            ran.output.contains("ShError: command was stopped") && ran.output.contains("cd sub; echo partial; sleep 60"),
            "named as it was written, not as the script that ran it: {}",
            ran.output
        );
        assert!(!ran.output.contains("__adi_"), "{}", ran.output);

        let after = conv.run("console.log(kept, process.cwd().endsWith('/sub'));");
        assert_eq!(after.output.trim(), "still here false");
    }

    /// A cell waiting on something that is not a command cannot be stopped at all. That costs the
    /// interpreter, and the call says so in as many words rather than reporting a dropped
    /// connection — and the conversation's next call is told it is somewhere new.
    #[test]
    fn typescript_that_cannot_be_stopped_ends_its_interpreter_and_says_so() {
        let conv = Conversation::ts("stuck");
        conv.run("const kept = 1;");

        let said = conv
            .code
            .run(
                &conv.shell,
                &conv.home,
                "console.log('tick');\nawait new Promise((r) => setTimeout(r, 60_000));",
                300,
                32_000,
                false,
                None,
            )
            .expect_err("a cell that will not stop is an error");
        assert!(
            said.starts_with("tick\n"),
            "what it printed is kept: {said}"
        );
        assert!(said.contains("cannot be interrupted"), "{said}");
        assert!(said.contains("is gone"), "{said}");

        let after = conv.run("typeof kept;");
        assert!(after.replaced, "the model is told its variables are gone");
        assert_eq!(after.output.trim(), "\"undefined\"");
    }

    /// A cancelled turn's cell is stopped rather than left holding the interpreter.
    #[test]
    fn typescript_whose_caller_hung_up_is_stopped_and_the_next_call_is_not_kept_waiting() {
        let conv = Conversation::ts("abandoned");
        conv.run("const kept = 'still here';");

        let mut live = conv.code.reach().expect("it is running");
        let request = json!({
            "code": "await sh('sleep 60');",
            "timeout_ms": 120_000,
            "max_output": 32_000,
            "env": launch_environment(),
        });
        writeln!(live.stream, "{request}").expect("send");
        std::thread::sleep(Duration::from_millis(400));
        drop(live);

        let began = Instant::now();
        let next = conv.run("kept;");
        assert_eq!(next.output.trim(), "\"still here\"");
        assert!(!next.replaced, "the same interpreter answered");
        assert!(
            began.elapsed() < Duration::from_secs(10),
            "{:?}",
            began.elapsed()
        );
    }

    /// Things that end an ordinary script — `process.exit`, an error in a callback, a promise
    /// nobody awaited — end at most the cell, and what a callback printed after its cell returned
    /// is handed to the next call instead of being lost.
    #[test]
    fn typescript_survives_what_would_end_a_script_and_reports_what_happened_between_calls() {
        let conv = Conversation::ts("survive");
        let exited = conv.run(
            "const kept = 1;\nconsole.log('before');\nprocess.exit(3);\nconsole.log('never');",
        );
        assert_eq!(exited.ended, Ended::Raised);
        assert_eq!(exited.output, "before\nprocess.exit(3)\n");

        conv.run(
            "setTimeout(() => { throw new Error('thrown in a callback'); }, 5);\n\
             Promise.reject(new Error('nobody awaited this'));\n\
             setTimeout(() => console.log('printed between calls'), 20);",
        );
        std::thread::sleep(Duration::from_millis(300));
        let next = conv.run("kept;");
        assert!(
            next.output.starts_with("[printed since your last call]\n"),
            "{}",
            next.output
        );
        assert!(
            next.output.contains("thrown in a callback"),
            "{}",
            next.output
        );
        assert!(
            next.output.contains("nobody awaited this"),
            "{}",
            next.output
        );
        assert!(
            next.output.contains("printed between calls"),
            "{}",
            next.output
        );
        assert!(next.output.ends_with("[this call]\n1\n"), "{}", next.output);
    }

    /// A module of the project's own is read again once it has changed — importing it again is
    /// what somebody who just edited it does next, and a stale copy would say the edit did nothing.
    #[test]
    fn typescript_imports_from_where_the_conversation_stands_and_rereads_an_edited_file() {
        let conv = Conversation::ts("import");
        std::fs::write(conv.home.join("lib.ts"), "export const version = 1;\n").expect("write");
        let first = conv.run("import { version } from './lib';\nconst dynamic = await import('./lib.ts');\n[version, dynamic.version, import.meta.dir === process.cwd()];");
        assert_eq!(first.output.trim(), "[ 1, 1, true ]", "{}", first.output);

        // A different second, since the file's modification time is what marks it as changed.
        std::thread::sleep(Duration::from_millis(1100));
        std::fs::write(conv.home.join("lib.ts"), "export const version = 2;\n").expect("write");
        let second = conv.run("import { version } from './lib';\nversion;");
        assert_eq!(second.output.trim(), "2");
    }

    /// Output past the limit loses its middle, not its end.
    #[test]
    fn typescript_long_output_keeps_both_ends() {
        let conv = Conversation::ts("long");
        let ran = conv
            .code
            .run(&conv.shell, &conv.home, "for (let i = 0; i < 5000; i++) console.log('row', i);\nthrow new Error('at the end');", 30_000, 2_000, false, None)
            .expect("run");
        assert!(ran.output.starts_with("row 0\n"), "{}", ran.output);
        assert!(ran.output.contains("bytes omitted here"), "{}", ran.output);
        assert!(ran.output.contains("Error: at the end"), "{}", ran.output);
        assert!(ran.output.len() < 3_000, "{}", ran.output.len());
    }

    /// A deleted session takes its interpreter with it, and a background cell is a script of its
    /// own with the same helpers.
    #[test]
    fn typescript_exits_with_its_session_and_runs_a_background_cell_as_a_script() {
        let conv = Conversation::ts("swept");
        conv.run("const x = 1;");

        let ok = conv.job("await sh('echo from a job');\ntypeof x;");
        assert!(
            ok.status.success(),
            "{}",
            String::from_utf8_lossy(&ok.stderr)
        );
        assert_eq!(
            String::from_utf8_lossy(&ok.stdout),
            "from a job\n\"undefined\"\n"
        );
        let failed = conv.job("throw new Error('boom');");
        assert_eq!(
            failed.status.code(),
            Some(1),
            "an exception is a failed job"
        );

        let live = conv.code.reach().expect("it is running");
        let pid = live.pid;
        drop(live);
        std::fs::remove_file(&conv.code.state).expect("sweep the state file");
        let deadline = Instant::now() + Duration::from_secs(10);
        while adi_osext::pid_alive(pid) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(100));
        }
        assert!(
            !adi_osext::pid_alive(pid),
            "it noticed its session was gone"
        );
    }
}
