//! On-demand services: the one piece of state the front door and the supervisor share.
//!
//! A `start: on-demand` service is not launched at boot. It exists as a *route* and nothing else
//! until a request arrives for its host; the front door then [`touch`](Demand::touch)es it, which
//! both starts it (if it is down) and stamps the activity that keeps it up. When no request has
//! arrived for the service's idle window, the supervisor stops it again — see [`crate::runner`].
//!
//! The two halves are deliberately not wired to each other. The proxy knows only "a request landed
//! on this service name"; the supervisor knows only "this service was wanted at this instant". A
//! service the supervisor never registered — the whole config of a route-only front door, which
//! launches nothing — is simply unknown here, so [`Demand::touch`] answers `false` and the front
//! door behaves exactly as it did before on-demand services existed.
//!
//! **One process.** The hive that routes a request is the hive that starts the service, because
//! this state lives in memory. Where the front door and the supervisor are two processes (a root
//! `:80` daemon routing what a per-user hive runs), the front door routes an on-demand service but
//! cannot wake it — it has no runner for it — and the request is answered by the ordinary
//! upstream-down page. The phases are written to a file so the *control panel* can still report
//! them; that file is a report, never a channel.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Instant;

use serde::Serialize;
use tokio::sync::watch;
use tracing::warn;

/// What an on-demand service is doing right now.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Phase {
    /// Not running, and stopped by *this* mechanism rather than by anybody's decision: nothing has
    /// asked for it since its idle window ran out (or since the hive started).
    IdleStopped,
    /// A request arrived and the process was spawned; its port is not answering yet. This is the
    /// state the holding page is shown for.
    Starting,
    /// Up and serving.
    Running,
    /// Idle: the `SIGTERM` has been sent and the grace window is running. Still able to answer, and
    /// still rescuable — a request arriving now cancels the escalation. Reported as `running`,
    /// because that is what it still is from the outside.
    Draining,
}

impl Phase {
    /// How the phase is named to anything outside this process (the state file, and through it the
    /// control panel). `Draining` is deliberately not one of the names: a draining service is still
    /// answering requests, and "stopping" in a table would invite a reader to act on a state that
    /// resolves itself either way within the grace window.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::IdleStopped => "idle-stopped",
            Self::Starting => "starting",
            Self::Running | Self::Draining => "running",
        }
    }
}

/// The registry of on-demand services: who is registered, when each was last wanted, and what each
/// is doing. Shared as an `Arc` between the proxy tasks and the supervisor's runner tasks.
#[derive(Debug, Default)]
pub struct Demand {
    services: Mutex<BTreeMap<String, Service>>,
    /// Where the phases are published for other processes, if anywhere.
    state_path: Option<PathBuf>,
}

#[derive(Debug)]
struct Service {
    /// Set to `now` by every request the front door routes here. A [`watch`] channel rather than a
    /// plain timestamp so the supervisor can *wait* on the next request as well as ask how long ago
    /// the last one was — and so a request landing while the supervisor task is busy is remembered
    /// instead of lost, which is precisely the race that decides whether a stopping service is
    /// rescued or killed.
    activity: watch::Sender<Instant>,
    phase: Phase,
}

/// One service's end of the registry, held by the supervisor task that owns its process.
#[derive(Debug)]
pub struct Handle {
    name: String,
    demand: Arc<Demand>,
    activity: watch::Receiver<Instant>,
}

impl Demand {
    /// A registry that publishes its phases to `state_path` (`None` to keep them in memory only).
    #[must_use]
    pub fn new(state_path: Option<PathBuf>) -> Self {
        Self {
            services: Mutex::new(BTreeMap::new()),
            state_path,
        }
    }

    /// Take charge of an on-demand service, from the supervisor task that will run it. Registering
    /// is what makes the front door able to wake it; until then it is an ordinary route.
    ///
    /// A re-registration (a service whose spec changed, so its task was replaced) keeps the
    /// existing activity stamp, so a reload does not read as a fresh hour of quiet.
    pub fn register(self: &Arc<Self>, name: &str) -> Handle {
        let activity = {
            let mut services = self.lock();
            let entry = services.entry(name.to_string()).or_insert_with(|| Service {
                activity: watch::channel(Instant::now()).0,
                phase: Phase::IdleStopped,
            });
            entry.activity.subscribe()
        };
        Handle {
            name: name.to_string(),
            demand: Arc::clone(self),
            activity,
        }
    }

