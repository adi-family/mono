//! Keeps background triggers alive.
//!
//! A [background](crate::KIND_BACKGROUND) trigger is a long-lived independent process, and
//! `enabled` is its power switch: while it is on, the supervisor keeps the code block running,
//! relaunching it with exponential backoff whenever it exits; turning it off stops the process.
//! Editing a running trigger's code restarts it, and nothing else — the untouched ones keep
//! running.
//!
//! The desired state is simply *what the store says*: the supervisor re-reads it every
//! [`TICK`] (and immediately whenever [`Supervisor::poke`] is called after a save), so any
//! writer — the API, the CLI, a hand-edited TOML — steers it without an IPC path. Each live
//! process publishes its [`RunState`](crate::RunState) so other processes can report status.

use std::collections::BTreeMap;
use std::io::Write as _;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::process::{Child, Command};
use tokio::sync::{Notify, watch};
use tokio::task::JoinHandle;
use tracing::{debug, info, warn};

use adi_config::now_unix;

use crate::Triggers;
use crate::fire;
use crate::run::RunState;
use crate::trigger::Trigger;

/// How often the desired state is re-read from the store, and how often a live process
/// refreshes its heartbeat. Well under the run state's staleness window, so a healthy trigger
/// never blinks to "stopped".
const TICK: Duration = Duration::from_secs(3);

/// First delay before relaunching a code block that exited; doubles on each successive exit.
const INITIAL_BACKOFF: Duration = Duration::from_millis(500);

/// Ceiling for the relaunch backoff — a permanently broken code block retries this often.
const MAX_BACKOFF: Duration = Duration::from_secs(30);

/// A process that ran at least this long before exiting is treated as healthy, so its backoff
/// (and restart count) resets — a daily job that runs for an hour isn't a crash loop.
const STABLE_RUNTIME: Duration = Duration::from_secs(30);

/// How long to wait after `SIGTERM` before escalating to `SIGKILL` when stopping.
const TERM_GRACE: Duration = Duration::from_secs(5);

/// What the supervisor watches for change. A trigger whose spec is byte-identical across a
/// reconcile is left strictly alone; any difference restarts it, which is exactly the behavior
/// a human editing a running bot's code expects.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Spec {
    name: String,
    runtime: String,
    code: String,
    extra: BTreeMap<String, String>,
    /// Bumped by [`Supervisor::request_restart`] to force an otherwise-unchanged trigger to
    /// restart.
    epoch: u64,
}

impl Spec {
    /// The spec for a trigger the supervisor should be running, at the given restart epoch.
    fn of(trigger: &Trigger, epoch: u64) -> Self {
        Self {
            name: trigger.name.clone(),
            runtime: trigger.manifest.runtime.clone(),
            code: trigger.manifest.code.clone(),
            extra: trigger.manifest.extra.clone(),
            epoch,
        }
    }
}

/// One supervised trigger: the spec it was started from, its private stop signal, and the task
/// driving it. Each gets its **own** stop signal rather than sharing one — that is what lets a
/// reconcile stop a single trigger while the others keep running untouched.
#[derive(Debug)]
struct Running {
    spec: Spec,
    shutdown: watch::Sender<bool>,
    task: JoinHandle<()>,
}

/// Supervises the store's enabled background triggers. Cheap to clone behind an `Arc`; hold one
/// for as long as the triggers should stay up, and [`Supervisor::shutdown`] it to stop them all.
#[derive(Debug)]
pub struct Supervisor {
    store: Triggers,
    /// Wakes the reconcile loop early, so a save takes effect now rather than at the next tick.
    wake: Notify,
    /// Per-trigger restart epochs, bumped to force a restart.
    epochs: std::sync::Mutex<BTreeMap<String, u64>>,
    /// Stops the reconcile loop itself.
    shutdown: watch::Sender<bool>,
    /// Flipped once the loop has torn every supervised process down, so [`Supervisor::stop`]
    /// can wait for that rather than racing the host's exit.
    done: watch::Sender<bool>,
}

impl Supervisor {
    /// Start supervising `store`'s background triggers: spawns the reconcile loop and returns
    /// immediately. Triggers come up over the next moment, not synchronously.
    ///
    /// # Panics
    /// If called outside a tokio runtime.
    #[must_use]
    pub fn start(store: Triggers) -> Arc<Self> {
        let (supervisor, rx) = Self::new(store);
        // Anything still running from a previous supervisor is an orphan: its processes live in
        // their own process groups, so a hard-killed host leaves them behind. Clear them before
        // starting, or every restart of the host leaks another copy of every background trigger.
        supervisor.reap_orphans();
        tokio::spawn(Arc::clone(&supervisor).reconcile_loop(rx));
        supervisor
    }

