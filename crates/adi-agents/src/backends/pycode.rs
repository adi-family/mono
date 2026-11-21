//! The Python interpreter a conversation keeps between `PyCode` calls.
//!
//! `Bash` carries a conversation's *location* from call to call — a directory and the variables it
//! exported (see [`super::shell`]). Everything else a command worked out is gone the moment it
//! returns, so an agent that parsed a file, fetched a listing or built a table has two choices:
//! print all of it into the transcript, or do the work again next call. `PyCode` is the third: the
//! interpreter outlives the call, so what a cell computed is still in a variable when the next
//! cell runs.
//!
//! # A process that outlives whoever started it
//!
//! Whoever serves a tool call is short-lived — the adi loop's turn child, or the MCP server an
//! engine's CLI spawned (the same fact [`super::jobs`] is built around). An interpreter held by
//! either would die with the turn. So it is a detached process of its own, one per conversation,
//! reached over a Unix socket: each call connects, sends one request, reads one reply.
//!
//! Three files in the session's own `<id>.*` namespace, where the session's deletion sweeps them
//! without this module being told:
//!
//! - `<id>.py-kernel` — who is serving: a pid and where its socket is. The socket itself lives in
//!   a private temp directory, because a socket path has about a hundred bytes to fit in and a
//!   session directory is often longer than that on its own.
//! - `<id>.pycode.py` — the interpreter's own source, written from this binary.
//! - `<id>.py-kernel.log` — what the interpreter process itself said, for when it will not start.
//!
//! The state file is also how the interpreter is told to stop. It checks every couple of seconds
//! that the file still names it: gone means the session was deleted, another pid means it was
//! replaced, and either way it exits. It also exits after [`IDLE_SECONDS`] without a call, leaving
//! a tombstone so the next call can say the interpreter it found is a new one.
//!
//! # One conversation state, two tools
//!
//! The interpreter reads and writes the *same* two files `Bash` carries its state in. Before each
//! cell it stands where the conversation stands — the caller's launch environment, the exported
//! variables on top, the recorded directory — and afterwards it records where the cell left
//! things. So `os.chdir()` moves `Bash` too, `export` in `Bash` shows up in `os.environ`, and a
//! variable the runner refreshes between turns reaches a cell as the runner meant it, exactly as
//! it reaches a command. A second, private notion of "where am I" is what this avoids: the tools
//! are two languages over one conversation, not two conversations.
//!
//! Only variables, imports and definitions are the interpreter's alone, and they are what is lost
//! when it goes. That is said to the model when it happens ([`Ran::replaced`]) rather than left to
//! be discovered as a `NameError`.
//!
//! # Stopping a cell
//!
//! The interpreter enforces the call's own deadline by interrupting the cell — a
//! `KeyboardInterrupt` where it stands, which leaves the variables intact — and does the same when
//! the caller hangs up, so a cancelled turn does not leave a cell holding the interpreter. This
//! side is only the backstop: a cell that will not be interrupted (a C loop, a caught interrupt)
//! is given [`STOP_GRACE`] and then the interpreter is killed, which is the one way a call loses
//! the conversation's variables for it.
//!
//! Unix only, like the session shell: the socket, the signals and the process group are all
//! things Windows spells differently, and an untested second spelling would be worse than none.
//! The tool is simply not declared there.

use std::io::{Read as _, Write as _};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use super::shell::Shell;

/// The interpreter, as shipped inside this binary.
const SOURCE: &str = include_str!("pycode.py");

/// The wire both halves were built against. An interpreter started by an older binary announces
/// an older number and is replaced rather than spoken to in a dialect it does not have.
const PROTOCOL: u64 = 1;

/// How long an interpreter nobody calls stays up. An hour is a conversation still in progress —
/// a build awaited, a person answering — while anything longer is most likely a conversation that
/// is over and a process holding memory for nobody.
const IDLE_SECONDS: u64 = 3600;

/// Overrides [`IDLE_SECONDS`], for a deployment that wants interpreters kept longer or shorter.
const IDLE_ENV: &str = "ADI_PYCODE_IDLE_SECONDS";

/// Names the interpreter to run instead of the first `python3` on `PATH`.
const PYTHON_ENV: &str = "ADI_PYCODE_PYTHON";

/// How long a new interpreter is given to come up. Its start is an import of the standard library
/// and a `bind`, tens of milliseconds warm — this is the budget for a cold disk, not an
/// expectation.
const START_WAIT: Duration = Duration::from_secs(20);