    /// Stop tracking a service — its runner was removed from the config, or the supervisor is
    /// handing it to a fresh task with a different spec.
    pub fn forget(&self, name: &str) {
        let removed = self.lock().remove(name).is_some();
        if removed {
            self.publish();
        }
    }

    /// Say that a request for `name` just arrived: stamp the activity that keeps it alive, and wake
    /// the supervisor task, which starts it when it is down and cancels an idle stop in progress.
    ///
    /// Returns `false` for a service this hive does not supervise on demand — every `always`
    /// service, and every service at all in a route-only front door — which is the front door's cue
    /// to route the request exactly as it always has.
    pub fn touch(&self, name: &str) -> bool {
        let services = self.lock();
        let Some(service) = services.get(name) else {
            return false;
        };
        service.activity.send_replace(Instant::now());
        true
    }

    /// What `name` is doing, or `None` if it is not an on-demand service here.
    #[must_use]
    pub fn phase(&self, name: &str) -> Option<Phase> {
        self.lock().get(name).map(|s| s.phase)
    }

    /// The phase of every registered service — what the published state file carries, and what the
    /// tests read.
    #[must_use]
    pub fn phases(&self) -> BTreeMap<String, Phase> {
        self.lock()
            .iter()
            .map(|(name, svc)| (name.clone(), svc.phase))
            .collect()
    }

    /// Record a phase change and publish it. A no-op when the phase is unchanged, so the state file
    /// is written on transitions (a handful over a service's life) and not on ticks.
    fn set_phase(&self, name: &str, phase: Phase) {
        {
            let mut services = self.lock();
            let Some(service) = services.get_mut(name) else {
                return;
            };
            if service.phase == phase {
                return;
            }
            service.phase = phase;
        }
        self.publish();
    }

    /// Write the phases where another process can read them — the control panel's `GET /api/hive`,
    /// which otherwise can only tell "listening" from "not listening" and would report a service
    /// that is *starting* as stopped.
    ///
    /// Nothing is written by a hive with no on-demand services registered. That is what keeps a
    /// route-only front door from clobbering the file its own supervisor writes: both read the same
    /// config directory, and only one of them runs anything.
    fn publish(&self) {
        let Some(path) = self.state_path.as_ref() else {
            return;
        };
        let phases = self.phases();
        if phases.is_empty() {
            return;
        }
        let named: BTreeMap<&str, &str> = phases
            .iter()
            .map(|(name, phase)| (name.as_str(), phase.as_str()))
            .collect();
        let written = serde_json::to_vec_pretty(&named)
            .map_err(std::io::Error::other)
            .and_then(|json| adi_osext::write_status_file(path, &json));
        if let Err(e) = written {
            warn!(error = %e, path = %path.display(), "could not write the on-demand state file");
        }
    }

    /// Remove the published state file, at shutdown — the phases in it are about a process that is
    /// no longer running.
    pub fn unpublish(&self) {
        if let Some(path) = self.state_path.as_ref() {
            let _ = std::fs::remove_file(path);
        }
    }