    /// A supervisor that supervises nothing: every [`poke`](Self::poke) and
    /// [`request_restart`](Self::request_restart) is accepted and quietly goes nowhere, because
    /// no reconcile loop is listening. For hosts that only need to satisfy the interface —
    /// tests, and any tool that mutates trigger definitions without owning their processes.
    #[must_use]
    pub fn inert(store: Triggers) -> Arc<Self> {
        Self::new(store).0
    }

    /// The shared state, plus the shutdown receiver the reconcile loop would watch. Creating it
    /// touches no runtime, which is what makes [`inert`](Self::inert) possible.
    fn new(store: Triggers) -> (Arc<Self>, watch::Receiver<bool>) {
        let (shutdown, rx) = watch::channel(false);
        let (done, _) = watch::channel(false);
        (
            Arc::new(Self {
                store,
                wake: Notify::new(),
                epochs: std::sync::Mutex::new(BTreeMap::new()),
                shutdown,
                done,
            }),
            rx,
        )
    }

    /// Re-read the store now instead of waiting for the next tick. Call it after any write that
    /// changes what should be running — a save, an enable, a delete.
    pub fn poke(&self) {
        self.wake.notify_one();
    }

    /// Restart a trigger even though its definition didn't change (the Restart action): bump its
    /// epoch so the next reconcile sees a different spec, and wake that reconcile.
    pub fn request_restart(&self, name: &str) {
        if let Ok(mut epochs) = self.epochs.lock() {
            *epochs.entry(name.to_string()).or_default() += 1;
        }
        self.poke();
    }

    /// Signal the reconcile loop to stop, without waiting. Prefer [`Supervisor::stop`] at
    /// shutdown — returning before the children are down is what leaks them.
    pub fn shutdown(&self) {
        let _ = self.shutdown.send(true);
    }

    /// Stop every supervised trigger and wait for them to actually exit.
    ///
    /// A supervised process runs in its own process group so the supervisor can signal its whole
    /// tree — which also means it does *not* die with its host. Exiting without waiting here
    /// orphans one process per background trigger, so a host that wants a clean shutdown must
    /// await this. Gives up after `grace` so a wedged code block can't block the host's exit.
    pub async fn stop(&self, grace: Duration) {
        let mut done = self.done.subscribe();
        if self.shutdown.send(true).is_err() {
            return;
        }
        let _ = tokio::time::timeout(grace, done.changed()).await;
    }

    /// Kill anything a previous supervisor left running.
    ///
    /// Every state published at this moment belongs to a *previous* run — this supervisor has
    /// just been constructed and published nothing, and a store has one supervisor. Freshness is
    /// deliberately not consulted: the host is restarted in seconds, so a hard-killed supervisor
    /// leaves a heartbeat that still looks alive, and skipping those is exactly how a leak
    /// survives a restart. Safety comes from identity instead — a pid is only signalled while it
    /// is *still running the command the state recorded* — so a recycled pid is never hit.
    fn reap_orphans(&self) {
        for (name, state) in self.store.published_run_states() {
            if state.pid > 0
                && running_command(state.pid).as_deref() == Some(state.command.as_str())
            {
                info!(trigger = %name, pid = state.pid, "killing orphaned trigger process");
                signal_group(state.pid, "KILL");
            }
            self.store.clear_run_state(&name);
        }
    }

    /// The reconcile loop: bring the running set in line with the store, then wait for the next
    /// tick, a poke, or shutdown.
    async fn reconcile_loop(self: Arc<Self>, mut rx: watch::Receiver<bool>) {
        let mut running: BTreeMap<String, Running> = BTreeMap::new();
        loop {
            self.reconcile(&mut running);

            tokio::select! {
                () = tokio::time::sleep(TICK) => {}
                () = self.wake.notified() => debug!("triggers supervisor poked"),
                _ = rx.changed() => break,
            }
            if *rx.borrow_and_update() {
                break;
            }
        }

        info!(count = running.len(), "stopping supervised triggers");
        for r in running.values() {
            let _ = r.shutdown.send(true);
        }
        for (_, r) in running {
            let _ = r.task.await;
        }
        let _ = self.done.send(true);
    }

