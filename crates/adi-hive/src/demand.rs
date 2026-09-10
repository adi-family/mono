//! On-demand services: the one piece of state the front door and the supervisor share.
//!
//! A `start: on-demand` service is not launched at boot. It exists as a *route* and nothing else
//! until a request arrives for its host; the front door then [`touch`](Demand::touch)es it, which
//! both starts it (if it is down) and stamps the activity that keeps it up. When no request has
//! arrived for the service's idle window, the supervisor stops it again — see [`crate::runner`].
//!
//! The two halves are deliberately not wired to each other. The proxy knows only "a request landed
//! on this service name"; the supervisor knows only "this service was wanted at this instant". A
//! service the supervisor never registered — every `always` service — is simply unknown here, so
//! [`Demand::touch`] answers `false` and the front door behaves exactly as it did before on-demand
//! services existed.
//!
//! **Two processes.** Where the hive that routes and the hive that supervises are not the same
//! process — a `:80` front door routing what a per-user hive runs, which is how every machine with
//! a `routes_only` front door is installed — the registry above is empty in the routing process and
//! nothing in it could ever be woken by a visit. The gap is closed by [`crate::shared`]: the router
//! leaves a wake request in a file in the store, and the supervisor's [`bridge`] task picks it up
//! within a tick and touches the registry exactly as a local request would. The in-process registry
//! stays the fast path and is asked first; the file is the bridge, not a replacement.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use serde::Serialize;
use tokio::sync::watch;
use tracing::warn;

use crate::shared;

/// How often the shared files are re-read. Sub-second on purpose: this is the delay between a
/// visitor's first request and the service being asked to start, and they are looking at a holding
/// page for the whole of it. Cheap enough to run at this rate — two small files, read and parsed.
const BRIDGE_TICK: Duration = Duration::from_millis(200);

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

    /// Read back a phase another hive published. `Draining` never comes back, because it never went
    /// out — a draining service is published as running, and to a reader that is all it is.
    /// Anything unrecognised is `None`: a newer hive may publish a phase this one has no name for,
    /// and guessing at it would be worse than not knowing.
    fn parse(raw: &str) -> Option<Self> {
        match raw {
            "idle-stopped" => Some(Self::IdleStopped),
            "starting" => Some(Self::Starting),
            "running" => Some(Self::Running),
            _ => None,
        }
    }
}

/// What a routed request means for the service it landed on — the front door's whole question.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Wanted {
    /// Nothing to wake: an `always` service, or a service no hive on this machine supervises on
    /// demand. The front door treats it exactly as it did before any of this existed.
    No,
    /// On demand, and supervised by **this** process: the registry has already been touched, which
    /// both starts it if it is down and keeps it up if it is not.
    Here,
    /// On demand, and supervised by **another** hive: a wake request has been left for it in the
    /// shared file, keyed by the route it arrived on (which is what the field carries, so the
    /// caller can ask again — see [`Demand::wake_now`] — without re-deriving it).
    Elsewhere(String),
}

impl Wanted {
    /// Whether this request is one an on-demand service has to come up for — i.e. whether a
    /// refused upstream means "starting", not "broken".
    #[must_use]
    pub fn is_on_demand(&self) -> bool {
        *self != Self::No
    }
}

/// The registry of on-demand services: who is registered, when each was last wanted, and what each
/// is doing. Shared as an `Arc` between the proxy tasks and the supervisor's runner tasks.
#[derive(Debug, Default)]
pub struct Demand {
    services: Mutex<BTreeMap<String, Service>>,
    /// Where the phases are published for other processes, if anywhere.
    state_path: Option<PathBuf>,
    /// Whether this registry has ever written that file. Only a hive that has published removes it
    /// (see [`Self::publish`]) — the file is store-level now, so a hive that supervises nothing
    /// must not clean up after the one that does.
    published: AtomicBool,
    /// This hive's end of the shared files, when it has one.
    bridge: Option<Bridge>,
    /// What another hive last published about the services **it** supervises on demand, refreshed
    /// by [`bridge`]. It is what lets the front door tell a service that is down because nobody has
    /// asked for it from one that is down because it is broken.
    elsewhere: Mutex<BTreeMap<String, Phase>>,
}

