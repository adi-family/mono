//! Detached-process lifecycle shared by the `process` and `harness` executors.
//!
//! Each launch is an independent *run*: the agent definition is only a template, so a fresh run is
//! spawned every time (never continuing a prior one), several runs of the same agent may be live at
//! once, and every run keeps its own PID, log, and metadata under a per-agent directory —
//! `<sessions>/<subdir>/<agent>/<run_id>.{pid,log,json}`. Finished runs persist so their output
//! stays browsable as history; the oldest are pruned once the count passes `MAX_RUNS`.

use std::fs::File;
use std::io::{Read as _, Seek as _, SeekFrom};
use std::os::unix::process::CommandExt as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::StoredAgent;
use crate::error::{Error, Result};
use crate::run::{Launch, RunInfo};

/// How many runs to keep per agent before the oldest finished ones are pruned.
const MAX_RUNS: usize = 50;

/// Disambiguates run ids minted within the same millisecond by one process.
static RUN_SEQ: AtomicU64 = AtomicU64::new(0);

/// A unique, time-sortable run id: `<unix_millis>-<seq>`. The millis prefix is zero-padded so ids
/// sort lexicographically by start time; the sequence disambiguates same-millisecond launches.
pub(crate) fn new_run_id() -> String {
    let ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let seq = RUN_SEQ.fetch_add(1, Ordering::Relaxed);
    format!("{ms:013}-{seq:04}")
}

/// The unix-millis start time encoded in a run id, or 0 if it can't be parsed.
pub(crate) fn started_at(run_id: &str) -> u64 {
    run_id
        .split_once('-')
        .and_then(|(ms, _)| ms.parse().ok())
        .unwrap_or(0)
}