    /// Start what should be running and isn't, stop what shouldn't, and restart anything whose
    /// spec changed. A trigger whose spec is byte-identical is never bounced.
    fn reconcile(&self, running: &mut BTreeMap<String, Running>) {
        let mut wanted = match self.wanted() {
            Ok(wanted) => wanted,
            Err(e) => {
                warn!(error = %e, "couldn't read triggers; leaving the running set alone");
                return;
            }
        };

        // Drop anything no longer wanted, or wanted differently. A changed one is restarted by
        // being stopped here and re-started below, since it stays in `wanted`.
        let stale: Vec<String> = running
            .iter()
            .filter(|(name, r)| wanted.get(*name) != Some(&r.spec))
            .map(|(name, _)| name.clone())
            .collect();
        for name in stale {
            if let Some(r) = running.remove(&name) {
                info!(trigger = %name, "stopping (disabled, deleted, or changed)");
                // Signal and move on: awaiting here would block reconciles for the whole
                // SIGTERM grace period.
                let _ = r.shutdown.send(true);
            }
        }

        // Whatever isn't already running is new (or was just stopped for a respec).
        wanted.retain(|name, _| !running.contains_key(name));
        for (name, spec) in wanted {
            info!(trigger = %name, runtime = %spec.runtime, "starting background trigger");
            let (shutdown, rx) = watch::channel(false);
            let task = tokio::spawn(supervise(self.store.clone(), spec.clone(), rx));
            running.insert(
                name,
                Running {
                    spec,
                    shutdown,
                    task,
                },
            );
        }
    }

    /// The triggers that should be running right now: every enabled background trigger with a
    /// code block, at its current restart epoch.
    fn wanted(&self) -> crate::Result<BTreeMap<String, Spec>> {
        let epochs = self.epochs.lock().ok();
        Ok(self
            .store
            .list()?
            .into_iter()
            .filter(|t| {
                t.manifest.enabled
                    && t.manifest.is_background()
                    && !t.manifest.code.trim().is_empty()
            })
            .map(|t| {
                let epoch = epochs
                    .as_ref()
                    .and_then(|e| e.get(&t.name).copied())
                    .unwrap_or_default();
                (t.name.clone(), Spec::of(&t, epoch))
            })
            .collect())
    }
}

/// Supervise one trigger: launch, watch, relaunch with backoff — until the stop signal arrives.
async fn supervise(store: Triggers, spec: Spec, mut rx: watch::Receiver<bool>) {
    let mut backoff = INITIAL_BACKOFF;
    let mut restarts = 0u32;

    loop {
        if *rx.borrow_and_update() {
            break;
        }

        let started = Instant::now();
        let (mut child, command) = match spawn(&store, &spec).await {
            Ok(launched) => launched,
            Err(e) => {
                warn!(trigger = %spec.name, error = %e, "couldn't launch; will retry");
                if sleep_or_stop(backoff, &mut rx).await {
                    break;
                }
                backoff = next_backoff(backoff);
                restarts = restarts.saturating_add(1);
                continue;
            }
        };

        let pid = child.id().unwrap_or_default();
        info!(trigger = %spec.name, pid, "background trigger up");
        let mut state = RunState {
            pid,
            started_at: now_unix(),
            restarts,
            heartbeat_at: now_unix(),
            command,
        };
        store.publish_run_state(&spec.name, &state);

        // Watch the child, refreshing the published heartbeat, until it exits or we're stopped.
        let stopped = loop {
            tokio::select! {
                status = child.wait() => {
                    match status {
                        Ok(status) => info!(trigger = %spec.name, code = ?status.code(), "background trigger exited"),
                        Err(e) => warn!(trigger = %spec.name, error = %e, "waiting on the process failed"),
                    }
                    break false;
                }
                () = tokio::time::sleep(TICK) => {
                    state.heartbeat_at = now_unix();
                    store.publish_run_state(&spec.name, &state);
                }
                _ = rx.changed() => {
                    stop_child(&mut child, pid).await;
                    break true;
                }
            }
        };

        store.clear_run_state(&spec.name);
        if stopped {
            break;
        }

        // A process that stayed up is healthy however it ended, so it starts over with a fresh
        // backoff and restart count — only a *tight* loop should look like one.
        if started.elapsed() >= STABLE_RUNTIME {
            backoff = INITIAL_BACKOFF;
            restarts = 0;
        } else {
            restarts = restarts.saturating_add(1);
        }

        warn!(trigger = %spec.name, delay = ?backoff, restarts, "relaunching");
        if sleep_or_stop(backoff, &mut rx).await {
            break;
        }
        backoff = next_backoff(backoff);
    }

    store.clear_run_state(&spec.name);
    info!(trigger = %spec.name, "supervision ended");
}

