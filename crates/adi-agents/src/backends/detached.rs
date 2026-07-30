//! Detached-process lifecycle shared by the `process` and `harness` executors.
//!
//! Each launch is an independent *run*: the agent definition is only a template, so a fresh run is
//! spawned every time (never continuing a prior one), several runs of the same agent may be live at
//! once, and every run keeps its own PID, log, and metadata under a per-agent directory —
//! `<sessions>/<subdir>/<agent>/<run_id>.{pid,log,json}`. A run owns that whole `<run_id>.*`
//! namespace, sidecars included, which is what deleting and pruning sweep. Finished runs persist so
//! their output stays browsable as history; the oldest are pruned once the count passes `MAX_RUNS`.

use std::collections::BTreeMap;
use std::fs::File;
use std::io::{Read as _, Seek as _, SeekFrom};
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
    run_path: &str,
    subdir: &str,
    argv: &[String],
    message: &str,
    run_env: &[(String, String)],
) -> Result<Launch> {
    let dir = agent_dir(sessions_dir, subdir, &agent.name);
    std::fs::create_dir_all(&dir)?;
    let run_id = new_run_id();

    // Metadata sidecar so the run list can show what each run was asked to do and when.
    let meta = serde_json::json!({ "started_at": started_at(&run_id), "message": message });
    let _ = std::fs::write(meta_path(&dir, &run_id), meta.to_string());

    let log = log_path_in(&dir, &run_id);
    let pid = spawn_child(&dir, &run_id, &log, base_dir, run_path, argv, run_env)?;

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

/// Every run of `agent` under `subdir`, newest first.
pub(crate) fn list_runs(sessions_dir: &Path, subdir: &str, agent_name: &str) -> Vec<RunInfo> {
    let dir = agent_dir(sessions_dir, subdir, agent_name);
    let touched = last_activity(&dir);
    let mut ids = run_ids(&dir);
    ids.sort_unstable();
    ids.reverse();
    ids.into_iter()
        .map(|run_id| {
            let meta = read_meta(&dir, &run_id);
            let started = if meta.started_at > 0 {
                meta.started_at
            } else {
                started_at(&run_id)
            };
            RunInfo {
                running: read_pid(&pid_path_in(&dir, &run_id)).is_some_and(pid_alive),
                // A run that left no readable mtime behind falls back to when it began, so the
                // field is always a time the run existed rather than the epoch.
                last_activity: touched.get(&run_id).copied().unwrap_or(0).max(started),
                started_at: started,
                message: meta.message,
                hidden: meta.hidden,
                run_id,
            }
        })
        .collect()
}

/// When each run in `dir` last did anything, as unix millis keyed by run id.
///
/// A run owns the whole `<run_id>.*` namespace of its agent dir, and the files it writes as it works
/// — the combined log, a harness conversation's transcript — are appended to for as long as it is
/// talking. So the newest mtime across a run's files is the last moment it moved. Runs whose files
/// carry no readable mtime are simply absent; the caller falls back to their start time.
///
/// The metadata sidecar is the one file left out: it is written at launch (which `started_at`
/// already reports) and then only when a *reader* changes its mind about the run — hiding it from
/// the rail. Counting that as activity would have a hide read as the run having just moved.
fn last_activity(dir: &Path) -> BTreeMap<String, u64> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return BTreeMap::new();
    };
    let mut newest: BTreeMap<String, u64> = BTreeMap::new();
    for entry in entries.flatten() {
        let Ok(name) = entry.file_name().into_string() else {
            continue;
        };
        // Run ids hold no dots (`{millis}-{seq}`), so everything before the first one is the id.
        let Some((run_id, rest)) = name.split_once('.') else {
            continue;
        };
        if rest == META_EXT {
            continue;
        }
        let Some(ms) = entry
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .and_then(|d| u64::try_from(d.as_millis()).ok())
        else {
            continue;
        };
        let slot = newest.entry(run_id.to_string()).or_default();
        *slot = (*slot).max(ms);
    }
    newest
}

