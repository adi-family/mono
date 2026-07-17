//! Detached-process lifecycle shared by the `process` and `harness` executors.
//!
//! Both spawn a headless CLI in its own process group, record a PID and combined log under a
//! per-executor subdir of the sessions dir (`process/`, `harness/`), and reap it. The only thing
//! that differs between the executors is that subdir and the command they build, so the machinery
//! lives here once and each executor passes its own `subdir`.

use std::fs::File;
use std::os::unix::process::CommandExt as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use crate::StoredAgent;
use crate::error::{Error, Result};
use crate::run::Launch;

/// Spawn `argv` detached, tracking it under `<sessions_dir>/<subdir>/<agent>.{pid,log}`.
pub(crate) fn launch(
    agent: &StoredAgent,
    sessions_dir: &Path,
    subdir: &str,
    argv: &[String],
    working_dir: Option<String>,
) -> Result<Launch> {
    let runtime_dir = runtime_dir(sessions_dir, subdir);
    std::fs::create_dir_all(&runtime_dir)?;
    let pid_file = pid_path(sessions_dir, subdir, &agent.name);
    if let Some(pid) = read_pid(&pid_file) {
        if pid_alive(pid) {
            return Err(Error::AlreadyRunning(agent.name.clone()));
        }
        let _ = std::fs::remove_file(&pid_file);
    }

    let log = log_path(sessions_dir, subdir, &agent.name);
    let log_file = File::create(&log)?;
    let errlog = log_file.try_clone()?;
    let (program, command_args) = argv
        .split_first()
        .ok_or_else(|| Error::Launch(format!("{subdir} backend built an empty command")))?;

    let mut command = Command::new(program);
    command
        .args(command_args)
        .env("PATH", augmented_path())
        .process_group(0)
        .stdin(Stdio::null())
        .stdout(Stdio::from(log_file))
        .stderr(Stdio::from(errlog));
    if let Some(dir) = working_dir.filter(|dir| !dir.trim().is_empty()) {
        command.current_dir(dir);
    } else if let Ok(home) = std::env::var("HOME") {
        command.current_dir(home);
    }

    let mut child = command
        .spawn()
        .map_err(|e| Error::Launch(format!("couldn't spawn {program}: {e}")))?;
    let pid = child.id();
    if let Err(e) = std::fs::write(&pid_file, format!("{pid}\n")) {
        let _ = child.kill();
        return Err(Error::Io(e));
    }

    // Long-lived app servers must reap completed children. In the short-lived CLI this helper
    // thread naturally disappears when the CLI exits and the OS adopts the still-running child.
    let reaper_pid_file = pid_file.clone();
    std::thread::spawn(move || {
        let _ = child.wait();
        if read_pid(&reaper_pid_file) == Some(pid) {
            let _ = std::fs::remove_file(reaper_pid_file);
        }
    });

    Ok(Launch::Process {
        command: display_command(argv),
        pid,
        log,
    })
}

#[must_use]
pub(crate) fn is_running(sessions_dir: &Path, subdir: &str, agent_name: &str) -> bool {
    read_pid(&pid_path(sessions_dir, subdir, agent_name)).is_some_and(pid_alive)
}

pub(crate) fn stop(sessions_dir: &Path, subdir: &str, agent_name: &str) -> Result<bool> {
    let pid_file = pid_path(sessions_dir, subdir, agent_name);
    let Some(pid) = read_pid(&pid_file) else {
        return Ok(false);
    };
    if !pid_alive(pid) {
        let _ = std::fs::remove_file(pid_file);
        return Ok(false);
    }

    signal_group(pid, "TERM")?;
    // A cooperative CLI normally exits immediately. A short bounded wait keeps the PID file in
    // place when it does not, preventing a second run from overlapping a process still shutting
    // down. The reaper removes it once a child launched by this process exits.
    for _ in 0..20 {
        if !pid_alive(pid) {
            let _ = std::fs::remove_file(&pid_file);
            break;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    Ok(true)
}

fn runtime_dir(sessions_dir: &Path, subdir: &str) -> PathBuf {
    sessions_dir.join(subdir)
}

fn pid_path(sessions_dir: &Path, subdir: &str, agent_name: &str) -> PathBuf {
    runtime_dir(sessions_dir, subdir).join(format!("{agent_name}.pid"))
}

fn log_path(sessions_dir: &Path, subdir: &str, agent_name: &str) -> PathBuf {
    runtime_dir(sessions_dir, subdir).join(format!("{agent_name}.log"))
}

fn read_pid(path: &Path) -> Option<u32> {
    std::fs::read_to_string(path).ok()?.trim().parse().ok()
}

fn pid_alive(pid: u32) -> bool {
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

fn augmented_path() -> String {
    let mut parts = Vec::new();
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

fn display_command(argv: &[String]) -> String {
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

    #[test]
    fn detached_process_is_recorded_and_stoppable() {
        let sessions = scratch_dir("lifecycle");
        let agent = StoredAgent {
            name: "sleeper".into(),
            manifest: StoredAgentManifest {
                backend: "process:test".into(),
                ..StoredAgentManifest::default()
            },
        };
        let launch = launch(
            &agent,
            &sessions,
            "process",
            &["/bin/sleep".into(), "10".into()],
            None,
        )
        .expect("launch");
        assert!(matches!(
            launch,
            Launch::Process { pid, ref log, .. } if pid > 0 && log.is_file()
        ));
        assert!(is_running(&sessions, "process", "sleeper"));
        assert!(stop(&sessions, "process", "sleeper").expect("stop"));
        for _ in 0..20 {
            if !is_running(&sessions, "process", "sleeper") {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(!is_running(&sessions, "process", "sleeper"));
        let _ = std::fs::remove_dir_all(sessions);
    }

    #[test]
    fn each_subdir_tracks_its_own_runs() {
        let sessions = scratch_dir("isolation");
        let agent = StoredAgent {
            name: "sleeper".into(),
            manifest: StoredAgentManifest::default(),
        };
        launch(
            &agent,
            &sessions,
            "harness",
            &["/bin/sleep".into(), "10".into()],
            None,
        )
        .expect("launch under harness");
        // The PID is filed under `harness/`, so the `process/` executor must not see it running.
        assert!(is_running(&sessions, "harness", "sleeper"));
        assert!(!is_running(&sessions, "process", "sleeper"));
        assert!(stop(&sessions, "harness", "sleeper").expect("stop"));
        let _ = std::fs::remove_dir_all(sessions);
    }
}
