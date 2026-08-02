//! The detached-child primitives every headless run is built out of: spawn one, tail its log,
//! find out whether it is still alive, and signal its process tree.
//!
//! Process mechanics only. This module used to also own the *run* — a per-agent directory of
//! `<run_id>.{pid,log,json}` files, a run history, hiding, pruning, and metadata sidecars — which
//! made it a second session store keyed by executor subdir, and that layout is the bug that
//! motivated the rewrite: change an agent's backend and its history moved out from under it.
//! Sessions belong to [`crate::store`] now, and what is left here is what a store should never have
//! to know: how to fork a child that outlives its launcher.
//!
//! Where the pid file goes is still decided here ([`pid_path_in`]) because the reaper thread has to
//! delete it after [`spawn_child`] returns — the one piece of layout that a caller cannot own.

use std::fs::File;
use std::io::{Read as _, Seek as _, SeekFrom};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::error::{Error, Result};

/// Spawn one detached child of a run: `argv` writing its combined stdout+stderr to `log` (created
/// fresh, so a re-used slot's previous output is replaced), its PID recorded at `<run_id>.pid`, and
/// a reaper thread that drops the PID file once the child exits. Returns the child PID.
///
/// Shared by the one-shot [`launch`] and the harness conversation turns, which spawn a fresh child
/// into the *same* `run_id` slot for each answer — so this is the single place the detached-child
/// wiring (secrets, `PATH`, working dir, process group, reaping) lives.
pub(crate) fn spawn_child(
    dir: &Path,
    run_id: &str,
    log: &Path,
    base_dir: &Path,
    run_path: &str,
    argv: &[String],
    run_env: &[(String, String)],
) -> Result<u32> {
    let log_file = File::create(log)?;
    let errlog = log_file.try_clone()?;
    let (program, command_args) = argv
        .split_first()
        .ok_or_else(|| Error::Launch("backend built an empty command".to_string()))?;

    let mut command = Command::new(program);
    command
        .args(command_args)
        // Injected secrets and the agent's declared vars go in first, under their literal names;
        // `PATH` is set right after so nothing there can shadow the tool path.
        .envs(run_env.iter().map(|(k, v)| (k, v)))
        .env("PATH", run_path)
        .stdin(Stdio::null())
        .stdout(Stdio::from(log_file))
        .stderr(Stdio::from(errlog));
    // Detach the run from the launcher's process group so a Ctrl-C / signal to the parent
    // doesn't tear down the agent. Unix: new process group; Windows: the equivalent flag.
    adi_osext::detach_process_group(&mut command);
    // `base_dir` is the run's directory, already resolved by `workspace::resolve` — the launch's
    // own choice, else the manifest's, else the agent's project, else the store root. Not the
    // launching daemon's cwd, and not re-decided here: one directory, decided once.
    command.current_dir(base_dir);

    let mut child = command
        .spawn()
        .map_err(|e| Error::Launch(format!("couldn't spawn {program}: {e}")))?;
    let pid = child.id();
    let pid_file = pid_path_in(dir, run_id);
    if let Err(e) = std::fs::write(&pid_file, format!("{pid}\n")) {
        let _ = child.kill();
        return Err(Error::Io(e));
    }

    // Long-lived app servers must reap completed children. On exit only the PID file is dropped
    // (marking the run/turn finished); the log and metadata stay as history.
    let reaper_pid_file = pid_file.clone();
    std::thread::spawn(move || {
        let _ = child.wait();
        if read_pid(&reaper_pid_file) == Some(pid) {
            let _ = std::fs::remove_file(reaper_pid_file);
        }
    });

    Ok(pid)
}

/// The tail of a log at a known path — the same read as [`tail_log`], for a caller that already
/// holds the path (the store keeps its own layout, and asking it beats rebuilding one here).
pub(crate) fn tail_of(path: &Path, max_bytes: u64) -> Option<String> {
    let mut file = File::open(path).ok()?;
    let len = file.metadata().ok()?.len();
    let start = len.saturating_sub(max_bytes);
    if start > 0 {
        file.seek(SeekFrom::Start(start)).ok()?;
    }
    let mut buf = Vec::new();
    file.read_to_end(&mut buf).ok()?;
    let text = String::from_utf8_lossy(&buf);
    let trimmed = text.trim_end();
    let body = if start > 0 {
        trimmed.split_once('\n').map_or(trimmed, |(_, rest)| rest)
    } else {
        trimmed
    };
    Some(body.to_string())
}

pub(crate) fn pid_path_in(dir: &Path, run_id: &str) -> PathBuf {
    dir.join(format!("{run_id}.pid"))
}

pub(crate) fn read_pid(path: &Path) -> Option<u32> {
    std::fs::read_to_string(path).ok()?.trim().parse().ok()
}