/// How many runs under `subdir` are alive right now, per agent — what the concurrency caps count.
/// Read from the PID files themselves rather than per-agent run lists: a finished run drops its PID
/// file, so the live ones are exactly the files still naming a living process. An agent with nothing
/// running is left out, so the map is as small as the load is.
#[must_use]
pub(crate) fn running_by_agent(sessions_dir: &Path, subdir: &str) -> BTreeMap<String, usize> {
    let Ok(agents) = std::fs::read_dir(sessions_dir.join(subdir)) else {
        return BTreeMap::new();
    };
    agents
        .flatten()
        .filter(|entry| entry.path().is_dir())
        .filter_map(|entry| {
            let name = entry.file_name().into_string().ok()?;
            let live = count_running_in(&entry.path());
            (live > 0).then_some((name, live))
        })
        .collect()
}

/// The live runs in one agent's run directory.
fn count_running_in(dir: &Path) -> usize {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension().is_some_and(|ext| ext == "pid") && read_pid(path).is_some_and(pid_alive)
        })
        .count()
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

/// The extension of a run's metadata sidecar, as it appears after the run id in the file name.
/// A harness conversation's `.queue.json` is a different sidecar and reads as `queue.json`.
const META_EXT: &str = "json";

pub(crate) fn meta_path(dir: &Path, run_id: &str) -> PathBuf {
    dir.join(format!("{run_id}.{META_EXT}"))
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

/// What a run's metadata sidecar records: when it began, what it was asked to do, and whether a
/// reader has hidden it from the chat rail.
#[derive(Default)]
struct RunMeta {
    started_at: u64,
    message: String,
    hidden: bool,
}

/// The metadata recorded for a run, all-default when the sidecar is absent or unreadable — an older
/// run with no sidecar simply reads as never hidden, with its start recovered from its run id.
fn read_meta(dir: &Path, run_id: &str) -> RunMeta {
    let Ok(text) = std::fs::read_to_string(meta_path(dir, run_id)) else {
        return RunMeta::default();
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) else {
        return RunMeta::default();
    };
    RunMeta {
        started_at: value
            .get("started_at")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0),
        message: value
            .get("message")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_string(),
        hidden: value
            .get("hidden")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false),
    }
}

/// Hide (or unhide) one run: a flag in its metadata sidecar, so the choice outlives the browser tab
/// that made it. Nothing about the run itself changes — it keeps running, keeps its log and
/// transcript, and is still listed by everything that asks for the full history.
///
/// Returns whether there was a run there to flag; an id with no files is `false`, so a stale click is
/// idempotent rather than an error. The sidecar is rewritten whole from what was read, which also
/// mints one for a run that predates it (its start recovered from its id, its message unknown —
/// which is exactly what the listing already showed for it).
pub(crate) fn set_hidden(
    sessions_dir: &Path,
    subdir: &str,
    agent_name: &str,
    run_id: &str,
    hidden: bool,
) -> Result<bool> {
    let dir = agent_dir(sessions_dir, subdir, agent_name);
    let meta_file = meta_path(&dir, run_id);
    if !log_path_in(&dir, run_id).exists() && !meta_file.exists() {
        return Ok(false);
    }
    let meta = read_meta(&dir, run_id);
    let started = if meta.started_at > 0 {
        meta.started_at
    } else {
        started_at(run_id)
    };
    let next = serde_json::json!({
        "started_at": started,
        "message": meta.message,
        "hidden": hidden,
    });
    std::fs::write(meta_file, next.to_string())?;
    Ok(true)
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
        remove_run_files(dir, &run_id);
    }
}

