//! Runs and supervises each service's local `runner` process so the proxy's upstreams are
//! alive. One task per [`RunnerSpec`]: run via `sh -c` in its own process group, relaunch
//! per [`RestartPolicy`] with exponential backoff; shutdown `SIGTERM`s then `SIGKILL`s the group.

use std::collections::BTreeMap;
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

/// A process that ran at least this long before exiting is treated as healthy, so its backoff resets.
const STABLE_RUNTIME: Duration = Duration::from_secs(10);

/// How long to wait after `SIGTERM` before escalating to `SIGKILL` at shutdown.
const TERM_GRACE: Duration = Duration::from_secs(5);

/// One supervised runner: the spec it was started from (so a reload can tell whether it
/// changed), its private stop signal, and the task driving it.
#[derive(Debug)]
struct Running {
    spec: RunnerSpec,
    shutdown: watch::Sender<bool>,
    task: JoinHandle<()>,
}

/// Owns the supervised runner tasks, keyed by service name.
///
/// Each runner gets its **own** stop signal rather than sharing one, which is what lets
/// [`Supervisor::reconcile`] add or remove a single service while the others keep running
/// untouched — the property hot reload depends on.
#[derive(Debug, Default)]
pub struct Supervisor {
    running: BTreeMap<String, Running>,
}

impl Supervisor {
    /// Spawn a supervised task per spec, returning immediately; an empty `specs` makes
    /// [`Supervisor::shutdown`] a no-op.
    #[must_use]
    pub fn start(specs: Vec<RunnerSpec>) -> Self {
        let mut supervisor = Self::default();
        supervisor.reconcile(specs);
        supervisor
    }

    /// Bring the running set in line with `specs`: start services that appeared, stop ones that
    /// vanished, and restart any whose definition changed. Services whose spec is byte-identical
    /// are left strictly alone — no restart, no dropped connections.
    ///
    /// Returns `(started, stopped)` counts, for logging.
    pub fn reconcile(&mut self, specs: Vec<RunnerSpec>) -> (usize, usize) {
        let mut wanted: BTreeMap<String, RunnerSpec> =
            specs.into_iter().map(|s| (s.name.clone(), s)).collect();

        // Drop anything no longer wanted, or wanted differently — the changed ones are restarted
        // below, since they are re-added to `wanted`'s survivors by not being removed here.
        let stale: Vec<String> = self
            .running
            .iter()
            .filter(|(name, r)| wanted.get(*name) != Some(&r.spec))
            .map(|(name, _)| name.clone())
            .collect();
        let stopped = stale.len();
        for name in stale {
            if let Some(r) = self.running.remove(&name) {
                info!(service = %name, "stopping runner (removed or changed)");
                // Signal and let the task wind the child down on its own; awaiting here would
                // block the reload loop for up to the SIGTERM grace period.
                let _ = r.shutdown.send(true);
            }
        }

        // Whatever is not already running is new (or was just stopped for a respec).
        wanted.retain(|name, _| !self.running.contains_key(name));
        let started = wanted.len();
        for (name, spec) in wanted {
            info!(service = %name, "starting runner");
            let (shutdown, rx) = watch::channel(false);
            let task = tokio::spawn(supervise(spec.clone(), rx));
            self.running.insert(
                name,
                Running {
                    spec,
                    shutdown,
                    task,
                },
            );
        }

        (started, stopped)
    }

    /// The number of runners currently supervised.
    #[must_use]
    pub fn len(&self) -> usize {
        self.running.len()
    }

    /// Signal every runner to stop, then wait for them all to terminate.
    pub async fn shutdown(self) {
        for r in self.running.values() {
            let _ = r.shutdown.send(true);
        }
        for (_, r) in self.running {
            let _ = r.task.await;
        }
    }
}