/// Whether a run's recorded pid still names a living process — one shared syscall-backed probe
/// ([`adi_osext::pid_alive`]), not a second copy of it.
///
/// This is the hottest question the store asks: every run listing checks one pid per run, and every
/// conversation poll checks the same one twice. It used to spawn `/bin/kill -0` to answer, which
/// cost a `fork`/`exec`/`wait` per call — milliseconds, spent blocked in the kernel — and that is
/// what let a couple of open chats starve the app server's threads.
pub(crate) fn pid_alive(pid: u32) -> bool {
    adi_osext::pid_alive(pid)
}

/// Signal a run's whole process tree. Unix: `kill -<sig> -<pid>` (negative pid = the group the
/// run leads). Windows: `taskkill /T` reaches the tree; `/F` on a hard kill (there is no graceful
/// signal for a headless child).
#[cfg(unix)]
pub(crate) fn signal_group(pid: u32, signal: &str) -> Result<()> {
    let status = Command::new("/bin/kill")
        .args([format!("-{signal}"), "--".into(), format!("-{pid}")])
        .status()
        .map_err(|e| Error::Process(e.to_string()))?;
    if status.success() || !pid_alive(pid) {
        Ok(())
    } else {
        Err(Error::Process(format!(
            "couldn't send SIG{signal} to process group {pid}"
        )))
    }
}

#[cfg(not(unix))]
pub(crate) fn signal_group(pid: u32, _signal: &str) -> Result<()> {
    // Headless agents have no window, so WM_CLOSE (soft `taskkill`) never reaches them — force
    // the whole tree (`/T /F`). The unix TERM-then-wait grace has no meaningful analog here.
    let status = Command::new("taskkill")
        .args(["/T", "/F", "/PID", &pid.to_string()])
        .status()
        .map_err(|e| Error::Process(e.to_string()))?;
    if status.success() || !pid_alive(pid) {
        Ok(())
    } else {
        Err(Error::Process(format!(
            "couldn't taskkill process tree {pid}"
        )))
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    fn scratch_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "adi-agents-detached-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("scratch dir");
        dir
    }

    /// A command that writes its working directory to `file`. `sh -c 'pwd > file'` on Unix;
    /// `cmd /c 'cd > file'` on Windows (`cd` with no args prints the cwd).
    fn write_cwd_argv(file: &str) -> Vec<String> {
        #[cfg(unix)]
        {
            vec!["/bin/sh".into(), "-c".into(), format!("pwd > {file}")]
        }
        #[cfg(not(unix))]
        {
            vec!["cmd".into(), "/c".into(), format!("cd > {file}")]
        }
    }

    /// The child runs in the directory it was given, not the launcher's. A run's directory is
    /// resolved once, at session creation, and every later turn re-enters it — an engine's own
    /// session store is keyed by cwd, so a child that starts somewhere else leaves the session
    /// unresumable and the files earlier turns wrote out of reach.
    #[test]
    fn a_child_starts_in_the_directory_it_was_given() {
        let dir = scratch_dir("basedir-run");
        let base = scratch_dir("basedir-cwd");

        // The child writes its own cwd, so the assertion is on where it actually ran.
        let pid = spawn_child(
            &dir,
            "run-1",
            &dir.join("run-1.log"),
            &base,
            "",
            &write_cwd_argv("cwd.txt"),
            &[],
        )
        .expect("spawn");
        assert!(pid > 0);

        let probe = base.join("cwd.txt");
        for _ in 0..100 {
            if probe.is_file() {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        let got = std::fs::read_to_string(&probe).expect("child wrote its cwd");
        // macOS temp dirs are symlinks (/var → /private/var), so compare canonical paths.
        let got = std::fs::canonicalize(got.trim()).unwrap();
        let want = std::fs::canonicalize(&base).unwrap();
        assert_eq!(got, want, "the child started in the directory it was given");

        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&base);
    }

    /// A spawned child records its pid where the reaper — and every later liveness check — expects
    /// it, and the reaper drops that file once the child exits. This is what makes "is it running?"
    /// answerable from a *different* process than the one that launched it.
    #[test]
    fn a_pid_is_recorded_and_dropped_when_the_child_exits() {
        let dir = scratch_dir("pidfile");
        let argv = vec!["/bin/sh".to_string(), "-c".to_string(), "exit 0".to_string()];
        let pid = spawn_child(&dir, "run-1", &dir.join("run-1.log"), &dir, "", &argv, &[]).expect("spawn");

        let pid_file = pid_path_in(&dir, "run-1");
        for _ in 0..200 {
            if !pid_file.exists() {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(!pid_file.exists(), "the reaper drops the pid of an exited child");
        assert!(!pid_alive(pid), "the child is gone");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_tail_reads_the_end_and_drops_a_partial_first_line() {
        let dir = scratch_dir("tail");
        // No such log → None, not an empty string.
        assert!(tail_of(&dir.join("missing.log"), 1024).is_none());

        let log = dir.join("run-1.log");
        std::fs::write(&log, "line one\nline two\n").unwrap();
        assert_eq!(tail_of(&log, 1024).as_deref(), Some("line one\nline two"));
        // A tail that starts mid-file (inside "line one") drops that partial line.
        assert_eq!(tail_of(&log, 13).as_deref(), Some("line two"));

        let _ = std::fs::remove_dir_all(&dir);
    }
}