/// Delete one run outright: stop it if it is still live, then remove everything it owns. Returns
/// whether there was a run there to delete — an id with no files is `false`, so a double-click on
/// Delete is idempotent rather than an error.
pub(crate) fn delete(
    sessions_dir: &Path,
    subdir: &str,
    agent_name: &str,
    run_id: &str,
) -> Result<bool> {
    let dir = agent_dir(sessions_dir, subdir, agent_name);
    if !log_path_in(&dir, run_id).exists() && !meta_path(&dir, run_id).exists() {
        return Ok(false);
    }
    // Kill first, or the child outlives its own log and writes into a slot nothing is reading.
    stop(sessions_dir, subdir, agent_name, run_id)?;
    remove_run_files(&dir, run_id);
    Ok(true)
}

/// Remove every file a run owns. A run holds the whole `<run_id>.*` namespace of its agent dir —
/// `.log`, `.json`, `.pid`, and whatever sidecars a backend keeps beside them (a harness
/// conversation's `.jsonl` transcript and `.queue.json`) — so the sweep is by prefix rather than a
/// list of extensions that a new sidecar would silently fall off the end of.
fn remove_run_files(dir: &Path, run_id: &str) {
    let prefix = format!("{run_id}.");
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let Ok(name) = entry.file_name().into_string() else {
            continue;
        };
        if name.starts_with(&prefix) {
            let _ = std::fs::remove_file(entry.path());
        }
    }
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

