//! Runs and supervises each service's local `runner` process so the proxy's upstreams are
//! alive. One task per [`RunnerSpec`]: run via `sh -c` in its own process group, relaunch
//! per [`RestartPolicy`] with exponential backoff; shutdown `SIGTERM`s then `SIGKILL`s the group.
//!
//! A [`StartPolicy::OnDemand`](crate::config::StartPolicy::OnDemand) service is supervised by a
//! second state machine ([`supervise_on_demand`]) over the same spawn and stop primitives: it
//! launches nothing until the front door reports a request for it, and stops it again once its
//! idle window passes without one. See [`crate::demand`] for the state the two halves share.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::process::{Child, Command};
use tokio::sync::watch;
use tokio::task::JoinHandle;
use tracing::{info, warn};

use crate::config::{RestartPolicy, RunnerSpec};
use crate::demand::{Demand, Handle, Phase};

/// First delay between relaunches; doubles on each successive crash.
const INITIAL_BACKOFF: Duration = Duration::from_millis(500);

/// Ceiling for the relaunch backoff.
const MAX_BACKOFF: Duration = Duration::from_secs(30);

/// A process that ran at least this long before exiting is treated as healthy, so its backoff resets.
const STABLE_RUNTIME: Duration = Duration::from_secs(10);

/// How long to wait after `SIGTERM` before escalating to `SIGKILL` at shutdown. Unix-only: on
/// Windows shutdown force-terminates the tree with `taskkill /T /F` (no graceful grace period).
#[cfg(unix)]
const TERM_GRACE: Duration = Duration::from_secs(5);

/// How often a running on-demand service is asked whether it has gone quiet — and, while it is
/// still coming up, whether its port has begun to answer. Frequent enough that the holding page
/// turns into the real service on its next refresh; cheap, because a tick reads a timestamp and,
/// once the service is up, nothing else.
const DEMAND_TICK: Duration = Duration::from_millis(250);

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
    /// The on-demand registry: what the front door touches, and what every on-demand task waits on.
    demand: Arc<Demand>,
}