/// Build and spawn the trigger's code block in its own process group, appending to its log — a
/// supervised trigger's log is a history across relaunches, not just the last run.
async fn spawn(store: &Triggers, spec: &Spec) -> crate::Result<(Child, String)> {
    let trigger = store
        .get(&spec.name)?
        .ok_or_else(|| crate::Error::NotFound(spec.name.clone()))?;
    let dir = store.dir();
    let launch = fire::launch(&dir, &trigger, None)?;

    let mut log = fire::open_log(&dir, &spec.name, true)?;
    let _ = writeln!(log, "\n── {} started at {} ──", spec.name, humanish_now());
    let errlog = log.try_clone()?;

    let mut cmd = Command::new(launch.program);
    cmd.args(&launch.args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::from(log))
        .stderr(std::process::Stdio::from(errlog))
        // Kill the direct child if this task is dropped; the stop path's group-kill covers
        // grandchildren.
        .kill_on_drop(true)
        // Become a process-group leader so the whole tree can be signalled.
        .process_group(0);
    for (key, value) in &launch.env {
        cmd.env(key, value);
    }
    if let Ok(home) = std::env::var("HOME") {
        cmd.current_dir(home);
    }

    let command = command_line(launch.program, &launch.args);
    let child = cmd
        .spawn()
        .map_err(|e| crate::Error::Launch(format!("couldn't spawn {}: {e}", launch.program)))?;
    Ok((child, command))
}

/// The command line as `ps` reports it, so a recorded state can be matched against a live
/// process before that process is signalled.
fn command_line(program: &str, args: &[String]) -> String {
    std::iter::once(program.to_string())
        .chain(args.iter().cloned())
        .collect::<Vec<_>>()
        .join(" ")
}

/// What `pid` is currently running, or `None` if no such process exists. Shelling out to `ps`
/// keeps this crate free of a libc dependency, matching how the rest of the platform signals
/// processes; it runs once per orphan at startup, never on a hot path.
fn running_command(pid: u32) -> Option<String> {
    let out = std::process::Command::new("ps")
        .args(["-o", "command=", "-p", &pid.to_string()])
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (!text.is_empty()).then_some(text)
}

/// Stop a supervised process: `SIGTERM` its group, wait a grace period, then `SIGKILL`.
async fn stop_child(child: &mut Child, pid: u32) {
    if pid > 0 {
        signal_group(pid, "TERM");
        if tokio::time::timeout(TERM_GRACE, child.wait())
            .await
            .is_err()
        {
            warn!(pid, "trigger did not exit on SIGTERM; sending SIGKILL");
            signal_group(pid, "KILL");
            let _ = child.wait().await;
        }
        return;
    }
    let _ = child.start_kill();
    let _ = child.wait().await;
}

/// Signal a code block and everything it spawned. The block is launched as its own process-group
/// leader, so a negative pid reaches the whole tree — but that only holds while it *is* the
/// leader (a shell can move itself), so the process itself is signalled too when the group form
/// finds nothing. Missing either one is how a `sleep` inside a killed loop keeps running.
fn signal_group(pid: u32, signal: &str) {
    let group = std::process::Command::new("kill")
        .arg(format!("-{signal}"))
        .arg(format!("-{pid}"))
        .status();
    if !matches!(group, Ok(status) if status.success()) {
        let _ = std::process::Command::new("kill")
            .arg(format!("-{signal}"))
            .arg(pid.to_string())
            .status();
    }
}

fn next_backoff(current: Duration) -> Duration {
    (current * 2).min(MAX_BACKOFF)
}

/// Sleep for `dur`, returning `true` if a stop arrived first.
async fn sleep_or_stop(dur: Duration, rx: &mut watch::Receiver<bool>) -> bool {
    tokio::select! {
        () = tokio::time::sleep(dur) => false,
        _ = rx.changed() => true,
    }
}