/// The two ends of [`crate::shared`] this hive holds: what it asks of other hives, and what other
/// hives are asking of it.
#[derive(Debug)]
struct Bridge {
    wake: shared::Wake,
    /// Behind a mutex only because polling needs `&mut` and the [`bridge`] task holds an `Arc`;
    /// there is exactly one caller.
    inbox: Mutex<shared::Reader>,
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
    /// The route this service answers on, as [`crate::shared`] keys it. `None` for a service with
    /// no `proxy.host` — nothing can arrive for it, from this process or any other.
    route: Option<String>,
}

/// One service's end of the registry, held by the supervisor task that owns its process.
#[derive(Debug)]
pub struct Handle {
    name: String,
    demand: Arc<Demand>,
    activity: watch::Receiver<Instant>,
}

impl Demand {
    /// A registry that publishes its phases to `state_path` and talks to other hives through the
    /// wake file at `wake_path`. `None` for either keeps that half in memory only, which is what
    /// every test and every embedder that is not the daemon wants.
    #[must_use]
    pub fn new(state_path: Option<PathBuf>, wake_path: Option<PathBuf>) -> Self {
        Self {
            services: Mutex::new(BTreeMap::new()),
            state_path,
            published: AtomicBool::new(false),
            bridge: wake_path.map(|path| Bridge {
                wake: shared::Wake::new(path.clone()),
                inbox: Mutex::new(shared::Reader::new(path)),
            }),
            elsewhere: Mutex::new(BTreeMap::new()),
        }
    }