impl Supervisor {
    /// Spawn a supervised task per spec, returning immediately; an empty `specs` makes
    /// [`Supervisor::shutdown`] a no-op.
    ///
    /// `demand` is the registry the front door shares (see [`crate::demand`]): an on-demand
    /// service registers itself there as its task starts, which is what makes a request able to
    /// wake it. Pass a fresh [`Demand`] for a hive whose proxy is elsewhere — nothing breaks, the
    /// on-demand services simply never hear about a request and stay stopped.
    #[must_use]
    pub fn start(specs: Vec<RunnerSpec>, demand: Arc<Demand>) -> Self {
        let mut supervisor = Self {
            running: BTreeMap::new(),
            demand,
        };
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
                // A service that is going away must stop being wakeable, or the front door would
                // keep touching a name whose task is winding down. A service that only changed
                // registers again below, and — like any respec — comes back stopped: on demand,
                // the next request is what starts it.
                self.demand.forget(&name);
            }
        }

        // Whatever is not already running is new (or was just stopped for a respec).
        wanted.retain(|name, _| !self.running.contains_key(name));
        let started = wanted.len();
        for (name, spec) in wanted {
            let (shutdown, rx) = watch::channel(false);
            // An on-demand service is *registered*, not started: what follows is a task that waits
            // for the front door to report a request for it.
            let task = if spec.start.is_on_demand() {
                info!(service = %name, idle_stop = ?spec.idle_stop,
                      "registering on-demand runner (starts on the first request)");
                let handle = self.demand.register(&name);
                tokio::spawn(supervise_on_demand(spec.clone(), rx, handle))
            } else {
                info!(service = %name, "starting runner");
                tokio::spawn(supervise(spec.clone(), rx))
            };
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

    /// Whether nothing is being supervised — a hive that only routes.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.running.is_empty()
    }

    /// Signal every runner to stop, then wait for them all to terminate.
    pub async fn shutdown(self) {
        for r in self.running.values() {
            let _ = r.shutdown.send(true);
        }
        for (_, r) in self.running {
            let _ = r.task.await;
        }
        // The published phases describe processes this hive was running; it is not any more.
        self.demand.unpublish();
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

/// Supervise one **on-demand** runner: nothing is launched until the front door reports a request
/// for this service, and the process is stopped again once none has arrived for `spec.idle_stop`.
///
/// `restart:` is deliberately not read here. For an on-demand service the *request* is the restart
/// policy: a process that exits on its own — cleanly or not — leaves the service idle-stopped, and
/// the next visitor starts a fresh one (and is shown the holding page while it comes up). Relaunching
/// a crashed service nobody is looking at would be exactly the cost this policy exists to avoid.
async fn supervise_on_demand(spec: RunnerSpec, mut rx: watch::Receiver<bool>, mut handle: Handle) {
    // Set by a stop that a request interrupted too late to rescue: the visitor is already waiting,
    // so the next process starts without waiting to be asked again.
    let mut wanted_now = false;
    loop {
        if *rx.borrow_and_update() {
            return;
        }
        if !wanted_now {
            handle.set_phase(Phase::IdleStopped);
            tokio::select! {
                _ = rx.changed() => return,
                () = handle.wait_for_activity() => {}
            }
            if *rx.borrow_and_update() {
                return;
            }
        }
        wanted_now = false;

        handle.set_phase(Phase::Starting);
        // A start counts as activity: a service that takes a minute to boot must not be measured as
        // a minute of silence and stopped before it has answered anything.
        handle.touch();
        let mut child = match spawn(&spec) {
            Ok(child) => child,
            Err(e) => {
                warn!(service = %spec.name, error = %e, dir = %spec.working_dir.display(),
                      "failed to spawn on-demand runner");
                if sleep_or_shutdown(INITIAL_BACKOFF, &mut rx).await {
                    return;
                }
                continue;
            }
        };
        let pid = child.id();
        info!(service = %spec.name, pid = ?pid, cmd = %spec.run,
              dir = %spec.working_dir.display(), "on-demand runner started");

        // Run, drain, and — when a request rescues it mid-drain — go on running the same process.
        let ended = loop {
            match run_until_idle(&spec, &mut child, &mut rx, &mut handle).await {
                Awake::Shutdown => break Ended::Shutdown,
                Awake::Exited => break Ended::Stopped,
                Awake::Idle => match drain(&spec, &mut child, pid, &mut rx, &mut handle).await {
                    // Rescued: round the loop again and go on watching the same process.
                    Drained::Kept => (),
                    Drained::Stopped => break Ended::Stopped,
                    Drained::Restart => break Ended::Restart,
                    Drained::Shutdown => break Ended::Shutdown,
                },
            }
        };
        match ended {
            Ended::Shutdown => {
                info!(service = %spec.name, "stopping on-demand runner");
                stop_child(&mut child, pid).await;
                return;
            }
            Ended::Restart => wanted_now = true,
            Ended::Stopped => {}
        }
    }
}

/// What ended a running on-demand process.
enum Awake {
    /// The hive is shutting down.
    Shutdown,
    /// The process exited on its own.
    Exited,
    /// Nobody has asked for it for its idle window.
    Idle,
}

/// How an idle stop finished.
enum Drained {
    /// A request arrived in time and the process is still serving — it goes on running.
    Kept,
    /// The process is gone; wait for the next request.
    Stopped,
    /// The process is gone, and somebody is already waiting for it — start it again now.
    Restart,
    /// The hive is shutting down.
    Shutdown,
}

/// Why the on-demand loop gave up its process, once the run/drain cycle is over.
enum Ended {
    Shutdown,
    Stopped,
    Restart,
}

/// Watch a running on-demand service until it exits, the hive stops, or its idle window passes
/// without a request.
///
/// The tick does double duty: while the service is still coming up it probes the port, so the phase
/// flips to [`Phase::Running`] — and the holding page turns into the real service — as soon as
/// something is listening.
async fn run_until_idle(
    spec: &RunnerSpec,
    child: &mut Child,
    rx: &mut watch::Receiver<bool>,
    handle: &mut Handle,
) -> Awake {
    let mut ticker = tokio::time::interval(DEMAND_TICK);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut answering = false;
    loop {
        tokio::select! {
            status = child.wait() => {
                match status {
                    Ok(status) => info!(service = %spec.name, code = ?status.code(),
                                        "on-demand runner exited"),
                    Err(e) => warn!(service = %spec.name, error = %e,
                                    "waiting on on-demand runner failed"),
                }
                return Awake::Exited;
            }
            _ = rx.changed() => return Awake::Shutdown,
            _ = ticker.tick() => {
                if !answering && listening(spec.http_port).await {
                    answering = true;
                    handle.set_phase(Phase::Running);
                }
                // Reading the idle time also marks every request so far as accounted for, which is
                // what lets the stop below tell a new request from an old one.
                if handle.idle_for() >= spec.idle_stop {
                    return Awake::Idle;
                }
            }
        }
    }
}

/// Stop an idle on-demand service: `SIGTERM` first, so the process gets to finish what it is doing,
/// and `SIGKILL` only once the grace window has passed.
///
/// The stop is **interruptible**. A request arriving in that window cancels the escalation and keeps
/// the service, provided the process is still alive and still listening; a process that has already
/// begun going away cannot serve it, so it is let go and started again ([`Drained::Restart`]) while
/// the visitor reads the holding page. There is a race left inside that check — the process may exit
/// between the two questions — and it resolves the safe way: a service that stops answering between
/// them is treated as gone and restarted, which costs a cold start and never a wrong answer.
async fn drain(
    spec: &RunnerSpec,
    child: &mut Child,
    pid: Option<u32>,
    rx: &mut watch::Receiver<bool>,
    handle: &mut Handle,
) -> Drained {
    info!(service = %spec.name, pid = ?pid, idle_stop = ?spec.idle_stop,
          "no request for the idle window; stopping the on-demand runner");
    handle.set_phase(Phase::Draining);
    request_stop(child, pid);

    // The select yields *which* of the four happened and nothing else, so the work each calls for
    // happens below with the child and the handle free again — the futures above hold them for as
    // long as the select expression itself lasts.
    let woke = tokio::select! {
        _ = child.wait() => Woke::Exited,
        _ = rx.changed() => Woke::Shutdown,
        () = handle.wait_for_activity() => Woke::Request,
        () = tokio::time::sleep(spec.stop_grace) => Woke::GraceOver,
    };
    match woke {
        Woke::Exited => {
            info!(service = %spec.name, "on-demand runner stopped");
            Drained::Stopped
        }
        Woke::Shutdown => Drained::Shutdown,
        Woke::Request => {
            if still_serving(child, spec.http_port).await {
                info!(service = %spec.name,
                      "a request arrived while stopping; keeping the service running");
                handle.set_phase(Phase::Running);
                return Drained::Kept;
            }
            info!(service = %spec.name,
                  "a request arrived after the process began exiting; starting it again");
            stop_child(child, pid).await;
            Drained::Restart
        }
        Woke::GraceOver => {
            warn!(service = %spec.name, pid = ?pid, grace = ?spec.stop_grace,
                  "on-demand runner did not exit on SIGTERM; sending SIGKILL");
            force_stop(child, pid);
            let _ = child.wait().await;
            Drained::Stopped
        }
    }
}

/// What interrupted an idle stop's grace window.
enum Woke {
    Exited,
    Shutdown,
    Request,
    GraceOver,
}

/// Whether a service that is being stopped can still answer: its process has not exited *and*
/// something is still listening on its port. A service with no declared port is judged on the
/// process alone — there is nothing else to ask.
async fn still_serving(child: &mut Child, port: Option<u16>) -> bool {
    if !matches!(child.try_wait(), Ok(None)) {
        return false;
    }
    listening(port).await
}

/// Whether something answers on this loopback port. A service that declares no port is taken to be
/// answering: the supervisor has no way to check, and refusing to believe it would leave such a
/// service reading as "starting" for its whole life.
async fn listening(port: Option<u16>) -> bool {
    let Some(port) = port else {
        return true;
    };
    tokio::net::TcpStream::connect((std::net::Ipv4Addr::LOCALHOST, port))
        .await
        .is_ok()
}

/// Ask a runner's process group to finish, without waiting for it — the first half of an idle stop.
///
/// Unix sends `SIGTERM`, which is the whole point of the grace window: the process is given the
/// chance to close what it has open. Windows has no graceful signal, so the drain there is the
/// termination the escalation would have reached anyway, and the grace window simply observes it.
fn request_stop(child: &mut Child, pid: Option<u32>) {
    #[cfg(unix)]
    if let Some(pid) = pid {
        signal_group(pid, "TERM");
        return;
    }
    #[cfg(windows)]
    if let Some(pid) = pid {
        let _ = std::process::Command::new("taskkill")
            .args(["/T", "/F", "/PID", &pid.to_string()])
            .status();
        return;
    }
    let _ = child.start_kill();
}

/// Escalate an idle stop the grace window did not finish: `SIGKILL` the group (Unix) or terminate
/// the tree (Windows).
fn force_stop(child: &mut Child, pid: Option<u32>) {
    #[cfg(unix)]
    if let Some(pid) = pid {
        signal_group(pid, "KILL");
        return;
    }
    #[cfg(windows)]
    if let Some(pid) = pid {
        let _ = std::process::Command::new("taskkill")
            .args(["/T", "/F", "/PID", &pid.to_string()])
            .status();
        return;
    }
    let _ = child.start_kill();
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
    #[cfg(windows)]
    {
        // Detach from the launcher's console group (CREATE_NEW_PROCESS_GROUP) so a Ctrl-C to
        // the parent doesn't reach the runner; shutdown kills the tree with `taskkill /T`.
        cmd.creation_flags(0x0000_0200);
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

/// Stop a running child at shutdown. Unix: `SIGTERM` its process group, wait a grace period, then
/// `SIGKILL` if still up. Windows: `taskkill /T /F` the tree by pid. Falls back to a direct kill
/// when there is no pid.
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
    #[cfg(windows)]
    if let Some(pid) = pid {
        // No graceful signal on Windows: terminate the whole tree (`/T` reaches grandchildren).
        let _ = std::process::Command::new("taskkill")
            .args(["/T", "/F", "/PID", &pid.to_string()])
            .status();
        let _ = child.wait().await;
        return;
    }
    let _ = child.start_kill();
    let _ = child.wait().await;
}

/// Send a signal to a whole process group (the runner leads its group, so a negative pid targets it).
///
/// The `--` is load-bearing. procps-ng `kill` reads a bare `-1352905` as only its *first digit*, so
/// on Linux — where pids reach seven figures — every group kill of a pid beginning with `1` became
/// `kill(-1, ...)`: a broadcast to every process the user owns. Reloading one service therefore
/// SIGTERMed the whole `systemd --user` session, taking the control panel, DNS and the manager
/// itself down with it. `--` makes the pid a pid again; verified against procps-ng 4.0.4.
#[cfg(unix)]
fn signal_group(pid: u32, signal: &str) {
    let _ = std::process::Command::new("/bin/kill")
        .args([format!("-{signal}"), "--".into(), format!("-{pid}")])
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
    use crate::config::StartPolicy;

    /// A spec that runs a command long enough to still be alive when the test inspects it.
    fn spec(name: &str, run: &str) -> RunnerSpec {
        RunnerSpec {
            name: name.to_string(),
            run: run.to_string(),
            working_dir: std::env::temp_dir(),
            env: Vec::new(),
            restart: RestartPolicy::Never,
            start: StartPolicy::Always,
            idle_stop: crate::config::DEFAULT_IDLE_STOP,
            stop_grace: crate::config::DEFAULT_STOP_GRACE,
            http_port: None,
        }
    }

    /// The same, on demand, with the two windows wound down to test speed.
    fn on_demand(name: &str, run: &str, idle_stop: Duration, stop_grace: Duration) -> RunnerSpec {
        RunnerSpec {
            start: StartPolicy::OnDemand,
            idle_stop,
            stop_grace,
            ..spec(name, run)
        }
    }

    /// A supervisor over a registry the test can touch, the way the front door does.
    fn supervisor(specs: Vec<RunnerSpec>) -> (Supervisor, Arc<Demand>) {
        let demand = Arc::new(Demand::default());
        (Supervisor::start(specs, Arc::clone(&demand)), demand)
    }

    /// Wait for a service to reach `phase`, up to a second — the tests drive real processes, so
    /// every assertion about them is an assertion about something that has to *happen*.
    async fn wait_for_phase(demand: &Demand, name: &str, phase: Phase) {
        let deadline = Instant::now() + Duration::from_secs(1);
        while Instant::now() < deadline {
            if demand.phase(name) == Some(phase) {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!(
            "{name} never reached {phase:?} (it is {:?})",
            demand.phase(name)
        );
    }

    /// The core hot-reload contract: an added service starts, a removed one stops, and a service
    /// whose spec is unchanged is left strictly alone (same task, never bounced).
    #[tokio::test]
    async fn reconcile_adds_and_removes_without_touching_unchanged_services() {
        let (mut sup, _) = supervisor(vec![spec("keep", "sleep 30"), spec("drop", "sleep 30")]);
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
        let (mut sup, _) = supervisor(vec![spec("api", "sleep 30")]);
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

    /// The whole point of the policy: an on-demand service costs nothing until somebody visits it,
    /// and a visit is what starts it.
    #[tokio::test]
    async fn an_on_demand_service_is_not_launched_until_a_request_arrives() {
        let dir = scratch("start");
        let marker = dir.join("started");
        let mut spec = on_demand(
            "web",
            "printf x >> started; sleep 30",
            Duration::from_secs(60),
            Duration::from_secs(5),
        );
        spec.working_dir = dir.clone();

        let (sup, demand) = supervisor(vec![spec]);
        // Long enough that a service which *was* going to be launched at boot would have been.
        tokio::time::sleep(Duration::from_millis(150)).await;
        assert!(
            !marker.exists(),
            "an on-demand service must not be started at boot"
        );
        assert_eq!(demand.phase("web"), Some(Phase::IdleStopped));

        assert!(demand.touch("web"), "the front door can wake it");
        wait_for_phase(&demand, "web", Phase::Running).await;
        assert!(marker.exists(), "the request started the process");

        sup.shutdown().await;
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The idle stop, and the shape of it the operator asked for: `SIGTERM` first, and the service
    /// left idle-stopped rather than gone — a later request brings it back.
    #[tokio::test]
    async fn an_idle_service_is_stopped_with_a_signal_and_starts_again_on_the_next_request() {
        let dir = scratch("idle");
        let spec = RunnerSpec {
            working_dir: dir.clone(),
            ..on_demand(
                "web",
                "printf x >> starts; sleep 30",
                Duration::from_millis(150),
                Duration::from_secs(5),
            )
        };
        let (sup, demand) = supervisor(vec![spec]);

        demand.touch("web");
        wait_for_phase(&demand, "web", Phase::Running).await;
        // Nothing asks for it again, so its idle window runs out.
        wait_for_phase(&demand, "web", Phase::IdleStopped).await;

        demand.touch("web");
        wait_for_phase(&demand, "web", Phase::Running).await;
        let starts = std::fs::read_to_string(dir.join("starts")).expect("the start log");
        assert_eq!(starts, "xx", "a second visit starts a second process");

        sup.shutdown().await;
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The race the interruptible stop exists for: a request landing between the `SIGTERM` and the
    /// exit keeps the service, rather than being answered by a process that is on its way out.
    ///
    /// The runner ignores `SIGTERM` (`trap ""`), which is how the test can be *sure* it is still
    /// alive inside the grace window — a real service would be finishing a request there.
    #[tokio::test]
    async fn a_request_during_the_grace_window_cancels_the_stop() {
        let dir = scratch("rescue");
        let spec = RunnerSpec {
            working_dir: dir.clone(),
            ..on_demand(
                "web",
                "trap '' TERM; printf x >> starts; while :; do sleep 0.05; done",
                Duration::from_millis(150),
                // Long enough that the test does the interrupting, not the clock.
                Duration::from_secs(10),
            )
        };
        let (sup, demand) = supervisor(vec![spec]);

        demand.touch("web");
        wait_for_phase(&demand, "web", Phase::Running).await;
        wait_for_phase(&demand, "web", Phase::Draining).await;

        // A visitor arrives while the service is draining.
        assert!(demand.touch("web"));
        wait_for_phase(&demand, "web", Phase::Running).await;

        // And it is the *same* process: the escalation was cancelled, not survived by a new one.
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(
            std::fs::read_to_string(dir.join("starts")).expect("the start log"),
            "x",
            "the rescued service must not have been restarted"
        );

        sup.shutdown().await;
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// An `always` service is untouched by any of this: it starts at boot, is never registered as
    /// on-demand, and is never idle-stopped.
    #[tokio::test]
    async fn an_always_service_starts_at_boot_and_is_never_idle_stopped() {
        let dir = scratch("always");
        let spec = RunnerSpec {
            working_dir: dir.clone(),
            ..spec("web", "printf x >> started; sleep 30")
        };
        let (sup, demand) = supervisor(vec![spec]);

        tokio::time::sleep(Duration::from_millis(200)).await;
        assert!(dir.join("started").exists(), "started with the hive");
        assert_eq!(demand.phase("web"), None, "not an on-demand service");
        assert!(
            !demand.touch("web"),
            "and the front door has nothing to wake"
        );

        sup.shutdown().await;
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A scratch directory for a test that has to watch a process do something on disk.
    fn scratch(label: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "adi-hive-demand-{label}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("a scratch dir");
        dir
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