/// The current time as epoch seconds, for the log banner — the log is read by humans in the UI,
/// which renders timestamps itself, so this stays dependency-free.
fn humanish_now() -> u64 {
    now_unix()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::process::CommandExt as _;

    use crate::TriggerManifest;
    use crate::trigger::{KIND_BACKGROUND, KIND_WEBHOOK, RUNTIME_SH};

    fn scratch(tag: &str) -> Triggers {
        let root = std::env::temp_dir().join(format!(
            "adi-triggers-sup-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id(),
        ));
        let _ = std::fs::remove_dir_all(&root);
        Triggers::with_config(adi_config::Config::with_root(root))
    }

    fn save(store: &Triggers, name: &str, kind: &str, code: &str, enabled: bool) {
        store
            .save(
                name,
                TriggerManifest {
                    kind: kind.into(),
                    runtime: RUNTIME_SH.into(),
                    code: code.into(),
                    enabled,
                    ..TriggerManifest::default()
                },
            )
            .expect("save");
    }

    /// Poll until `pred` holds, so a test never races the supervisor's tick. The budget is
    /// generous because these tests spawn real processes: on a machine running the whole
    /// workspace's suites at once, a launch that normally takes milliseconds can be starved for
    /// seconds, and a tight budget turns that into a flake rather than a finding.
    async fn wait_until(mut pred: impl FnMut() -> bool) -> bool {
        for _ in 0..600 {
            if pred() {
                return true;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        false
    }

    /// The core contract: an enabled background trigger comes up and stays up, and disabling it
    /// takes it down.
    #[tokio::test]
    async fn an_enabled_background_trigger_runs_until_disabled() {
        let store = scratch("lifecycle");
        save(&store, "bot", KIND_BACKGROUND, "sleep 300", true);

        let sup = Supervisor::start(store.clone());
        assert!(
            wait_until(|| store.status("bot").is_some()).await,
            "the trigger should have come up"
        );

        save(&store, "bot", KIND_BACKGROUND, "sleep 300", false);
        sup.poke();
        assert!(
            wait_until(|| store.status("bot").is_none()).await,
            "disabling should stop it"
        );

        sup.shutdown();
    }

    /// A webhook trigger is launched by its endpoint, never held open — the supervisor must
    /// ignore it entirely, however its code block reads.
    #[tokio::test]
    async fn webhook_triggers_are_never_supervised() {
        let store = scratch("webhook");
        save(&store, "hook", KIND_WEBHOOK, "sleep 300", true);

        let sup = Supervisor::start(store.clone());
        tokio::time::sleep(Duration::from_millis(400)).await;
        assert!(
            store.status("hook").is_none(),
            "a webhook must not be held open"
        );

        sup.shutdown();
    }

    /// A code block that exits immediately is relaunched, and the restart count it publishes is
    /// what surfaces a crash loop in the UI.
    #[tokio::test]
    async fn a_dying_code_block_is_relaunched_and_counted() {
        let store = scratch("crashloop");
        save(&store, "flaky", KIND_BACKGROUND, "exit 1", true);

        let sup = Supervisor::start(store.clone());
        assert!(
            wait_until(|| store.status("flaky").is_some_and(|s| s.restarts >= 1)).await,
            "a crash loop should show restarts"
        );

        sup.shutdown();
    }

    /// Editing a running trigger's code restarts it; leaving it alone does not.
    #[tokio::test]
    async fn a_changed_spec_restarts_and_an_unchanged_one_does_not() {
        let store = scratch("respec");
        save(&store, "svc", KIND_BACKGROUND, "sleep 300", true);

        let sup = Supervisor::start(store.clone());
        assert!(
            wait_until(|| store.status("svc").is_some()).await,
            "should come up"
        );
        let first_pid = store.status("svc").expect("running").pid;

        // A reconcile with no change must leave the process strictly alone.
        sup.poke();
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert_eq!(
            store.status("svc").map(|s| s.pid),
            Some(first_pid),
            "an unchanged trigger must not be bounced"
        );

        save(&store, "svc", KIND_BACKGROUND, "sleep 301", true);
        sup.poke();
        assert!(
            wait_until(|| store.status("svc").is_some_and(|s| s.pid != first_pid)).await,
            "an edited code block should restart the process"
        );

        sup.shutdown();
    }

    /// The explicit Restart action: same definition, new process.
    #[tokio::test]
    async fn request_restart_replaces_the_process() {
        let store = scratch("restart");
        save(&store, "svc", KIND_BACKGROUND, "sleep 300", true);

        let sup = Supervisor::start(store.clone());
        assert!(
            wait_until(|| store.status("svc").is_some()).await,
            "should come up"
        );
        let first_pid = store.status("svc").expect("running").pid;

        sup.request_restart("svc");
        assert!(
            wait_until(|| store.status("svc").is_some_and(|s| s.pid != first_pid)).await,
            "restart should produce a new process"
        );

        sup.shutdown();
    }

    /// The leak this exists to prevent: a supervised process lives in its own process group, so
    /// killing its host leaves it running. A fresh supervisor over the same store must find it
    /// through the run state the dead one published, and kill it.
    ///
    /// The orphan is spawned directly rather than through a first supervisor — a supervisor
    /// "killed" inside this process would go on ticking and refreshing the heartbeat, which is
    /// exactly what a real SIGKILL stops happening.
    #[tokio::test]
    async fn a_new_supervisor_reaps_processes_the_last_one_orphaned() {
        let store = scratch("orphan");
        // Disabled, so the reaping is what the test observes rather than a fresh launch.
        save(&store, "leaky", KIND_BACKGROUND, "sleep 300", false);

        let mut orphan = std::process::Command::new("sleep")
            .arg("300")
            .process_group(0)
            .spawn()
            .expect("spawn orphan");
        let pid = orphan.id();
        store.publish_run_state(
            "leaky",
            &RunState {
                pid,
                started_at: now_unix() - 600,
                restarts: 0,
                // Quiet: whoever published this is gone.
                heartbeat_at: now_unix() - 600,
                command: running_command(pid).expect("the orphan is running"),
            },
        );

        let sup = Supervisor::start(store.clone());
        // Its exit status, not its absence from `ps`: a killed child this test parented stays a
        // zombie until waited on. (A real orphan is parented by launchd, which reaps it.)
        assert!(
            wait_until(|| orphan.try_wait().ok().flatten().is_some()).await,
            "the orphaned process (pid {pid}) should have been killed"
        );
        assert!(
            store.status("leaky").is_none(),
            "its stale state should be cleared"
        );

        sup.stop(Duration::from_secs(5)).await;
    }

    /// The restart is fast — launchd brings the host back in seconds — so the dead supervisor's
    /// last heartbeat is still *fresh* when the new one starts. Treating a fresh heartbeat as
    /// "someone else is watching this" is what let a leaked process survive every restart.
    #[tokio::test]
    async fn an_orphan_is_reaped_even_when_its_heartbeat_still_looks_alive() {
        let store = scratch("fast-restart");
        save(&store, "leaky", KIND_BACKGROUND, "sleep 300", false);

        let mut orphan = std::process::Command::new("sleep")
            .arg("300")
            .process_group(0)
            .spawn()
            .expect("spawn orphan");
        let pid = orphan.id();
        store.publish_run_state(
            "leaky",
            &RunState {
                pid,
                started_at: now_unix(),
                restarts: 0,
                // Beating a moment ago — the supervisor that wrote this died right after.
                heartbeat_at: now_unix(),
                command: running_command(pid).expect("the orphan is running"),
            },
        );

        let sup = Supervisor::start(store.clone());
        assert!(
            wait_until(|| orphan.try_wait().ok().flatten().is_some()).await,
            "a fresh heartbeat must not shield an orphan from the reaper"
        );

        sup.stop(Duration::from_secs(5)).await;
    }

    /// The safety rail on reaping: a pid the OS has since handed to something else must never be
    /// signalled, so identity is confirmed against the recorded command line first.
    #[tokio::test]
    async fn a_recycled_pid_is_never_killed() {
        let store = scratch("recycled");
        save(&store, "ghost", KIND_BACKGROUND, "sleep 300", false);

        // A bystander process, and a stale state claiming its pid ran something else entirely.
        let mut bystander = tokio::process::Command::new("sleep")
            .arg("30")
            .kill_on_drop(true)
            .spawn()
            .expect("spawn bystander");
        let pid = bystander.id().expect("pid");
        store.publish_run_state(
            "ghost",
            &RunState {
                pid,
                started_at: now_unix() - 600,
                restarts: 0,
                heartbeat_at: now_unix() - 600,
                command: "bun run /some/other/thing.ts".into(),
            },
        );

        let sup = Supervisor::start(store.clone());
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert!(
            running_command(pid).is_some(),
            "a recycled pid must survive the reaper"
        );
        assert!(
            store.status("ghost").is_none(),
            "the stale state is still cleared"
        );

        sup.stop(Duration::from_secs(5)).await;
        let _ = bystander.kill().await;
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