/// Supervise one runner: (re)launch and relaunch per policy until the policy says stop or shutdown is requested.
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
    // Env parity with adi-app's launcher (shared in adi-config): an augmented PATH (+ HOME) so a
    // runner spawned under launchd's bare environment still finds bun/node/Homebrew/docker. Applied
    // before `spec.env`, so a service's own `environment.static` can still override PATH/HOME.
    for (key, value) in adi_config::launch_env() {
        cmd.env(key, value);
    }
    for (key, value) in &spec.env {
        cmd.env(key, value);
    }
    // Kill the direct child if this task is dropped; the shutdown group-kill covers grandchildren.
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

/// Stop a running child at shutdown: `SIGTERM` its process group, wait a grace period, then `SIGKILL` if still up (direct kill when off unix or no pid).
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

/// Send a signal to a whole process group (the runner leads its group, so a negative pid targets it).
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

/// Sleep for `dur`, or return early if shutdown is requested; returns `true` if cut short by shutdown.
async fn sleep_or_shutdown(dur: Duration, rx: &mut watch::Receiver<bool>) -> bool {
    tokio::select! {
        () = tokio::time::sleep(dur) => false,
        _ = rx.changed() => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A spec that runs a command long enough to still be alive when the test inspects it.
    fn spec(name: &str, run: &str) -> RunnerSpec {
        RunnerSpec {
            name: name.to_string(),
            run: run.to_string(),
            working_dir: std::env::temp_dir(),
            env: Vec::new(),
            restart: RestartPolicy::Never,
        }
    }

    /// The core hot-reload contract: an added service starts, a removed one stops, and a service
    /// whose spec is unchanged is left strictly alone (same task, never bounced).
    #[tokio::test]
    async fn reconcile_adds_and_removes_without_touching_unchanged_services() {
        let mut sup = Supervisor::start(vec![spec("keep", "sleep 30"), spec("drop", "sleep 30")]);
        assert_eq!(sup.len(), 2);
        let keep_task = sup.running["keep"].task.id();

        // Swap `drop` out for `add`, leaving `keep` byte-identical.
        let (started, stopped) =
            sup.reconcile(vec![spec("keep", "sleep 30"), spec("add", "sleep 30")]);
        assert_eq!((started, stopped), (1, 1), "one added, one removed");
        assert_eq!(sup.len(), 2);
        assert!(sup.running.contains_key("add"), "the new service started");
        assert!(
            !sup.running.contains_key("drop"),
            "the removed service is gone"
        );
        assert_eq!(
            sup.running["keep"].task.id(),
            keep_task,
            "an unchanged service must keep its original task — no restart"
        );

        sup.shutdown().await;
    }

    /// A service whose definition changed is restarted, not left running the stale command.
    #[tokio::test]
    async fn reconcile_restarts_a_service_whose_spec_changed() {
        let mut sup = Supervisor::start(vec![spec("api", "sleep 30")]);
        let before = sup.running["api"].task.id();

        let (started, stopped) = sup.reconcile(vec![spec("api", "sleep 31")]);
        assert_eq!(
            (started, stopped),
            (1, 1),
            "respec counts as a stop then a start"
        );
        assert_eq!(sup.len(), 1);
        assert_ne!(
            sup.running["api"].task.id(),
            before,
            "a changed spec must produce a fresh task"
        );
        assert_eq!(sup.running["api"].spec.run, "sleep 31");

        sup.shutdown().await;
    }

    /// Reconciling to nothing stops everything; reconciling an empty supervisor is a no-op.
    #[tokio::test]
    async fn reconcile_handles_the_empty_cases() {
        let mut sup = Supervisor::default();
        assert_eq!(sup.reconcile(Vec::new()), (0, 0));

        sup.reconcile(vec![spec("only", "sleep 30")]);
        assert_eq!(sup.len(), 1);

        let (started, stopped) = sup.reconcile(Vec::new());
        assert_eq!((started, stopped), (0, 1));
        assert_eq!(sup.len(), 0);

        sup.shutdown().await;
    }

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
