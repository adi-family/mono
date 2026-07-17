//! Detached process lifecycle.

mod claude;
mod codex;

use std::fs::File;
use std::os::unix::process::CommandExt as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use crate::arguments::{ProcessClaudeArguments, ProcessCodexArguments};
use crate::backend::Backend;
use crate::error::{Error, Result};
use crate::run::Launch;
use crate::{StoredAgent, StoredAgentManifest};

const PROCESS_DIR: &str = "process";

#[must_use]
pub fn is_runnable(manifest: &StoredAgentManifest) -> bool {
    engine_run(manifest, "").is_ok()
}

pub fn launch(agent: &StoredAgent, sessions_dir: &Path, message: &str) -> Result<Launch> {
    let (argv, working_dir) = engine_run(&agent.manifest, message)?;
    spawn_detached(agent, sessions_dir, &argv, working_dir)
}

#[must_use]
pub fn is_running(sessions_dir: &Path, agent_name: &str) -> bool {
    read_pid(&pid_path(sessions_dir, agent_name)).is_some_and(pid_alive)
}

pub fn stop(sessions_dir: &Path, agent_name: &str) -> Result<bool> {
    let pid_file = pid_path(sessions_dir, agent_name);
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

fn engine_run(
    manifest: &StoredAgentManifest,
    message: &str,
) -> Result<(Vec<String>, Option<String>)> {
    match Backend::parse(&manifest.backend) {
        Some(Backend::ProcessClaude) => {
            let arguments = manifest.typed_arguments::<ProcessClaudeArguments>()?;
            let working_dir = arguments.working_dir.clone();
            Ok((claude::argv(&arguments, message), working_dir))
        }
        Some(Backend::ProcessCodex) => {
            let arguments = manifest.typed_arguments::<ProcessCodexArguments>()?;
            let working_dir = arguments.working_dir.clone();
            Ok((codex::argv(&arguments, message), working_dir))
        }
        _ => Err(Error::NotRunnable(manifest.backend.clone())),
    }
}

fn spawn_detached(
    agent: &StoredAgent,
    sessions_dir: &Path,
    argv: &[String],
    working_dir: Option<String>,
) -> Result<Launch> {
    let runtime_dir = sessions_dir.join(PROCESS_DIR);
    std::fs::create_dir_all(&runtime_dir)?;
    let pid_file = pid_path(sessions_dir, &agent.name);
    if let Some(pid) = read_pid(&pid_file) {
        if pid_alive(pid) {
            return Err(Error::AlreadyRunning(agent.name.clone()));
        }
        let _ = std::fs::remove_file(&pid_file);
    }

    let log = log_path(sessions_dir, &agent.name);
    let log_file = File::create(&log)?;
    let errlog = log_file.try_clone()?;
    let (program, command_args) = argv
        .split_first()
        .ok_or_else(|| Error::Launch("process backend built an empty command".into()))?;

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

fn runtime_dir(sessions_dir: &Path) -> PathBuf {
    sessions_dir.join(PROCESS_DIR)
}

fn pid_path(sessions_dir: &Path, agent_name: &str) -> PathBuf {
    runtime_dir(sessions_dir).join(format!("{agent_name}.pid"))
}

fn log_path(sessions_dir: &Path, agent_name: &str) -> PathBuf {
    runtime_dir(sessions_dir).join(format!("{agent_name}.log"))
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

    fn scratch_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "adi-agents-process-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn unknown_process_engines_are_not_runnable() {
        let manifest = StoredAgentManifest {
            backend: "process:unknown".into(),
            ..StoredAgentManifest::default()
        };
        assert!(matches!(
            engine_run(&manifest, "run"),
            Err(Error::NotRunnable(_))
        ));
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
        let launch = spawn_detached(&agent, &sessions, &["/bin/sleep".into(), "10".into()], None)
            .expect("launch");
        assert!(matches!(
            launch,
            Launch::Process { pid, ref log, .. } if pid > 0 && log.is_file()
        ));
        assert!(is_running(&sessions, "sleeper"));
        assert!(stop(&sessions, "sleeper").expect("stop"));
        for _ in 0..20 {
            if !is_running(&sessions, "sleeper") {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(!is_running(&sessions, "sleeper"));
        let _ = std::fs::remove_dir_all(sessions);
    }
}
