//! Background jobs — a `Bash` command that outlives the turn that started it.
//!
//! A turn ends when the model stops calling tools, and until now everything it started ended with
//! it: `Bash` ran the command to completion or killed it at the timeout, so a build was something an
//! agent could only ever *wait out* inside one call. A **job** is the other option — the command is
//! detached, the call returns immediately with an id, and the conversation is woken when the command
//! exits (see [`crate::awaits`]).
//!
//! That waking is the point, and it is why this is not the engine's own background mode. Claude
//! Code's built-in `Bash` can also run a command in the background, but its job is polled *from
//! inside the same turn* — the turn stays alive asking whether it is done yet, and when the turn
//! ends the job is gone. Here the turn is free to end. Nothing is held open, nothing is polled by
//! the run, and the wake arrives whenever the command actually finishes — a minute later or an hour.
//!
//! # Why a sentinel file rather than a process handle
//!
//! Whoever starts a job is short-lived: the adi loop's turn child, or the MCP server the engine's
//! CLI spawned. Both are gone long before a real build is. So a job cannot be *watched* by the
//! process that started it — a `wait` in a parent that exits first tells nobody anything, and the
//! run that needs the answer is a different process again.
//!
//! What survives instead is the filesystem. The command runs under a wrapper that writes its exit
//! status to a sentinel file as its last act, and the wake condition is *the sentinel exists, or the
//! process is gone*. The second half of that is not belt-and-braces: a wrapper killed with `SIGKILL`
//! never runs its last act, and without the liveness check its conversation would wait for a
//! sentinel that is never coming.
//!
//! Files live in the session's own `<id>.*` namespace, beside the shell's, so a deleted session
//! sweeps them without this module being told.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use super::shell::Shell;
use crate::error::{Error, Result};

/// How much of a finished job's output travels in the wake. The whole log stays on disk and the
/// model is told where — a tail is what fits in a message, not what was kept.
const WAKE_TAIL_BYTES: u64 = 4_000;

/// How often the await looks. Jobs are minutes-to-hours things and the check is two `test`s, so
/// this is about how promptly a finish is noticed rather than about load.
pub(crate) const LOOK_EVERY_SECONDS: u64 = 5;

/// A detached command and the two files that say what became of it.
#[derive(Debug, Clone)]
pub(crate) struct Job {
    /// This job's id — also the stem of its files, and what the model names it by.
    pub id: String,
    /// Where the command's output goes, both streams together.
    pub log: PathBuf,
    /// Written with the exit status as the wrapper's last act.
    pub exit: PathBuf,
    /// The wrapper's process id, so a job killed outright is still noticed.
    pub pid: u32,
}

/// Start `command` detached, inheriting the conversation's shell but recording nothing back into it.
///
/// # Errors
/// [`Error::Launch`] if the wrapper cannot be spawned, [`Error::Io`] if its log cannot be created.
pub(crate) fn start(
    agent_dir: &Path,
    conv: &str,
    shell: &Shell,
    start_dir: &Path,
    command: &str,
) -> Result<Job> {
    std::fs::create_dir_all(agent_dir)?;
    let id = new_id();
    let log = agent_dir.join(format!("{conv}.{id}.log"));
    let exit = agent_dir.join(format!("{conv}.{id}.exit"));

    // The status hangs off an `EXIT` trap rather than a line after the command, and that is
    // load-bearing: a command ending in `exit 7` — or failing under `set -e`, or dying on a syntax
    // error in what the model wrote — never reaches a line placed after it, and would leave a job
    // that runs, finishes, and reports nothing. The trap fires however the shell ends. `$?` is the
    // trap's first act, so what it records is the command's status and not its own.
    let script = shell.inherit(&format!(
        "__adi_job_done() {{ printf '%s' \"$?\" > {}; }}\ntrap __adi_job_done EXIT\n{command}",
        quote(&exit)
    ));

    let log_file = std::fs::File::create(&log)?;
    let errlog = log_file.try_clone()?;
    let mut cmd = if cfg!(windows) {
        let mut c = Command::new("cmd");
        c.arg("/C").arg(&script);
        c
    } else {
        let mut c = Command::new("sh");
        c.arg("-c").arg(&script);
        c
    };
    cmd.current_dir(start_dir)
        .stdin(Stdio::null())
        .stdout(Stdio::from(log_file))
        .stderr(Stdio::from(errlog));
    // Its own process group, for the same reason a run gets one: the turn that started this job is
    // about to end, and a signal to that process must not take the job with it.
    adi_osext::detach_process_group(&mut cmd);

    let mut child = cmd
        .spawn()
        .map_err(|e| Error::Launch(format!("couldn't start the job: {e}")))?;
    let pid = child.id();
    // Reaped on a thread of its own, for as long as this process happens to last. Not because the
    // ending is needed here — the sentinel is the record, and a turn child is usually gone first, at
    // which point the job is reparented and the system reaps it. It matters because an *unreaped*
    // child is a zombie, and a zombie still answers `kill -0`: without this, a job killed outright
    // would read as running to anything asking from inside this process.
    std::thread::spawn(move || {
        let _ = child.wait();
    });

    Ok(Job {
        id,
        log,
        exit,
        pid,
    })
}