/// Spawn `argv` as a new detached run of `agent`, seeded by `message` (kept as the run's metadata).
/// Never blocks on a prior run — runs are independent, so an agent may have several live at once.
pub(crate) fn launch(
    agent: &StoredAgent,
    sessions_dir: &Path,
    base_dir: &Path,
    bin_dir: Option<&Path>,
    subdir: &str,
    argv: &[String],
    working_dir: Option<String>,
    message: &str,
    secret_env: &[(String, String)],
) -> Result<Launch> {
    let dir = agent_dir(sessions_dir, subdir, &agent.name);
    std::fs::create_dir_all(&dir)?;
    let run_id = new_run_id();

    // Metadata sidecar so the run list can show what each run was asked to do and when.
    let meta = serde_json::json!({ "started_at": started_at(&run_id), "message": message });
    let _ = std::fs::write(meta_path(&dir, &run_id), meta.to_string());

    let log = log_path_in(&dir, &run_id);
    let pid = spawn_child(&dir, &run_id, &log, base_dir, bin_dir, argv, working_dir.as_deref(), secret_env)?;

    prune_old_runs(&dir);

    Ok(Launch::Process {
        command: display_command(argv),
        pid,
        log,
        run_id,
    })
}

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
    bin_dir: Option<&Path>,
    argv: &[String],
    working_dir: Option<&str>,
    secret_env: &[(String, String)],
) -> Result<u32> {
    let log_file = File::create(log)?;
    let errlog = log_file.try_clone()?;
    let (program, command_args) = argv
        .split_first()
        .ok_or_else(|| Error::Launch("backend built an empty command".to_string()))?;

    let mut command = Command::new(program);
    command
        .args(command_args)
        // Injected secrets go in first, under their literal names; `PATH` is set right after so
        // a secret can never shadow the tool path.
        .envs(secret_env.iter().map(|(k, v)| (k, v)))
        .env("PATH", augmented_path(bin_dir))
        .process_group(0)
        .stdin(Stdio::null())
        .stdout(Stdio::from(log_file))
        .stderr(Stdio::from(errlog));
    // The agent's own `working_dir` wins; otherwise a run starts in `base_dir` (the ADI mono store
    // root), not the launching daemon's cwd.
    if let Some(d) = working_dir.filter(|d| !d.trim().is_empty()) {
        command.current_dir(d);
    } else {
        command.current_dir(base_dir);
    }

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

/// Every run of `agent` under `subdir`, newest first.
pub(crate) fn list_runs(sessions_dir: &Path, subdir: &str, agent_name: &str) -> Vec<RunInfo> {
    let dir = agent_dir(sessions_dir, subdir, agent_name);
    let mut ids = run_ids(&dir);
    ids.sort_unstable();
    ids.reverse();
    ids.into_iter()
        .map(|run_id| {
            let (meta_started, message) = read_meta(&dir, &run_id);
            RunInfo {
                running: read_pid(&pid_path_in(&dir, &run_id)).is_some_and(pid_alive),
                started_at: if meta_started > 0 {
                    meta_started
                } else {
                    started_at(&run_id)
                },
                message,
                run_id,
            }
        })
        .collect()
}

/// Whether any run of `agent` is still alive.
#[must_use]
pub(crate) fn any_running(sessions_dir: &Path, subdir: &str, agent_name: &str) -> bool {
    let dir = agent_dir(sessions_dir, subdir, agent_name);
    run_ids(&dir)
        .iter()
        .any(|id| read_pid(&pid_path_in(&dir, id)).is_some_and(pid_alive))
}

/// Whether one specific run is still alive.
#[must_use]
pub(crate) fn is_running(
    sessions_dir: &Path,
    subdir: &str,
    agent_name: &str,
    run_id: &str,
) -> bool {
    let dir = agent_dir(sessions_dir, subdir, agent_name);
    read_pid(&pid_path_in(&dir, run_id)).is_some_and(pid_alive)
}

/// Stop one specific run, returning whether a live run was found and signalled.
pub(crate) fn stop(
    sessions_dir: &Path,
    subdir: &str,
    agent_name: &str,
    run_id: &str,
) -> Result<bool> {
    let dir = agent_dir(sessions_dir, subdir, agent_name);
    let pid_file = pid_path_in(&dir, run_id);
    let Some(pid) = read_pid(&pid_file) else {
        return Ok(false);
    };
    if !pid_alive(pid) {
        let _ = std::fs::remove_file(&pid_file);
        return Ok(false);
    }

    signal_group(pid, "TERM")?;
    // A cooperative CLI normally exits immediately. A short bounded wait keeps the PID file in
    // place when it does not, and the reaper removes it once a child launched here exits.
    for _ in 0..20 {
        if !pid_alive(pid) {
            let _ = std::fs::remove_file(&pid_file);
            break;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    Ok(true)
}

/// The log path of one run — the `tail -f` target the live view shows.
pub(crate) fn log_path(
    sessions_dir: &Path,
    subdir: &str,
    agent_name: &str,
    run_id: &str,
) -> PathBuf {
    log_path_in(&agent_dir(sessions_dir, subdir, agent_name), run_id)
}

/// The tail of one run's combined log (stdout+stderr): up to `max_bytes` from the end, or `None`
/// when it has no log. A mid-file cut drops its partial first line, trailing whitespace is trimmed,
/// and invalid UTF-8 is replaced rather than failing — a best-effort snapshot, not a strict decode.
pub(crate) fn tail_log(
    sessions_dir: &Path,
    subdir: &str,
    agent_name: &str,
    run_id: &str,
    max_bytes: u64,
) -> Option<String> {
    let path = log_path(sessions_dir, subdir, agent_name, run_id);
    let mut file = File::open(&path).ok()?;
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

// ---- paths & bookkeeping -----------------------------------------------------------

pub(crate) fn agent_dir(sessions_dir: &Path, subdir: &str, agent_name: &str) -> PathBuf {
    sessions_dir.join(subdir).join(agent_name)
}

pub(crate) fn log_path_in(dir: &Path, run_id: &str) -> PathBuf {
    dir.join(format!("{run_id}.log"))
}

pub(crate) fn pid_path_in(dir: &Path, run_id: &str) -> PathBuf {
    dir.join(format!("{run_id}.pid"))
}

pub(crate) fn meta_path(dir: &Path, run_id: &str) -> PathBuf {
    dir.join(format!("{run_id}.json"))
}

/// All run ids present in an agent dir, derived from their `.log` files.
fn run_ids(dir: &Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().into_string().ok()?;
            name.strip_suffix(".log").map(ToString::to_string)
        })
        .collect()
}

/// The `(started_at, message)` recorded for a run, defaulting to `(0, "")` when absent or unreadable.
fn read_meta(dir: &Path, run_id: &str) -> (u64, String) {
    let Ok(text) = std::fs::read_to_string(meta_path(dir, run_id)) else {
        return (0, String::new());
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) else {
        return (0, String::new());
    };
    let started = value
        .get("started_at")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0);
    let message = value
        .get("message")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .to_string();
    (started, message)
}

/// Keep only the newest `MAX_RUNS` runs, deleting older *finished* runs' files. A run that is somehow
/// still alive is never pruned.
pub(crate) fn prune_old_runs(dir: &Path) {
    let mut ids = run_ids(dir);
    if ids.len() <= MAX_RUNS {
        return;
    }
    ids.sort_unstable(); // oldest first
    let excess = ids.len() - MAX_RUNS;
    for run_id in ids.into_iter().take(excess) {
        if read_pid(&pid_path_in(dir, &run_id)).is_some_and(pid_alive) {
            continue;
        }
        let _ = std::fs::remove_file(log_path_in(dir, &run_id));
        let _ = std::fs::remove_file(meta_path(dir, &run_id));
        let _ = std::fs::remove_file(pid_path_in(dir, &run_id));
    }
}

pub(crate) fn read_pid(path: &Path) -> Option<u32> {
    std::fs::read_to_string(path).ok()?.trim().parse().ok()
}

pub(crate) fn pid_alive(pid: u32) -> bool {
    Command::new("/bin/kill")
        .args(["-0", &pid.to_string()])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

fn signal_group(pid: u32, signal: &str) -> Result<()> {
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

fn augmented_path(bin_dir: Option<&Path>) -> String {
    let mut parts = Vec::new();
    // The agent's own `.bin` (its enabled tools) comes first, so it runs those tools by name.
    if let Some(dir) = bin_dir {
        parts.push(dir.display().to_string());
    }
    if let Ok(home) = std::env::var("HOME") {
        parts.extend([
            format!("{home}/.local/bin"),
            format!("{home}/bin"),
            format!("{home}/.cargo/bin"),
        ]);
    }
    parts.extend([
        "/opt/homebrew/bin".to_string(),
        "/usr/local/bin".to_string(),
        "/usr/bin".to_string(),
        "/bin".to_string(),
    ]);
    if let Ok(existing) = std::env::var("PATH") {
        parts.push(existing);
    }
    parts.join(":")
}

pub(crate) fn display_command(argv: &[String]) -> String {
    argv.iter()
        .map(|arg| {
            if arg
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || "-._/:=".contains(c))
            {
                arg.clone()
            } else {
                format!("'{}'", arg.replace('\'', "'\\''"))
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::StoredAgentManifest;

    fn scratch_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "adi-agents-detached-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    fn agent(name: &str) -> StoredAgent {
        StoredAgent {
            name: name.into(),
            manifest: StoredAgentManifest::default(),
        }
    }

    #[test]
    fn each_run_is_independent_and_recorded_in_history() {
        let sessions = scratch_dir("history");
        let a = agent("sleeper");
        // Two runs of the same agent, launched back to back — both must be live at once.
        let r1 = launch(
            &a,
            &sessions,
            &sessions,
            None,
            "harness",
            &["/bin/sleep".into(), "10".into()],
            None,
            "task one",
            &[],
        )
        .expect("run 1");
        let r2 = launch(
            &a,
            &sessions,
            &sessions,
            None,
            "harness",
            &["/bin/sleep".into(), "10".into()],
            None,
            "task two",
            &[],
        )
        .expect("run 2");
        let (id1, id2) = match (&r1, &r2) {
            (Launch::Process { run_id: a, .. }, Launch::Process { run_id: b, .. }) => {
                (a.clone(), b.clone())
            }
            _ => panic!("detached launch must be Launch::Process"),
        };
        assert_ne!(id1, id2, "each run gets its own id");
        assert!(any_running(&sessions, "harness", "sleeper"));
        assert!(is_running(&sessions, "harness", "sleeper", &id1));
        assert!(is_running(&sessions, "harness", "sleeper", &id2));

        // History lists both, newest first, with the tasks they were launched with.
        let runs = list_runs(&sessions, "harness", "sleeper");
        assert_eq!(runs.len(), 2);
        assert_eq!(runs[0].run_id, id2);
        assert_eq!(runs[0].message, "task two");
        assert_eq!(runs[1].message, "task one");
        assert!(runs.iter().all(|r| r.running));

        // Stopping one run leaves the other alive and keeps both in history.
        assert!(stop(&sessions, "harness", "sleeper", &id1).expect("stop run 1"));
        for _ in 0..40 {
            if !is_running(&sessions, "harness", "sleeper", &id1) {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(!is_running(&sessions, "harness", "sleeper", &id1));
        assert!(is_running(&sessions, "harness", "sleeper", &id2));
        assert_eq!(list_runs(&sessions, "harness", "sleeper").len(), 2);

        assert!(stop(&sessions, "harness", "sleeper", &id2).expect("stop run 2"));
        let _ = std::fs::remove_dir_all(sessions);
    }

    #[test]
    fn a_run_without_working_dir_starts_in_base_dir() {
        let sessions = scratch_dir("basedir-sessions");
        let base = scratch_dir("basedir-cwd");
        std::fs::create_dir_all(&base).unwrap();
        let a = agent("cwd-probe");

        // No explicit working_dir, so the run must start in base_dir — the child writes its cwd.
        let _ = launch(
            &a,
            &sessions,
            &base,
            None,
            "harness",
            &["/bin/sh".into(), "-c".into(), "pwd > cwd.txt".into()],
            None,
            "probe",
            &[],
        )
        .expect("launch");

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
        assert_eq!(got, want, "the run started in base_dir");

        let _ = std::fs::remove_dir_all(&sessions);
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn tail_log_reads_the_end_and_drops_a_partial_first_line() {
        let sessions = scratch_dir("tail");
        let dir = agent_dir(&sessions, "harness", "job");
        std::fs::create_dir_all(&dir).unwrap();
        // No such run → None, not an empty string.
        assert!(tail_log(&sessions, "harness", "job", "missing", 1024).is_none());

        std::fs::write(
            log_path_in(&dir, "0000000000001-0000"),
            "line one\nline two\n",
        )
        .unwrap();
        assert_eq!(
            tail_log(&sessions, "harness", "job", "0000000000001-0000", 1024).as_deref(),
            Some("line one\nline two")
        );
        // A tail that starts mid-file (inside "line one") drops that partial line.
        assert_eq!(
            tail_log(&sessions, "harness", "job", "0000000000001-0000", 13).as_deref(),
            Some("line two")
        );
        let _ = std::fs::remove_dir_all(sessions);
    }

    #[test]
    fn each_subdir_tracks_its_own_runs() {
        let sessions = scratch_dir("isolation");
        let a = agent("sleeper");
        let launched = launch(
            &a,
            &sessions,
            &sessions,
            None,
            "harness",
            &["/bin/sleep".into(), "10".into()],
            None,
            "go",
            &[],
        )
        .expect("launch under harness");
        let run_id = match launched {
            Launch::Process { run_id, .. } => run_id,
            _ => panic!("expected Launch::Process"),
        };
        // The run is filed under `harness/`, so the `process/` executor must not see it.
        assert!(is_running(&sessions, "harness", "sleeper", &run_id));
        assert!(!any_running(&sessions, "process", "sleeper"));
        assert!(stop(&sessions, "harness", "sleeper", &run_id).expect("stop"));
        let _ = std::fs::remove_dir_all(sessions);
    }
}