#[cfg(not(unix))]
fn signal_group(pid: u32, _signal: &str) -> Result<()> {
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

    /// A long-running child command that exists on the test host. `/bin/sleep` on Unix;
    /// `ping -n 11 127.0.0.1` (~10s, runs headless with stdin nulled) on Windows.
    fn sleep_argv() -> Vec<String> {
        #[cfg(unix)]
        {
            vec!["/bin/sleep".into(), "10".into()]
        }
        #[cfg(not(unix))]
        {
            vec!["ping".into(), "-n".into(), "11".into(), "127.0.0.1".into()]
        }
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

    #[test]
    fn each_run_is_independent_and_recorded_in_history() {
        let sessions = scratch_dir("history");
        let a = agent("sleeper");
        // Two runs of the same agent, launched back to back — both must be live at once.
        let r1 = launch(
            &a,
            &sessions,
            &sessions,
            "",
            "harness",
            &sleep_argv(),
            "task one",
            &[],
        )
        .expect("run 1");
        let r2 = launch(
            &a,
            &sessions,
            &sessions,
            "",
            "harness",
            &sleep_argv(),
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

    /// A run's *last activity* is when its files last changed, not when it started — that is what
    /// makes a long, quiet conversation sort below one answered a minute ago. And it never precedes
    /// the start, so a run whose files somehow predate it still reads as a moment it existed.
    #[test]
    fn last_activity_follows_the_files_but_never_precedes_the_start() {
        let sessions = scratch_dir("activity");
        let dir = agent_dir(&sessions, "harness", "talker");
        std::fs::create_dir_all(&dir).unwrap();
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| u64::try_from(d.as_millis()).unwrap())
            .unwrap();

        // A run started an hour ago whose transcript was written just now: its log/transcript mtimes
        // are what the rail reads as "active", so it must land near now rather than an hour back.
        let old = format!("{:013}-0001", now - 3_600_000);
        std::fs::write(dir.join(format!("{old}.log")), "hello").unwrap();
        std::fs::write(dir.join(format!("{old}.jsonl")), "{}\n").unwrap();

        // A run whose id claims it starts in an hour — its files are already older than that.
        let future = format!("{:013}-0002", now + 3_600_000);
        std::fs::write(dir.join(format!("{future}.log")), "hello").unwrap();

        let runs = list_runs(&sessions, "harness", "talker");
        let by_id = |id: &str| runs.iter().find(|r| r.run_id == id).cloned().unwrap();

        let quiet = by_id(&old);
        assert_eq!(quiet.started_at, now - 3_600_000, "start comes from the id");
        assert!(
            quiet.last_activity >= now - 60_000,
            "last activity comes from the files, not the id: {} vs {now}",
            quiet.last_activity,
        );

        let odd = by_id(&future);
        assert_eq!(
            odd.last_activity, odd.started_at,
            "an mtime older than the start never drags last activity backwards",
        );

        let _ = std::fs::remove_dir_all(sessions);
    }

    /// `base_dir` is the run's directory, full stop — resolved once by `workspace::resolve` and not
    /// re-decided here. This is the check that a spawned child really lands in it.
    #[test]
    fn a_run_starts_in_base_dir() {
        let sessions = scratch_dir("basedir-sessions");
        let base = scratch_dir("basedir-cwd");
        std::fs::create_dir_all(&base).unwrap();
        let a = agent("cwd-probe");

        // The child writes its own cwd, so the assertion is on where it actually ran.
        let _ = launch(
            &a,
            &sessions,
            &base,
            "",
            "harness",
            &write_cwd_argv("cwd.txt"),
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

    /// Hiding a session is a flag in its metadata and nothing else: the run stays in the history with
    /// its task intact, the flag survives a re-read (which is what makes it survive a reload), and
    /// writing it never reads as the run having just moved — a hidden chat brought back must not have
    /// jumped to the top of the rail in the meantime.
    #[test]
    fn hiding_a_run_only_flags_it() {
        let sessions = scratch_dir("hidden");
        let dir = agent_dir(&sessions, "harness", "talker");
        std::fs::create_dir_all(&dir).unwrap();
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| u64::try_from(d.as_millis()).unwrap())
            .unwrap();

        // A conversation that started an hour ago and has been quiet since — the case the assertion
        // about activity needs, since `last_activity` never precedes a run's start.
        let run_id = format!("{:013}-0001", now - 3_600_000);
        std::fs::write(dir.join(format!("{run_id}.log")), "hello").unwrap();
        std::fs::write(dir.join(format!("{run_id}.jsonl")), "{}\n").unwrap();
        std::fs::write(
            meta_path(&dir, &run_id),
            serde_json::json!({ "started_at": now - 3_600_000, "message": "some task" })
                .to_string(),
        )
        .unwrap();
        // Its files are written just now, so back-date them all to when it was actually talking.
        let hour_ago = SystemTime::now() - Duration::from_secs(3_600);
        for entry in std::fs::read_dir(&dir).unwrap().flatten() {
            let file = File::options().write(true).open(entry.path()).unwrap();
            file.set_modified(hour_ago).unwrap();
        }

        let listed = |id: &str| {
            list_runs(&sessions, "harness", "talker")
                .into_iter()
                .find(|r| r.run_id == id)
                .expect("run is listed")
        };
        let before = listed(&run_id);
        assert!(!before.hidden, "a run nobody hid is not hidden");
        let backdated = before.last_activity;
        assert!(backdated < now - 60_000, "the files really are old");

        assert!(
            set_hidden(&sessions, "harness", "talker", &run_id, true).expect("hide"),
            "an existing run is there to flag",
        );
        let hidden = listed(&run_id);
        assert!(hidden.hidden, "the flag round-trips through the sidecar");
        assert_eq!(hidden.message, before.message, "the task is not lost");
        assert_eq!(hidden.started_at, before.started_at);
        assert_eq!(
            hidden.last_activity, backdated,
            "hiding is not activity — the sidecar's mtime is left out of it",
        );

        // Unhiding is the same write in reverse, and an unknown run is a quiet no-op.
        assert!(set_hidden(&sessions, "harness", "talker", &run_id, false).expect("unhide"));
        assert!(!listed(&run_id).hidden);
        assert!(
            !set_hidden(&sessions, "harness", "talker", "0000000000001-0000", true).expect("absent"),
            "a run that isn't there is nothing to flag",
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
            "",
            "harness",
            &sleep_argv(),
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