/// The shell condition an await runs to decide whether this job is done, and what to tell the
/// conversation when it is.
///
/// Exit 0 wakes the run. Two ways to be done, and both are needed: the sentinel is there (the normal
/// ending, and the only one that knows the status), or the process is gone without having written
/// one (killed outright — which no sentinel will ever report). What it prints is what reaches the
/// model, so it prints the status and the tail rather than merely succeeding.
///
/// The liveness half is a pid, with a pid's one weakness: a killed job whose number the kernel has
/// since handed to somebody else reads as still running. The sentinel covers every ordinary ending,
/// so this only bites a job that was killed *and* unlucky, and the await's own expiry is what stops
/// that conversation waiting for ever.
#[must_use]
pub(crate) fn done_check(job: &Job) -> String {
    let (log, exit, pid) = (quote(&job.log), quote(&job.exit), job.pid);
    format!(
        "if [ -f {exit} ]; then printf 'job {id} finished — exit %s\\n\\n' \"$(cat {exit})\"; \
         elif kill -0 {pid} 2>/dev/null; then exit 1; \
         else printf 'job {id} is gone without recording a status — it was killed\\n\\n'; fi; \
         tail -c {WAKE_TAIL_BYTES} {log} 2>/dev/null; exit 0",
        id = job.id,
    )
}

/// One `sh` word, single-quoted. These paths are ours rather than the model's, but they carry a
/// session id and an agent name, and an agent name is free text.
fn quote(path: &Path) -> String {
    format!("'{}'", path.display().to_string().replace('\'', r"'\''"))
}

/// A fresh job id: the millisecond it started and a counter, so two jobs started in the same
/// millisecond by the same turn still get their own files.
fn new_id() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or_default();
    format!("job-{millis}-{:03}", SEQ.fetch_add(1, Ordering::Relaxed) % 1000)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "adi-jobs-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id(),
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("scratch");
        dir
    }

    /// Run a job's check the way the await worker does, and say whether it fired.
    fn check(job: &Job) -> (bool, String) {
        let out = Command::new("sh")
            .arg("-c")
            .arg(done_check(job))
            .output()
            .expect("check runs");
        (
            out.status.success(),
            String::from_utf8_lossy(&out.stdout).into_owned(),
        )
    }

    fn settle(job: &Job) {
        for _ in 0..100 {
            if job.exit.exists() {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        panic!("the job never recorded a status");
    }

    #[cfg(unix)]
    #[test]
    fn a_job_outlives_the_call_and_reports_its_status_and_output() {
        let dir = scratch("finish");
        let shell = Shell::new(&dir, "conv-1");
        let job = start(&dir, "conv-1", &shell, &dir, "echo built-it; exit 7").expect("start");

        settle(&job);
        let (fired, said) = check(&job);
        assert!(fired, "a finished job wakes its conversation");
        assert!(said.contains("exit 7"), "the status travels: {said}");
        assert!(said.contains("built-it"), "so does the output: {said}");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The half that is easy to leave out: a job still running must *not* wake anybody.
    #[cfg(unix)]
    #[test]
    fn a_running_job_does_not_wake_its_conversation() {
        let dir = scratch("running");
        let shell = Shell::new(&dir, "conv-1");
        let job = start(&dir, "conv-1", &shell, &dir, "sleep 30").expect("start");

        let (fired, _) = check(&job);
        assert!(!fired, "a job in flight is not a finished one");
        assert!(!job.exit.exists());

        let _ = Command::new("kill").arg("-9").arg(job.pid.to_string()).status();
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A job killed outright never writes a sentinel. Without the liveness half of the check, its
    /// conversation would wait for a status that is never coming.
    #[cfg(unix)]
    #[test]
    fn a_killed_job_is_noticed_rather_than_waited_on_forever() {
        let dir = scratch("killed");
        let shell = Shell::new(&dir, "conv-1");
        let job = start(&dir, "conv-1", &shell, &dir, "sleep 30").expect("start");

        // SIGKILL the group: the wrapper gets no chance to run its last act, by design.
        let _ = Command::new("kill")
            .arg("-9")
            .arg(format!("-{}", job.pid))
            .status();
        for _ in 0..100 {
            if !Command::new("kill")
                .args(["-0", &job.pid.to_string()])
                .status()
                .map(|s| s.success())
                .unwrap_or(false)
            {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }

        let (fired, said) = check(&job);
        assert!(fired, "a job that is simply gone is still an ending");
        assert!(!job.exit.exists(), "and it recorded nothing");
        assert!(said.contains("killed"), "which is said plainly: {said}");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A job starts where the conversation stands — and leaves that where it found it.
    #[cfg(unix)]
    #[test]
    fn a_job_inherits_the_conversations_shell_without_moving_it() {
        let dir = scratch("inherit");
        let shell = Shell::new(&dir, "conv-1");
        let home = dir.join("home");
        let elsewhere = dir.join("elsewhere");
        std::fs::create_dir_all(&home).expect("home");
        std::fs::create_dir_all(&elsewhere).expect("elsewhere");
        let out = Command::new("sh")
            .arg("-c")
            .arg(shell.script(&format!(
                "cd {} && export MARK=carried",
                quote(&elsewhere)
            )))
            .current_dir(&home)
            .output()
            .expect("seed");
        assert!(out.status.success());

        let job = start(&dir, "conv-1", &shell, &home, "pwd -P; echo MARK=$MARK").expect("start");
        settle(&job);
        let said = std::fs::read_to_string(&job.log).expect("log");
        assert!(
            said.contains(&std::fs::canonicalize(&elsewhere).expect("real").display().to_string()),
            "the job starts where the conversation stands: {said}"
        );
        assert!(said.contains("MARK=carried"), "and with what it exported: {said}");

        // …and the conversation is exactly where it was: a job that recorded its own `cd` would
        // land it after later commands had already made theirs.
        assert_eq!(
            shell.ended_in().map(|d| std::fs::canonicalize(&d).unwrap_or(d)),
            Some(std::fs::canonicalize(&elsewhere).expect("real")),
        );

        let _ = std::fs::remove_dir_all(&dir);
    }
}