/// How long past its deadline a cell is given to stop on its own before the interpreter is killed.
/// The interpreter interrupts the cell at the deadline and again every few seconds; a cell running
/// a command needs a moment to kill it first. Past this it is not going to stop.
const STOP_GRACE: Duration = Duration::from_secs(15);

/// One conversation's interpreter, as the files that say where it is.
#[derive(Debug)]
pub(crate) struct Interpreter {
    /// Who is serving, or a tombstone saying nobody is any more.
    state: PathBuf,
    /// [`SOURCE`], on disk where a `python3` can run it.
    script: PathBuf,
    /// The interpreter process's own stdout and stderr.
    log: PathBuf,
    /// The session directory, for the scripts background cells are written to.
    dir: PathBuf,
    /// The session id, the stem of everything above.
    id: String,
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
    pub(crate) fn new(agent_dir: &Path, session_id: &str) -> Self {
        Self {
            state: agent_dir.join(format!("{session_id}.py-kernel")),
            script: agent_dir.join(format!("{session_id}.pycode.py")),
            log: agent_dir.join(format!("{session_id}.py-kernel.log")),
            dir: agent_dir.to_path_buf(),
            id: session_id.to_string(),
        }
    }

    /// Run `code` in the conversation's interpreter, starting one if there is none.
    ///
    /// `home` is the run's own directory, where a conversation that has not moved stands.
    /// `max_output` is how many bytes of what the cell printed may come back.
    ///
    /// # Errors
    /// A message for the model when there is no interpreter to be had (no `python3`, or it would
    /// not start), or when the one that ran the cell had to be killed or died under it. A cell
    /// that *raised* is not an error here — it ran, and [`Ran::ended`] says how it went.
    pub(crate) fn run(
        &self,
        shell: &Shell,
        home: &Path,
        code: &str,
        timeout_ms: u64,
        max_output: usize,
        restart: bool,
    ) -> Result<Ran, String> {
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

        let request = json!({
            "code": code,
            "timeout_ms": timeout_ms,
            "max_output": max_output,
            "env": launch_environment(),
        });
        let sent = writeln!(live.stream, "{request}").and_then(|()| live.stream.flush());
        if let Err(e) = sent {
            return Err(format!("couldn't reach the Python interpreter: {e}"));
        }

        let deadline = Instant::now() + Duration::from_millis(timeout_ms) + STOP_GRACE;
        let reply = match read_reply(&mut live.stream, deadline) {
            Ok(reply) => reply,
            Err(Unanswered::TimedOut) => {
                kill(&live);
                return Err(format!(
                    "the code was still running after {timeout_ms}ms and could not be \
                     interrupted, so its interpreter was killed — every variable and import it \
                     held is gone, and your next call starts a new one. Whatever the code had \
                     already changed on disk stays changed. Set `background` for work that long."
                ));
            }
            Err(Unanswered::Gone(why)) => {
                return Err(format!(
                    "the Python interpreter went away while running this ({why}) — it exited or \
                     was killed, so every variable and import it held is gone, and your next call \
                     starts a new one. Whatever the code had already changed on disk stays \
                     changed."
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
            _ => Ended::Raised,
        };
        Ok(Ran {
            output,
            ended,
            replaced,
        })
    }

    /// The shell command that runs `code` as a script of its own — what a background `PyCode`
    /// hands to [`super::jobs`].
    ///
    /// A fresh interpreter, deliberately, and for [`Shell::inherit`]'s reason: a job outlives the
    /// call that started it, and the conversation's interpreter runs one cell at a time, so a job
    /// inside it would hold every later call behind a build. It gets the same helpers and the same
    /// echoing of top-level expressions; it does not get the conversation's variables, and the
    /// tool's description says so.
    ///
    /// # Errors
    /// A message for the model if the script cannot be written.
    pub(crate) fn job_command(&self, code: &str) -> Result<String, String> {
        self.install()
            .map_err(|e| format!("couldn't write the interpreter's source: {e}"))?;
        let cell = self.dir.join(format!("{}.pyjob-{}.py", self.id, stamp()));
        std::fs::write(&cell, code).map_err(|e| format!("couldn't write {}: {e}", cell.display()))?;
        // `-u`: a job's output is a log somebody may read while it runs, so it is written through
        // as it happens rather than when a buffer fills.
        Ok(format!(
            "exec {} -u {} run {}",
            quote(&python()),
            quote(&self.script.display().to_string()),
            quote(&cell.display().to_string()),
        ))
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

    /// Write [`SOURCE`] where a `python3` can run it, unless that is already what is there.
    fn install(&self) -> std::io::Result<()> {
        std::fs::create_dir_all(&self.dir)?;
        if std::fs::read_to_string(&self.script).is_ok_and(|have| have == SOURCE) {
            return Ok(());
        }
        std::fs::write(&self.script, SOURCE)
    }

    /// Start an interpreter and wait until it answers.
    fn start(&self, shell: &Shell, home: &Path) -> Result<Live, String> {
        self.install()
            .map_err(|e| format!("couldn't write the interpreter's source: {e}"))?;
        // Whatever was recorded is about an interpreter that is not there. Removed before the new
        // one starts, so the file that appears next can only be the new one's.
        let _ = std::fs::remove_file(&self.state);

        let open_log = || {
            std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&self.log)
        };
        let (out, err) = open_log()
            .and_then(|out| Ok((out.try_clone()?, out)))
            .map_err(|e| format!("couldn't open {}: {e}", self.log.display()))?;

        let python = python();
        let (cwd_file, env_file) = shell.files();
        let idle = std::env::var(IDLE_ENV)
            .ok()
            .and_then(|v| v.trim().parse::<u64>().ok())
            .unwrap_or(IDLE_SECONDS);
        let mut cmd = Command::new(&python);
        cmd.arg(&self.script)
            .arg("kernel")
            .arg("--state")
            .arg(&self.state)
            .arg("--shell-cwd")
            .arg(cwd_file)
            .arg("--shell-env")
            .arg(env_file)
            .arg("--home")
            .arg(home)
            .arg("--idle")
            .arg(idle.to_string())
            .arg("--protocol")
            .arg(PROTOCOL.to_string())
            .current_dir(home)
            .stdin(Stdio::null())
            .stdout(Stdio::from(out))
            .stderr(Stdio::from(err));
        // Its own process group, for the reason a job gets one: the process starting it is about
        // to end, and a signal to that process must not take the interpreter with it.
        adi_osext::detach_process_group(&mut cmd);

        let mut child = cmd.spawn().map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                format!(
                    "PyCode needs a Python 3 interpreter and there is no `{python}` on this \
                     machine's PATH. Use Bash instead."
                )
            } else {
                format!("couldn't start `{python}`: {e}")
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
                    "the Python interpreter exited as it started ({status}). Use Bash instead.\n{}",
                    tail(&self.log)
                ));
            }
            if Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!(
                    "the Python interpreter did not come up within {}s. Use Bash instead.\n{}",
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
        let wait = left.clamp(Duration::from_millis(1), Duration::from_secs(1));
        if let Err(e) = stream.set_read_timeout(Some(wait)) {
            return Err(Unanswered::Gone(e.to_string()));
        }
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

/// Which interpreter to run.
fn python() -> String {
    std::env::var(PYTHON_ENV)
        .ok()
        .map(|named| named.trim().to_string())
        .filter(|named| !named.is_empty())
        .unwrap_or_else(|| "python3".to_string())
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
        python: Interpreter,
    }

    impl Conversation {
        fn new(tag: &str) -> Self {
            let home = std::env::temp_dir().join(format!("adi-pycode-{tag}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&home);
            std::fs::create_dir_all(&home).expect("scratch dir");
            Self {
                shell: Shell::new(&home, "conv"),
                python: Interpreter::new(&home, "conv"),
                home,
            }
        }

        fn run(&self, code: &str) -> Ran {
            self.run_for(code, 30_000)
        }

        fn run_for(&self, code: &str, timeout_ms: u64) -> Ran {
            self.python
                .run(&self.shell, &self.home, code, timeout_ms, 32_000, false)
                .expect("the interpreter runs the cell")
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
            self.python.stop();
            let _ = std::fs::remove_dir_all(&self.home);
        }
    }

    /// The whole point: what one call computed is there for the next, without being printed and
    /// without being computed again.
    #[test]
    fn a_variable_a_function_and_an_import_reach_the_next_call() {
        let conv = Conversation::new("carry");
        let first = conv.run("import math\nrows = [3, 4]\ndef hyp(a, b):\n    return math.hypot(a, b)");
        assert_eq!(first.ended, Ended::Completed);
        assert_eq!(first.output, "", "nothing was printed, so nothing comes back");
        assert!(!first.replaced, "a first call is not a replacement");

        let second = conv.run("hyp(*rows)");
        assert_eq!(second.output.trim(), "5.0", "a bare expression is echoed");
        assert!(!second.replaced);
    }

    /// Every top-level expression is echoed, so three bare commands read like three commands —
    /// and a `print` in between lands in order with them.
    #[test]
    fn top_level_expressions_are_echoed_in_order_with_what_is_printed() {
        let conv = Conversation::new("echo");
        let ran = conv.run("sh('echo one')\nprint('two')\nsh('echo three >&2')\nx = sh('echo quiet')\n4");
        assert_eq!(ran.output, "one\ntwo\nthree\n4\n");
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
        assert!(ran.output.contains("ShError: command exited 3"), "{}", ran.output);
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
        assert!(ran.output.contains("ShError: couldn't run adi-no-such-program --flag"), "{}", ran.output);
        assert!(!ran.output.contains("subprocess.py"), "{}", ran.output);

        let ran = conv.run("[sh(c, check=False).code for c in (['adi-no-such-program'], 'adi-no-such-program')]");
        assert_eq!(ran.output.trim(), "[127, 127]", "the list and the string agree");
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

        conv.run(&format!("os.environ['PY_SET'] = {awkward:?} + '\\nsecond line'"));
        // Twice: the second command reads the file as Bash itself rewrote it.
        conv.bash("true");
        let seen = conv.bash("printf '%s' \"$PY_SET\"");
        assert_eq!(seen, format!("{awkward}\nsecond line"));

        conv.bash(&format!("export SH_SET='{}'", awkward.replace('\'', r"'\''")));
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
        assert!(began.elapsed() < Duration::from_secs(10), "{:?}", began.elapsed());
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

        let mut live = conv.python.reach().expect("it is running");
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
        assert!(began.elapsed() < Duration::from_secs(10), "{:?}", began.elapsed());
    }

    /// A lost interpreter is said, not left to surface as a `NameError` three calls later — but
    /// only when it was lost. A restart the model asked for is not news to it.
    #[test]
    fn a_new_interpreter_is_announced_only_when_the_old_one_was_lost() {
        let conv = Conversation::new("replaced");
        conv.run("x = 1");

        let live = conv.python.reach().expect("it is running");
        kill(&live);
        // The state file still names it — which is exactly what a crash leaves behind.
        let after = conv.run("'x' in globals()");
        assert!(after.replaced, "the model is told its variables are gone");
        assert_eq!(after.output.trim(), "False");

        conv.run("y = 2");
        let asked = conv
            .python
            .run(&conv.shell, &conv.home, "'y' in globals()", 30_000, 32_000, true)
            .expect("run");
        assert!(!asked.replaced, "it asked for this one");
        assert_eq!(asked.output.trim(), "False");
    }

    /// Output past the limit loses its middle, not its end: the end is where the traceback is.
    #[test]
    fn long_output_keeps_both_ends() {
        let conv = Conversation::new("long");
        let ran = conv
            .python
            .run(
                &conv.shell,
                &conv.home,
                "for i in range(5000): print('row', i)\nraise ValueError('at the end')",
                30_000,
                2_000,
                false,
            )
            .expect("run");
        assert!(ran.output.starts_with("row 0\n"), "{}", ran.output);
        assert!(ran.output.contains("bytes omitted here"), "{}", ran.output);
        assert!(ran.output.trim_end().ends_with("ValueError: at the end"), "{}", ran.output);
        assert!(ran.output.len() < 3_000, "{}", ran.output.len());
    }

    /// A deleted session takes its interpreter with it, by the only signal it sends: its files are
    /// gone. Nothing here tells the process to stop.
    #[test]
    fn an_interpreter_whose_session_is_deleted_exits_on_its_own() {
        let conv = Conversation::new("swept");
        conv.run("x = 1");
        let live = conv.python.reach().expect("it is running");
        let pid = live.pid;
        drop(live);

        std::fs::remove_file(&conv.python.state).expect("sweep the state file");
        let deadline = Instant::now() + Duration::from_secs(10);
        while adi_osext::pid_alive(pid) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(100));
        }
        assert!(!adi_osext::pid_alive(pid), "it noticed its session was gone");
    }

    /// A background cell is a script of its own: the same helpers, a fresh namespace, and an exit
    /// status that says whether it raised.
    #[test]
    fn a_background_cell_runs_as_a_script_with_the_same_helpers() {
        let conv = Conversation::new("job");
        let run = |code: &str| {
            let command = conv.python.job_command(code).expect("command");
            Command::new("sh")
                .arg("-c")
                .arg(command)
                .current_dir(&conv.home)
                .output()
                .expect("sh")
        };

        let ok = run("sh('echo from a job')\n'x' in globals()");
        assert!(ok.status.success());
        assert_eq!(String::from_utf8_lossy(&ok.stdout), "from a job\nFalse\n");

        let failed = run("raise SystemExit(4)");
        assert_eq!(failed.status.code(), Some(1), "an exception is a failed job");
    }
}