    /// The registry, recovering rather than propagating a poisoned lock: a panic elsewhere must not
    /// take the front door's routing down with it.
    fn lock(&self) -> std::sync::MutexGuard<'_, BTreeMap<String, Service>> {
        self.services.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl Handle {
    /// This service's key, as the config names it.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// How long it has been since the last request for this service.
    ///
    /// Reading it also marks what it read as seen, which is what makes
    /// [`wait_for_activity`](Self::wait_for_activity) mean "a request I have not accounted for yet"
    /// rather than "any request ever". The supervisor reads this on every tick of a running
    /// service, so by the time it decides the service is idle, only a genuinely *new* request can
    /// interrupt the stop that follows.
    pub fn idle_for(&mut self) -> std::time::Duration {
        self.activity.borrow_and_update().elapsed()
    }

    /// Wait for the next request for this service. Edge-triggered on the activity channel, and
    /// marked seen only when it fires, so a request that arrives while the caller is doing
    /// something else is delivered the moment it comes back — never dropped.
    ///
    /// Never completes if the registry has been dropped, which cannot happen while the supervisor
    /// that owns this handle is alive.
    pub async fn wait_for_activity(&mut self) {
        if self.activity.changed().await.is_err() {
            std::future::pending::<()>().await;
        }
    }

    /// Treat now as a request, without one having arrived: what a start does, so a service that
    /// takes a minute to boot is not measured as a minute of silence.
    pub fn touch(&mut self) {
        self.demand.touch(&self.name);
        // Our own receiver must not read that stamp as a request to answer — it is one we made.
        self.activity.borrow_and_update();
    }

    /// Say what this service is doing now.
    pub fn set_phase(&self, phase: Phase) {
        self.demand.set_phase(&self.name, phase);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The contract the front door depends on: a registered service can be woken, and one this
    /// hive does not supervise says so rather than pretending.
    #[test]
    fn only_a_registered_service_can_be_touched() {
        let demand = Arc::new(Demand::default());
        assert!(!demand.touch("web"), "nothing is registered yet");
        let handle = demand.register("web");
        assert!(demand.touch("web"), "the supervisor took charge of it");
        assert_eq!(handle.name(), "web");
        assert_eq!(demand.phase("web"), Some(Phase::IdleStopped));
        assert_eq!(demand.phase("other"), None);

        demand.forget("web");
        assert!(!demand.touch("web"), "a removed service is unknown again");
    }

    /// A request that lands while the supervisor task is busy elsewhere must still be seen when it
    /// next looks — that is the race between a `SIGTERM` and the process exiting.
    #[tokio::test]
    async fn a_request_is_remembered_until_the_supervisor_looks_at_it() {
        let demand = Arc::new(Demand::default());
        let mut handle = demand.register("web");

        // Nobody is waiting at this instant.
        assert!(demand.touch("web"));
        // And the wait completes immediately anyway, on the stamp it missed.
        tokio::time::timeout(
            std::time::Duration::from_millis(100),
            handle.wait_for_activity(),
        )
        .await
        .expect("the earlier request is still there to be found");

        // Having been seen, it does not fire twice.
        assert!(
            tokio::time::timeout(
                std::time::Duration::from_millis(50),
                handle.wait_for_activity()
            )
            .await
            .is_err(),
            "one request is one wake-up"
        );
    }

    /// A service's own start is not a visit — but it does reset the clock, or a slow boot would be
    /// measured as silence and stopped on the first idle check.
    #[tokio::test]
    async fn a_self_touch_resets_the_idle_clock_without_waking_the_task() {
        let demand = Arc::new(Demand::default());
        let mut handle = demand.register("web");
        handle.touch();
        assert_eq!(demand.phase("web"), Some(Phase::IdleStopped));
        assert!(handle.idle_for() < std::time::Duration::from_secs(1));
        assert!(
            tokio::time::timeout(
                std::time::Duration::from_millis(50),
                handle.wait_for_activity()
            )
            .await
            .is_err(),
            "a start must not look like a request to serve",
        );
    }

    /// Re-registering (a reload that replaced the task) keeps the activity stamp — a service that
    /// was busy a second ago must not look like an hour of quiet to its new task.
    #[test]
    fn re_registering_keeps_the_activity_stamp() {
        let demand = Arc::new(Demand::default());
        let mut first = demand.register("web");
        demand.touch("web");
        std::thread::sleep(std::time::Duration::from_millis(20));
        let mut second = demand.register("web");
        assert!(second.idle_for() >= std::time::Duration::from_millis(20));
        assert!(
            second.idle_for().saturating_sub(first.idle_for())
                < std::time::Duration::from_millis(20),
            "both handles read the same clock"
        );
    }

    /// The phases other processes read, named for the four states the control panel shows.
    #[test]
    fn a_draining_service_is_still_reported_as_running() {
        assert_eq!(Phase::IdleStopped.as_str(), "idle-stopped");
        assert_eq!(Phase::Starting.as_str(), "starting");
        assert_eq!(Phase::Running.as_str(), "running");
        assert_eq!(Phase::Draining.as_str(), "running");
    }

    /// The state file is what the control panel reads to tell "starting" from "stopped"; a hive
    /// supervising nothing on demand must leave it alone, since a second hive over the same config
    /// dir may be the one writing it.
    #[test]
    fn the_state_file_is_written_only_by_a_hive_that_supervises_something() {
        let dir = std::env::temp_dir().join(format!(
            "adi-hive-demand-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("demand.json");

        let empty = Arc::new(Demand::new(Some(path.clone())));
        empty.publish();
        assert!(!path.exists(), "a route-only hive writes nothing");

        let demand = Arc::new(Demand::new(Some(path.clone())));
        let handle = demand.register("proj/web");
        handle.set_phase(Phase::Starting);
        let written = std::fs::read_to_string(&path).expect("the state file");
        assert!(written.contains("proj/web"), "{written}");
        assert!(written.contains("starting"), "{written}");

        handle.set_phase(Phase::Running);
        let written = std::fs::read_to_string(&path).expect("the state file");
        assert!(written.contains("running"), "{written}");

        demand.unpublish();
        assert!(!path.exists(), "shutdown takes the report with it");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
