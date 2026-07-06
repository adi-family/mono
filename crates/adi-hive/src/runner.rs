//! Runs and supervises each service's local `runner` process, so the upstreams the
//! proxy forwards to are actually alive — no manual `bun run dev`.
//!
//! One supervised task per [`RunnerSpec`]. Each task runs the command via `sh -c` in the
//! service's working directory, with `PORT*` and static env injected (see
//! [`crate::config`]), then loops: on exit it relaunches per the [`RestartPolicy`] with
//! an exponential backoff (reset once a process has run long enough to be considered
//! healthy). Every runner is spawned in its **own process group** so that at shutdown we
//! can signal the whole tree — the `sh`, the dev server, and any grandchildren it forked
//! — not just the top process. Shutdown is broadcast over a `watch` channel; each task
//! `SIGTERM`s its group, waits a grace period, then `SIGKILL`s if needed.

use std::time::{Duration, Instant};

use tokio::process::{Child, Command};
use tokio::sync::watch;
use tokio::task::JoinHandle;
use tracing::{info, warn};

use crate::config::{RestartPolicy, RunnerSpec};

/// First delay between relaunches; doubles on each successive crash.
const INITIAL_BACKOFF: Duration = Duration::from_millis(500);

/// Ceiling for the relaunch backoff.
const MAX_BACKOFF: Duration = Duration::from_secs(30);

/// A process that ran at least this long before exiting is treated as healthy, so its
/// backoff resets — otherwise a server that runs fine for hours then restarts would come
/// back only after the maximum delay.
const STABLE_RUNTIME: Duration = Duration::from_secs(10);

/// How long to wait after `SIGTERM` before escalating to `SIGKILL` at shutdown.
const TERM_GRACE: Duration = Duration::from_secs(5);

/// Owns the supervised runner tasks and the channel that tells them to stop.
#[derive(Debug)]
pub struct Supervisor {
    shutdown: watch::Sender<bool>,
    tasks: Vec<JoinHandle<()>>,
}

impl Supervisor {
    /// Spawn a supervised task per spec. Returns immediately; the runners come up in the
    /// background. An empty `specs` yields a supervisor whose [`Supervisor::shutdown`] is
    /// a no-op.
    #[must_use]
    pub fn start(specs: Vec<RunnerSpec>) -> Self {
        let (shutdown, rx) = watch::channel(false);
        let tasks = specs
            .into_iter()
            .map(|spec| tokio::spawn(supervise(spec, rx.clone())))
            .collect();
        Self { shutdown, tasks }
    }

    /// Signal every runner to stop, then wait for them all to terminate (each task kills
    /// its process group before returning).
    pub async fn shutdown(self) {
        let _ = self.shutdown.send(true);
        for task in self.tasks {
            let _ = task.await;
        }
    }
}

/// Supervise one runner: (re)launch it, wait, and relaunch per policy until either the
/// policy says stop or shutdown is requested.
async fn supervise(spec: RunnerSpec, mut rx: watch::Receiver<bool>) {
    let mut backoff = INITIAL_BACKOFF;
    loop {
        if *rx.borrow_and_update() {
            return;
        }

        let started = Instant::now();
        let mut child = match spawn(&spec) {
            Ok(child) => child,
            Err(e) => {
                warn!(service = %spec.name, error = %e, dir = %spec.working_dir.display(),
                      "failed to spawn runner; will retry");
                if sleep_or_shutdown(backoff, &mut rx).await {
                    return;
                }
                backoff = next_backoff(backoff);
                continue;
            }
        };
        let pid = child.id();
        info!(service = %spec.name, pid = ?pid, cmd = %spec.run,
              dir = %spec.working_dir.display(), "runner started");

        let exited_cleanly = tokio::select! {
            status = child.wait() => match status {
                Ok(status) => {
                    info!(service = %spec.name, code = ?status.code(), "runner exited");
                    status.success()
                }
                Err(e) => {
                    warn!(service = %spec.name, error = %e, "waiting on runner failed");
                    false
                }
            },
            _ = rx.changed() => {
                info!(service = %spec.name, "stopping runner");
                stop_child(&mut child, pid).await;
                return;
            }
        };

        match spec.restart {
            RestartPolicy::Never => {
                info!(service = %spec.name, "runner done (restart: no)");
                return;
            }
            RestartPolicy::OnFailure if exited_cleanly => {
                info!(service = %spec.name, "runner exited cleanly (restart: on-failure)");
                return;
            }
            _ => {}
        }

        if started.elapsed() >= STABLE_RUNTIME {
            backoff = INITIAL_BACKOFF;
        }
        warn!(service = %spec.name, delay = ?backoff, "relaunching runner");
        if sleep_or_shutdown(backoff, &mut rx).await {
            return;
        }
        backoff = next_backoff(backoff);
    }
}

/// Build and spawn the child in its own process group with the runner's env and cwd.
fn spawn(spec: &RunnerSpec) -> std::io::Result<Child> {
    let mut cmd = shell_command(&spec.run);
    cmd.current_dir(&spec.working_dir);
    for (key, value) in &spec.env {
        cmd.env(key, value);
    }
    // Kill the direct child if this task is dropped; the group kill at shutdown covers
    // grandchildren.
    cmd.kill_on_drop(true);
    #[cfg(unix)]
    {
        // Become a process-group leader (pgid == pid) so we can signal the whole tree.
        cmd.process_group(0);
    }
    cmd.spawn()
}

#[cfg(unix)]
fn shell_command(run: &str) -> Command {
    let mut cmd = Command::new("sh");
    cmd.arg("-c").arg(run);
    cmd
}

#[cfg(not(unix))]
fn shell_command(run: &str) -> Command {
    let mut cmd = Command::new("cmd");
    cmd.arg("/C").arg(run);
    cmd
}

/// Stop a running child at shutdown: on unix, `SIGTERM` its process group, wait a grace
/// period, then `SIGKILL` the group if it's still up. Falls back to a direct kill when
/// there's no pid or off unix.
async fn stop_child(child: &mut Child, pid: Option<u32>) {
    #[cfg(unix)]
    if let Some(pid) = pid {
        signal_group(pid, "TERM");
        if tokio::time::timeout(TERM_GRACE, child.wait())
            .await
            .is_err()
        {
            warn!(pid, "runner did not exit on SIGTERM; sending SIGKILL");
            signal_group(pid, "KILL");
            let _ = child.wait().await;
        }
        return;
    }
    let _ = child.start_kill();
    let _ = child.wait().await;
}

/// Send a signal to a whole process group. The runner is its group's leader, so the
/// group id equals its pid; a negative pid targets the group (`kill -TERM -<pid>`).
#[cfg(unix)]
fn signal_group(pid: u32, signal: &str) {
    let _ = std::process::Command::new("kill")
        .arg(format!("-{signal}"))
        .arg(format!("-{pid}"))
        .status();
}

fn next_backoff(current: Duration) -> Duration {
    (current * 2).min(MAX_BACKOFF)
}

/// Sleep for `dur`, or return early if shutdown is requested. Returns `true` if it was
/// cut short by shutdown.
async fn sleep_or_shutdown(dur: Duration, rx: &mut watch::Receiver<bool>) -> bool {
    tokio::select! {
        () = tokio::time::sleep(dur) => false,
        _ = rx.changed() => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_doubles_and_saturates() {
        assert_eq!(
            next_backoff(Duration::from_millis(500)),
            Duration::from_secs(1)
        );
        assert_eq!(next_backoff(Duration::from_secs(20)), MAX_BACKOFF);
        assert_eq!(next_backoff(MAX_BACKOFF), MAX_BACKOFF);
    }
}