    /// Take charge of an on-demand service, from the supervisor task that will run it. Registering
    /// is what makes the front door able to wake it; until then it is an ordinary route.
    ///
    /// `route` is the key a request for this service arrives under (see [`crate::shared`]), so a
    /// wake written by another process can find it. Registering also publishes, which is what puts
    /// a service that has never run yet into the state file — the case that matters most, since a
    /// front door has to know a stopped service is *supposed* to be stopped before it asks for it.
    ///
    /// A re-registration (a service whose spec changed, so its task was replaced) keeps the
    /// existing activity stamp, so a reload does not read as a fresh hour of quiet.
    pub fn register(self: &Arc<Self>, name: &str, route: Option<String>) -> Handle {
        let (activity, added) = {
            let mut services = self.lock();
            let added = !services.contains_key(name);
            let entry = services.entry(name.to_string()).or_insert_with(|| Service {
                activity: watch::channel(Instant::now()).0,
                phase: Phase::IdleStopped,
                route: None,
            });
            entry.route = route;
            (entry.activity.subscribe(), added)
        };
        if added {
            self.publish();
        }
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
    /// to ask [`Self::wanted`] whether another hive supervises it.
    pub fn touch(&self, name: &str) -> bool {
        let services = self.lock();
        let Some(service) = services.get(name) else {
            return false;
        };
        service.activity.send_replace(Instant::now());
        true
    }

    /// Carry an activity stamp from another process into this service's idle clock: the service was
    /// last used `idle_for` ago, wherever that was observed.
    ///
    /// It moves the clock **forward only, and silently**. Forward only, because the newest stamp
    /// from any process is the one a service is judged on, and a poll that re-reads a file must not
    /// undo a request this process has since served itself. Silently — the receivers are not
    /// notified — because this says a service *was* used, not that anybody is waiting for it now: a
    /// notification here would start a stopped service on the strength of a stamp from minutes ago,
    /// which is precisely the resurrection an idle policy must not do. Starting is what
    /// [`Self::touch`] is for.
    pub fn extend(&self, name: &str, idle_for: Duration) {
        let services = self.lock();
        let Some(service) = services.get(name) else {
            return;
        };
        let Some(at) = Instant::now().checked_sub(idle_for) else {
            return;
        };
        service.activity.send_if_modified(|current| {
            if at > *current {
                *current = at;
            }
            false
        });
    }

    /// What a request that just landed on `route`, for `service`, means — and, when this hive is
    /// not the one that supervises it, the act of asking the hive that does.
    ///
    /// The order is the whole design: the in-process registry first, so a hive that both routes and
    /// supervises never touches the disk; then what another hive has published, so a request is
    /// written to the shared file only for a service somebody is actually supervising on demand.
    /// A machine where nothing is on demand therefore never writes the file at all.
    pub fn wanted(&self, service: &str, route: &str) -> Wanted {
        if self.touch(service) {
            return Wanted::Here;
        }
        let Some(phase) = self.elsewhere(service) else {
            return Wanted::No;
        };
        if let Some(bridge) = &self.bridge {
            // Ask for a start only when the last thing we heard was that it is not up. A running
            // service needs the activity stamp and nothing else.
            bridge.wake.stamp(route, phase != Phase::Running);
        }
        Wanted::Elsewhere(route.to_string())
    }

    /// Ask outright for the service behind `route` to be started: the front door tried the upstream
    /// and it refused the connection, which outranks whatever phase was last published — the
    /// process the other hive thinks it has is not answering.
    pub fn wake_now(&self, route: &str) {
        if let Some(bridge) = &self.bridge {
            bridge.wake.stamp(route, true);
        }
    }

    /// The phase another hive published for `service`, or `None` if no hive supervises it on demand.
    fn elsewhere(&self, service: &str) -> Option<Phase> {
        self.elsewhere
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(service)
            .copied()
    }

    /// One pass of the bridge: apply what other hives have asked of this one, and re-read what they
    /// have published. A no-op for a hive with no shared files configured.
    ///
    /// Public so a test can drive the crossing without a clock, and so the daemon's task below is
    /// nothing but a timer.
    pub fn absorb(&self) {
        if let Some(state) = self.state_path.as_ref() {
            let phases = shared::read_phases(state)
                .iter()
                .filter_map(|(name, raw)| Some((name.clone(), Phase::parse(raw)?)))
                .collect();
            *self
                .elsewhere
                .lock()
                .unwrap_or_else(PoisonError::into_inner) = phases;
        }
        let Some(bridge) = &self.bridge else {
            return;
        };
        let pending = bridge
            .inbox
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .poll();
        for shared::Pending {
            route,
            start,
            idle_for,
        } in pending
        {
            // A route this hive does not supervise is another hive's business — most of the file
            // usually is, on a machine with more than one front door.
            let Some(name) = self.service_on(&route) else {
                continue;
            };
            if start {
                self.touch(&name);
            } else {
                self.extend(&name, idle_for);
            }
        }
    }

    /// Which supervised service answers on `route`.
    fn service_on(&self, route: &str) -> Option<String> {
        self.lock()
            .iter()
            .find(|(_, svc)| svc.route.as_deref() == Some(route))
            .map(|(name, _)| name.clone())
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

    /// Write the phases where other processes can read them: the control panel's `GET /api/hive`,
    /// which otherwise can only tell "listening" from "not listening" and would report a service
    /// that is *starting* as stopped — and the front door, which reads it to learn which services
    /// are supervised on demand at all before it asks for one.
    ///
    /// Nothing is written by a hive with no on-demand services registered, and such a hive removes
    /// nothing either unless it is taking away phases it published itself. Both halves of that rule
    /// exist for the same reason: the file is one store-level path now, and a route-only front door
    /// reads it while its supervisor writes it.
    ///
    /// This is also what creates the store's `hive/` directory, deliberately — the supervisor is
    /// the unprivileged process of the pair, so the directory ends up owned by the user who has to
    /// keep writing into it, rather than by a root front door that happened to start first.
    fn publish(&self) {
        let Some(path) = self.state_path.as_ref() else {
            return;
        };
        let phases = self.phases();
        if phases.is_empty() {
            if self.published.swap(false, Ordering::Relaxed) {
                let _ = std::fs::remove_file(path);
            }
            return;
        }
        let named: BTreeMap<&str, &str> = phases
            .iter()
            .map(|(name, phase)| (name.as_str(), phase.as_str()))
            .collect();
        let written = serde_json::to_vec_pretty(&named)
            .map_err(std::io::Error::other)
            .and_then(|json| {
                if let Some(dir) = path.parent() {
                    std::fs::create_dir_all(dir)?;
                }
                shared::write(path, &json)
            });
        match written {
            Ok(()) => self.published.store(true, Ordering::Relaxed),
            Err(e) => {
                warn!(error = %e, path = %path.display(), "could not write the on-demand state file");
            }
        }
    }

    /// Remove the published state file, at shutdown — the phases in it are about a process that is
    /// no longer running. A hive that never published leaves it alone: on a split install that file
    /// belongs to the other hive, which is still running.
    pub fn unpublish(&self) {
        if let (Some(path), true) = (
            self.state_path.as_ref(),
            self.published.swap(false, Ordering::Relaxed),
        ) {
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

/// Run the bridge for the life of the process: nothing but a timer over [`Demand::absorb`].
///
/// One task, whichever half this hive is. A route-only front door uses it to keep its picture of
/// what the supervisor is running up to date; the supervisor uses it to hear the requests the front
/// door routed. A hive that is both simply finds nothing to do in it, because its own requests
/// never reached the disk.
pub async fn bridge(demand: Arc<Demand>) {
    let mut ticker = tokio::time::interval(BRIDGE_TICK);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        ticker.tick().await;
        demand.absorb();
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    /// A registry with no files behind it — every test about the in-process half.
    fn local() -> Arc<Demand> {
        Arc::new(Demand::default())
    }

    /// A scratch store directory, standing in for `~/.adi/mono/hive`.
    fn scratch(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "adi-hive-demand-{label}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("a scratch dir");
        dir
    }

    /// The contract the front door depends on: a registered service can be woken, and one this
    /// hive does not supervise says so rather than pretending.
    #[test]
    fn only_a_registered_service_can_be_touched() {
        let demand = local();
        assert!(!demand.touch("web"), "nothing is registered yet");
        let handle = demand.register("web", None);
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
        let demand = local();
        let mut handle = demand.register("web", None);

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
        let demand = local();
        let mut handle = demand.register("web", None);
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
        let demand = local();
        let mut first = demand.register("web", None);
        demand.touch("web");
        std::thread::sleep(std::time::Duration::from_millis(20));
        let mut second = demand.register("web", None);
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

    /// The state file is what the control panel reads to tell "starting" from "stopped", and what
    /// the front door reads to know a service is supervised on demand at all. A hive supervising
    /// nothing on demand must neither write it nor remove it — on a split install it belongs to the
    /// other hive, which is still running.
    #[test]
    fn the_state_file_is_written_only_by_a_hive_that_supervises_something() {
        let dir = scratch("state");
        let path = dir.join("demand.json");

        let empty = Arc::new(Demand::new(Some(path.clone()), None));
        empty.publish();
        assert!(!path.exists(), "a route-only hive writes nothing");

        let demand = Arc::new(Demand::new(Some(path.clone()), None));
        let handle = demand.register("proj/web", Some("web.adi".to_string()));
        let written = std::fs::read_to_string(&path).expect("the state file");
        assert!(
            written.contains("idle-stopped"),
            "a service that has never run is still one the front door may wake: {written}"
        );

        handle.set_phase(Phase::Starting);
        let written = std::fs::read_to_string(&path).expect("the state file");
        assert!(written.contains("proj/web"), "{written}");
        assert!(written.contains("starting"), "{written}");

        handle.set_phase(Phase::Running);
        let written = std::fs::read_to_string(&path).expect("the state file");
        assert!(written.contains("running"), "{written}");

        empty.unpublish();
        assert!(
            path.exists(),
            "a hive removes only what it published itself"
        );

        demand.unpublish();
        assert!(!path.exists(), "shutdown takes the report with it");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A pair of registries over one pair of files, which is what a machine with a route-only front
    /// door actually has: the routing hive supervises nothing, the supervising hive routes nothing,
    /// and a visit still has to start a service.
    fn split(dir: &Path) -> (Arc<Demand>, Arc<Demand>) {
        let (state, wake) = (dir.join("demand.json"), dir.join("wake.json"));
        let supervisor = Arc::new(Demand::new(Some(state.clone()), Some(wake.clone())));
        let router = Arc::new(Demand::new(Some(state), Some(wake)));
        (router, supervisor)
    }

    /// The whole point of the shared files: a request that arrives at the hive which only routes
    /// starts the service in the hive that supervises it.
    #[tokio::test]
    async fn a_wake_crosses_from_the_hive_that_routes_to_the_hive_that_runs() {
        let dir = scratch("crossing");
        let (router, supervisor) = split(&dir);
        let mut handle = supervisor.register("proj/web", Some("web.adi".to_string()));

        // The router knows nothing until it has read what the supervisor published.
        assert_eq!(router.wanted("proj/web", "web.adi"), Wanted::No);
        router.absorb();
        assert_eq!(
            router.wanted("proj/web", "web.adi"),
            Wanted::Elsewhere("web.adi".to_string()),
            "the request is for a service somebody else supervises"
        );

        supervisor.absorb();
        tokio::time::timeout(
            std::time::Duration::from_millis(100),
            handle.wait_for_activity(),
        )
        .await
        .expect("the visit reached the supervisor");

        // And it is consumed: the same request must not start the service twice.
        supervisor.absorb();
        assert!(
            tokio::time::timeout(
                std::time::Duration::from_millis(50),
                handle.wait_for_activity()
            )
            .await
            .is_err(),
            "one visit is one start"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The other half, and the one that costs somebody their session if it is missing: activity
    /// observed in the routing process has to reach the idle clock the supervisor judges by, or a
    /// service being used all afternoon is stopped for being quiet.
    #[tokio::test]
    async fn activity_crosses_so_a_busy_service_is_not_idle_stopped() {
        let dir = scratch("activity");
        let (router, supervisor) = split(&dir);
        let mut handle = supervisor.register("proj/web", Some("web.adi".to_string()));
        handle.set_phase(Phase::Running);
        router.absorb();

        // Time passes with nothing arriving in this process.
        tokio::time::sleep(std::time::Duration::from_millis(60)).await;
        assert!(handle.idle_for() >= std::time::Duration::from_millis(60));

        // A request lands on the front door instead.
        assert!(router.wanted("proj/web", "web.adi").is_on_demand());
        supervisor.absorb();
        assert!(
            handle.idle_for() < std::time::Duration::from_millis(60),
            "the front door's request is the newest activity this service has"
        );

        // But it is not a start: the service is up, and a stamp is not somebody waiting.
        assert!(
            tokio::time::timeout(
                std::time::Duration::from_millis(50),
                handle.wait_for_activity()
            )
            .await
            .is_err(),
            "an activity stamp must not wake the supervisor task"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A stamp left behind by a visitor who has long gone must never bring a service back — that is
    /// the one thing an idle policy exists to prevent — and it must not drag the idle clock
    /// backwards either.
    #[tokio::test]
    async fn a_stale_entry_neither_starts_a_service_nor_rewinds_its_clock() {
        let dir = scratch("stale");
        let (_, supervisor) = split(&dir);
        let mut handle = supervisor.register("proj/web", Some("web.adi".to_string()));
        handle.touch();

        // An hour-old visit, and one for a host that no longer exists at all.
        let old = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("a clock after 1970")
            .as_millis();
        let old = u64::try_from(old).expect("a unix stamp fits a u64") - 60 * 60 * 1000;
        let entries = BTreeMap::from([
            (
                "web.adi".to_string(),
                shared::Entry {
                    wake: Some(old),
                    activity: old,
                },
            ),
            (
                "gone.adi".to_string(),
                shared::Entry {
                    wake: Some(old),
                    activity: old,
                },
            ),
        ]);
        shared::write(
            &dir.join("wake.json"),
            &serde_json::to_vec(&entries).expect("json"),
        )
        .expect("seed the wake file");

        supervisor.absorb();
        assert!(
            tokio::time::timeout(
                std::time::Duration::from_millis(50),
                handle.wait_for_activity()
            )
            .await
            .is_err(),
            "an hour-old request starts nothing"
        );
        assert!(
            handle.idle_for() < std::time::Duration::from_secs(60),
            "and it does not make a service that was just used look idle"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The single-process path is untouched by any of this: the registry answers, and nothing is
    /// written to a file at all — which is also why a machine with no on-demand service anywhere
    /// never creates one.
    #[test]
    fn a_hive_that_routes_what_it_supervises_never_touches_the_shared_file() {
        let dir = scratch("single");
        let (state, wake) = (dir.join("demand.json"), dir.join("wake.json"));
        let hive = Arc::new(Demand::new(Some(state), Some(wake.clone())));
        let _handle = hive.register("proj/web", Some("web.adi".to_string()));
        hive.absorb();

        assert_eq!(hive.wanted("proj/web", "web.adi"), Wanted::Here);
        assert!(!wake.exists(), "the in-process registry answered it");

        // And a service nobody supervises on demand is nobody's to wake, wherever it lives.
        assert_eq!(hive.wanted("proj/worker", "worker.adi"), Wanted::No);
        assert!(!wake.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
