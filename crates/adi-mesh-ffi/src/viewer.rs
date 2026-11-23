//! The viewer half of the mesh, for a device that hosts nothing.
//!
//! A phone is the one machine in the fleet that is *only* a viewer. It has no hive, no route
//! table, no services, and it never answers `adi/mesh/http/1` — so this runtime is deliberately
//! not [`adi_mesh::Daemon`] with parts switched off. It binds one endpoint answering exactly one
//! ALPN ([`join`]), which is the whole of what a viewer must accept: a node dialling back to be
//! paired. Everything else it does, it *initiates*.
//!
//! ## Why a port per service, and not one gateway
//!
//! On a Mac the front door owns `*.n.adi` and hands the gateway a `Host` to parse. A `WKWebView`
//! resolves names through the system, and nothing on an unjailbroken iPhone can make `n.adi`
//! resolve without a Network Extension — so there is no hostname to parse and the host-based
//! gateway has nothing to work with.
//!
//! Instead each `(node, service)` pair gets its **own loopback listener**, and the target is fixed
//! when the listener is bound rather than read off each request. That keeps the one property §4
//! exists to protect: a service is still exactly one origin, so relative URLs, cookies,
//! `localStorage` and WebSocket upgrades all behave as they do anywhere else. It is also why
//! [`ports`] persists the port it picked — an origin that changed port between launches would drop
//! the page's stored state on the floor every time the app restarted.
//!
//! The `Host` header the node ends up seeing is `127.0.0.1:<port>`. That is correct, not a
//! compromise: §3 forbids rewriting it precisely so a node's absolute redirects land back on the
//! origin the browser is on, and here that origin *is* the loopback port. Routing on the far side
//! never consults `Host` — it uses the service label in the frame — so nothing depends on the name.

use std::collections::HashMap;
use std::future::Future;
use std::net::{Ipv4Addr, SocketAddr};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use adi_mesh::config::MeshConfig;
use adi_mesh::fleet::{FleetRegistry, Grant, Scope};
use adi_mesh::gateway::{IrohDialer, Pool};
use adi_mesh::protocol::{self, HttpStatus};
use adi_mesh::{dns, identity, join, relay, ticket, tunnel};
use anyhow::Context as _;
use iroh::endpoint::{RecvStream, SendStream, presets};
use iroh::{Endpoint, EndpointId};
use serde::Serialize;
use tokio::io::AsyncWriteExt as _;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::watch;
use tokio::time::{Instant, timeout};
use tracing::{debug, info, warn};

mod catalog;
mod ports;

pub use catalog::{Catalog, DashboardInfo};

/// How long to wait for a home relay before settling for the address we have.
///
/// The same bound [`adi_mesh::Daemon`] uses. An invite minted without a relay only works on the
/// local network, so this is worth waiting for — but not worth blocking a UI on forever.
const RELAY_WAIT: Duration = Duration::from_secs(8);

/// How long an invite minted for the UI stays spendable. Short on purpose: the flow is "show a QR,
/// scan it on the node, done", which takes seconds, and an invite is a bearer token until spent.
pub const INVITE_TTL: Duration = Duration::from_secs(10 * 60);

/// After an accept error, pause so a persistent failure cannot spin the loop hot.
const ACCEPT_ERROR_BACKOFF: Duration = Duration::from_millis(100);

/// How long any one step of the mesh round trip may take before the request is given up on.
///
/// **This is the difference between an error and a white screen.** iOS suspends the app; the QUIC
/// connection in the pool dies without ever being *closed*, so it still reads as usable, and the
/// next `open_bi` waits on a peer that will never answer. Meanwhile the loopback listener is a
/// kernel socket — it accepts the browser's connection whether or not anything is left to service
/// it. With no bound on the wait, the page loads forever: no error, no timeout of its own, nothing
/// to retry. Twelve seconds is long enough for a relay round trip on a bad connection and short
/// enough that a person is still looking at the screen when the answer arrives.
const STEP_TIMEOUT: Duration = Duration::from_secs(12);

/// How long a retired endpoint gets to tell its peers it is going before it is dropped anyway.
///
/// A reconnect is ordered exactly when the network under the endpoint is suspect, which is also
/// when a goodbye may never be acknowledged. iroh's own bound for that is three seconds; the
/// person holding the phone is waiting on the *new* endpoint, so this one is shorter, and a peer
/// that never hears the close finds out from its idle timeout.
const CLOSE_WAIT: Duration = Duration::from_secs(2);

/// A reconnect asked for this soon after one finished is the same event reported twice.
///
/// Coming back to the foreground on a different network is one thing that happened, and the app
/// hears of it from two places — the scene phase and the path monitor — a moment apart. Without
/// this the second report would throw away the connections the first one had just made.
const RECONNECT_QUIET: Duration = Duration::from_secs(2);

/// How many times one request starts over because the endpoint was replaced under it.
///
/// A request in flight when the phone changes network fails on the *old* transport, and that
/// failure says nothing about the new one — so it is tried again there instead of being shown.
/// Bounded, so two reconnects in a row cannot keep a request alive past its own timeout's intent.
const REPLACED_RETRIES: u32 = 2;

/// How many requests in a row must fail before the viewer reconnects without being told to.
const FAILURES_BEFORE_RECONNECT: u32 = 2;

/// How long nothing at all must have succeeded before it does. Together with the count above this
/// is what "the connection is lost" means from in here: not one node that is asleep while another
/// answers, but nothing answering, for a while, on a transport that is not carrying anything.
const SILENCE_BEFORE_RECONNECT: Duration = Duration::from_secs(20);

/// The shortest gap between two reconnects the viewer orders by itself, doubling after each one up
/// to [`AUTO_RECONNECT_CAP`] and reset by the first request that succeeds. A fleet whose only
/// node is switched off looks exactly like a lost connection, and must not cost a new endpoint
/// every half minute for as long as somebody leaves the app open on it.
const AUTO_RECONNECT_BASE: Duration = Duration::from_secs(15);

/// The ceiling on that gap.
const AUTO_RECONNECT_CAP: Duration = Duration::from_secs(5 * 60);

/// How long a machine that answered before gets to answer again, when it is asked to vouch for
/// the endpoint. See [`heal`].
const WITNESS_TIMEOUT: Duration = Duration::from_secs(6);

/// The first and the longest pause between attempts to bind something that would not bind — the
/// endpoint after a reconnect, a loopback port after its listener was lost.
const REBIND_BACKOFF_BASE: Duration = Duration::from_millis(50);
const REBIND_BACKOFF_CAP: Duration = Duration::from_secs(2);

// ---------------------------------------------------------------------------------------
// What crosses the FFI
// ---------------------------------------------------------------------------------------

/// One paired node, as the UI lists it.
#[derive(Debug, Serialize)]
pub struct NodeInfo {
    /// What this phone calls the node — the label a service is addressed under.
    pub petname: String,
    /// The node's key: the identity of record, shown so a human has something to verify.
    pub key: String,
    /// What the node last called itself.
    pub nickname: String,
    /// Unix seconds at which the petname was bound to the key.
    pub paired_at: u64,
    /// A nickname the node has declared since pairing and that we have *not* adopted (§2 rule 4).
    /// Surfaced so the UI can offer the rename rather than perform it.
    pub pending_nickname: Option<String>,
    /// The service labels this phone may ask the node for, derived from the grants it holds.
    pub services: Vec<String>,
    /// True when the node granted `http:*`, i.e. [`Self::services`] is a starting point rather
    /// than the whole list, and the UI may let a human name another service.
    pub any_service: bool,
}

/// A pairing that completed while the app was running.
///
/// The password is here because this side minted it (§8) and there is no terminal to print it to;
/// the UI is expected to move it into the Keychain and drop it. It is handed over exactly once —
/// [`Viewer::take_pairings`] drains the queue.
#[derive(Debug, Serialize)]
pub struct Paired {
    /// The petname the node was filed under.
    pub petname: String,
    /// The username its services will demand.
    pub username: String,
    /// The plaintext password, for the Keychain and nowhere else.
    pub password: String,
}

// ---------------------------------------------------------------------------------------
// The runtime
// ---------------------------------------------------------------------------------------

/// What requests travel over: one endpoint, and the pool of connections dialled from it.
///
/// The two are replaced *together or separately*, and the generation says that either happened.
/// A new pool on the same endpoint is the cheap answer to "these connections are suspect"; a new
/// endpoint is the answer to "this endpoint is" — see [`reconnect`] for when that is.
#[derive(Debug)]
struct Transport {
    endpoint: Endpoint,
    pool: Pool<IrohDialer>,
    /// Which transport this is, counted from one. A request remembers the generation it ran on,
    /// which is how a failure is charged to the transport it happened on and to no other.
    generation: u64,
    /// Which *endpoint* this is, counted from one: unchanged by a new pool, moved on by a
    /// reconnect. How a request tells "my connection failed" from "the ground under it was
    /// replaced" — iroh's own `is_closed` only turns true once a close has finished.
    binding: u64,
}

/// Where this viewer's endpoints come from — the one it starts with, and each one a
/// [`reconnect`] replaces it with.
enum Origin {
    /// The app's: bound on the stored identity ([`bind_endpoint`]), publishing a ticket and
    /// accepting pairings ([`attend`]).
    Stored,
    /// A test's: whatever the closure binds, with no store behind it and nothing attending it —
    /// so the whole of the reconnect machinery can run against a machine made up on the spot.
    #[cfg(test)]
    Given(Box<dyn Fn() -> tests::Binding + Send + Sync>),
}

impl Origin {
    async fn bind(&self) -> anyhow::Result<Endpoint> {
        match self {
            Self::Stored => bind_endpoint().await,
            #[cfg(test)]
            Self::Given(bind) => bind().await,
        }
    }

    /// Whether an endpoint from here gets the two tasks of [`attend`].
    fn attended(&self) -> bool {
        matches!(self, Self::Stored)
    }
}

impl std::fmt::Debug for Origin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Stored => "Stored",
            #[cfg(test)]
            Self::Given(_) => "Given",
        })
    }
}

/// Whether anything is getting through, and what was last done about it when nothing was.
#[derive(Debug)]
struct Health {
    /// Requests that failed since one last succeeded.
    failures: u32,
    /// When a request last made a full round trip — or when the transport it would have used was
    /// put in place, so a fresh endpoint gets its chance before it is judged.
    last_ok: Instant,
    /// The earliest the viewer may next reconnect on its own initiative.
    next_auto: Instant,
    /// How long after that one the following may come. See [`AUTO_RECONNECT_BASE`].
    backoff: Duration,
    /// When the last reconnect was asked for, and when it began: what tells a report of
    /// something new from a second report of the same thing.
    reconnect_asked: Option<Instant>,
    reconnect_began: Option<Instant>,
}

/// Everything shared with the spawned tasks.
#[derive(Debug)]
struct Shared {
    origin: Origin,
    /// The transport in use, or `None` for the moment a reconnect is replacing the endpoint — a
    /// request that arrives then waits for the new one rather than failing on the old.
    transport: watch::Sender<Option<Arc<Transport>>>,
    /// The generation the next transport will carry, and the binding the next endpoint will.
    generations: AtomicU64,
    bindings: AtomicU64,
    /// One reconnect at a time: two endpoints alive on one key race for the same relay session.
    reconnecting: tokio::sync::Mutex<()>,
    health: Mutex<Health>,
    /// The machines that have answered during this run and not failed since, and when each last
    /// did: the ones that can vouch for the endpoint when another stops answering. See [`heal`].
    answered: Mutex<HashMap<EndpointId, Instant>>,
    /// How many browser connections are spliced to a node right now. A spliced stream on a dead
    /// connection ends within iroh's idle timeout, so a count above zero is evidence that
    /// *something* is still being carried — and a reason not to reconnect over one quiet node.
    splices: AtomicUsize,
    /// Bumped to make every loopback listener bind its port afresh. See [`Viewer::resume`].
    listeners: watch::Sender<u64>,
    /// How many times a listener has done so. Nothing acts on it; it is what a log line and a
    /// test can point at to say the port in use is not the socket it was.
    rebound: AtomicU64,
    /// Flipped once, by [`Viewer::stop`].
    shutdown: watch::Sender<bool>,
    /// This run's ticket, once the relay is known. `None` until then, which is also the honest
    /// answer to "can a node reach me yet?".
    ticket: Mutex<Option<String>>,
    /// Pairings not yet collected by the UI.
    pairings: Mutex<Vec<Paired>>,
    /// `(node, service)` → the loopback port serving it, for listeners already bound this run.
    open: Mutex<HashMap<(String, String), u16>>,
}

impl Shared {
    fn new(origin: Origin) -> Self {
        let now = Instant::now();
        Self {
            origin,
            transport: watch::Sender::new(None),
            generations: AtomicU64::new(1),
            reconnecting: tokio::sync::Mutex::new(()),
            health: Mutex::new(Health {
                failures: 0,
                last_ok: now,
                next_auto: now,
                backoff: AUTO_RECONNECT_BASE,
                reconnect_asked: None,
                reconnect_began: None,
            }),
            bindings: AtomicU64::new(1),
            answered: Mutex::new(HashMap::new()),
            splices: AtomicUsize::new(0),
            listeners: watch::Sender::new(0),
            rebound: AtomicU64::new(0),
            shutdown: watch::Sender::new(false),
            ticket: Mutex::new(None),
            pairings: Mutex::new(Vec::new()),
            open: Mutex::new(HashMap::new()),
        }
    }

    fn health(&self) -> std::sync::MutexGuard<'_, Health> {
        self.health.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn answered(&self) -> std::sync::MutexGuard<'_, HashMap<EndpointId, Instant>> {
        self.answered.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn stopping(&self) -> bool {
        *self.shutdown.borrow()
    }

    /// The transport in use right now, if a reconnect is not in the middle of replacing it.
    fn current(&self) -> Option<Arc<Transport>> {
        self.transport.borrow().clone()
    }

    /// The transport to send a request over, waiting out a reconnect that is in progress.
    ///
    /// Unbounded on its own: every caller is already inside a [`STEP_TIMEOUT`].
    async fn transport(&self) -> anyhow::Result<Arc<Transport>> {
        let mut seen = self.transport.subscribe();
        let ready = seen
            .wait_for(Option::is_some)
            .await
            .map_err(|_| anyhow::anyhow!("the mesh has been stopped"))?
            .clone();
        ready.context("the mesh is reconnecting")
    }

    /// Has endpoint `binding` been replaced — by a new one, or by the gap before a new one?
    ///
    /// A new *pool* on the same endpoint is deliberately not this. A request that failed over a
    /// pool somebody else has since retired failed for the reason they retired it, and going
    /// again would only multiply the dials to a node that is off.
    fn rebound(&self, binding: u64) -> bool {
        self.transport
            .borrow()
            .as_ref()
            .is_none_or(|current| current.binding != binding)
    }

    /// Put `endpoint` in use, with an empty pool and the next generation.
    fn install(&self, endpoint: Endpoint) -> Arc<Transport> {
        let transport = Arc::new(Transport {
            pool: Pool::new(IrohDialer::new(endpoint.clone())),
            endpoint,
            generation: self.generations.fetch_add(1, Ordering::Relaxed),
            binding: self.bindings.fetch_add(1, Ordering::Relaxed),
        });
        self.transport.send_replace(Some(Arc::clone(&transport)));
        transport
    }

    /// Throw the pool of transport `generation` away, so the next request dials fresh — unless
    /// that transport has been replaced already. Returns whether this call was the one that did.
    ///
    /// A whole new [`Pool`] rather than an eviction, because what has to go is not one entry but
    /// the pool's memory of every peer: after a suspension *all* of its connections are dead, and
    /// each still reports itself usable ([`iroh`] only learns otherwise once its idle timeout
    /// fires). A request already holding the old transport finishes against it and picks the new
    /// one up on the next.
    ///
    /// **Only for the generation that failed**, because failures arrive in batches: every request
    /// that was waiting on a dead connection times out in the same second, and if each of them
    /// retired "the current pool" the second would throw away the connection the first one's
    /// retry had just dialled.
    fn retire_pool(&self, generation: u64) -> bool {
        self.transport.send_if_modified(|slot| {
            let Some(current) = slot.as_ref().filter(|t| t.generation == generation) else {
                return false;
            };
            let (endpoint, binding) = (current.endpoint.clone(), current.binding);
            *slot = Some(Arc::new(Transport {
                pool: Pool::new(IrohDialer::new(endpoint.clone())),
                endpoint,
                generation: self.generations.fetch_add(1, Ordering::Relaxed),
                binding,
            }));
            true
        })
    }

    /// `peer` answered a request in full: whatever was wrong is over.
    fn succeeded(&self, peer: EndpointId) {
        let now = Instant::now();
        self.answered().insert(peer, now);
        let mut health = self.health();
        health.failures = 0;
        health.last_ok = now;
        health.backoff = AUTO_RECONNECT_BASE;
    }

    /// A request could not reach `peer` over transport `generation`.
    ///
    /// The pool it used is retired, as it always was. What is new is the second step: when
    /// requests keep failing and nothing has succeeded in between, the fault may not be in the
    /// connections at all but under them — an endpoint whose sockets the OS took back while the
    /// app was frozen, a relay session nobody noticed dying — and no number of fresh dials over
    /// the same endpoint finds that out. So the viewer goes looking for a connection again, from
    /// the socket up. See [`heal`], which first makes sure that is what the failures mean.
    fn failed(self: &Arc<Self>, peer: EndpointId, generation: u64) {
        self.retire_pool(generation);
        // A machine that is not answering cannot vouch for anything.
        self.answered().remove(&peer);
        let now = Instant::now();
        let due = {
            let mut health = self.health();
            health.failures = health.failures.saturating_add(1);
            let due = reconnect_is_due(&health, now, self.splices.load(Ordering::Relaxed));
            if due {
                health.next_auto = now + health.backoff;
                health.backoff = health.backoff.saturating_mul(2).min(AUTO_RECONNECT_CAP);
            }
            due
        };
        if due && !self.stopping() {
            tokio::spawn(heal(Arc::clone(self)));
        }
    }

    /// The machine that answered most recently and has not failed since, if there is one.
    fn witness(&self) -> Option<EndpointId> {
        self.answered()
            .iter()
            .max_by_key(|(_, at)| **at)
            .map(|(peer, _)| *peer)
    }
}

/// Is it time for the viewer to reconnect without being asked?
///
/// Every condition is one way of *not* mistaking something ordinary for a lost connection: one
/// failure is a blip, a recent success means some node still answers, a live splice means some
/// connection still carries bytes, and [`Health::next_auto`] is the backoff that keeps a fleet
/// whose only node is off from costing an endpoint per poll.
fn reconnect_is_due(health: &Health, now: Instant, live_splices: usize) -> bool {
    health.failures >= FAILURES_BEFORE_RECONNECT
        && now.saturating_duration_since(health.last_ok) >= SILENCE_BEFORE_RECONNECT
        && now >= health.next_auto
        && live_splices == 0
}

/// Counts one spliced browser connection for as long as it is held.
struct LiveSplice<'a>(&'a AtomicUsize);

impl<'a> LiveSplice<'a> {
    fn begin(count: &'a AtomicUsize) -> Self {
        count.fetch_add(1, Ordering::Relaxed);
        Self(count)
    }
}

impl Drop for LiveSplice<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::Relaxed);
    }
}

/// A running viewer: the tasks it spawned, the transport they share, and the runtime they live on.
#[derive(Debug)]
pub struct Viewer {
    /// Owned so the tasks below outlive the call that started them, and die with this value.
    rt: tokio::runtime::Runtime,
    shared: Arc<Shared>,
    key: EndpointId,
}

impl Viewer {
    /// Bind the endpoint and start accepting pairings.
    ///
    /// `home` overrides `$HOME`, which is what decides where the store lives (`$HOME/.adi/mono`).
    /// On iOS that is already the app's container, so passing the container path is a statement of
    /// intent rather than a correction — but it is also the seam a test uses to get a scratch store.
    ///
    /// # Errors
    /// If the identity cannot be read or the endpoint cannot bind.
    pub fn start(home: Option<&str>) -> anyhow::Result<Self> {
        if let Some(home) = home.map(str::trim).filter(|h| !h.is_empty()) {
            // SAFETY: `set_var` is unsound only when another thread is reading the environment
            // concurrently. This runs before the tokio runtime exists, on the one call that is
            // documented to happen once at launch and before any other entry point.
            unsafe { std::env::set_var("HOME", home) };
        }

        let rt = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .context("building the mesh runtime")?;

        let endpoint = rt.block_on(bind_endpoint())?;
        let key = endpoint.id();
        info!(%key, "adi-mesh viewer bound");

        let shared = Arc::new(Shared::new(Origin::Stored));
        {
            // `attend` spawns, and spawning needs the runtime entered.
            let _entered = rt.enter();
            attend(&shared, &shared.install(endpoint));
        }

        Ok(Self { rt, shared, key })
    }

    /// This phone's key — the thing a node's operator authorizes, and the only true name it has.
    #[must_use]
    pub fn key(&self) -> String {
        self.key.to_string()
    }

    /// This run's ticket, or `None` while the relay session is still coming up.
    #[must_use]
    pub fn ticket(&self) -> Option<String> {
        self.shared
            .ticket
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// Mint a single-use invite for a node to spend with `adi-mono mesh join <token>`.
    ///
    /// # Errors
    /// If the relay session is not up yet (the invite would name an endpoint nothing can dial), or
    /// the invite book cannot be written.
    pub fn invite(&self) -> anyhow::Result<String> {
        let ticket = self.ticket().context(
            "this phone has no relay session yet, so a node would have nothing to dial — \
             wait a moment and try again",
        )?;
        join::mint_invite_for(
            &ticket,
            INVITE_TTL,
            &adi_config::Config::open(),
            adi_config::now_unix(),
        )
    }

    /// Spend an invite a **machine** minted, pairing with it from this side.
    ///
    /// The mirror of [`invite`](Self::invite), and the other direction the
    /// protocol allows: there the phone mints and the node dials, here the node mints
    /// (`adi-mono mesh invite`) and the phone dials. Both end with the same record in the same
    /// registry, so a node paired this way is indistinguishable afterwards — it appears in
    /// [`nodes`](Self::nodes) like any other, and the petname it lands under is the `viewer-<hex>`
    /// form until it is renamed.
    ///
    /// This exists because a phone cannot always be dialled *first*: whoever holds the machine may
    /// be somewhere else, and a person handed a token has no terminal to spend it from. It is also
    /// what lets a fleet be shown to someone who does not own one yet.
    ///
    /// Uses [`join_on`](join::join_on) rather than `join` so the dial goes out over the endpoint
    /// this viewer already holds. Binding a second one on the same key would race the first for
    /// its relay session — see that function's note.
    ///
    /// Answers a [`Paired`] and not just a name, for the reason that struct exists: the machine
    /// mints the password in this direction, `record_viewer` keeps only a *digest* of it in
    /// `fleet.toml`, and the plaintext is never recoverable afterwards. It crosses the FFI once,
    /// here, and the app's only correct move is to put it in the Keychain — the same contract
    /// [`take_pairings`](Self::take_pairings) has for the other direction.
    ///
    /// # Errors
    /// If the token is malformed, expired or already spent, if the minting machine cannot be
    /// reached, or if it refuses.
    pub fn join(&self, token: &str) -> anyhow::Result<Paired> {
        let token = token.trim().to_string();
        let shared = Arc::clone(&self.shared);
        let joined = self.rt.block_on(async move {
            // Whichever endpoint is current — and the new one, if a reconnect is under way.
            let transport = timeout(STEP_TIMEOUT, shared.transport())
                .await
                .context("the mesh is still reconnecting — try again in a moment")??;
            join::join_on(&transport.endpoint, &token).await
        })?;
        Ok(Paired {
            // `viewer` is what *this* device now calls the machine; `petname` is what the machine
            // decided to call us. The Keychain is keyed by the former, because that is the name
            // every later call here passes back in.
            petname: joined.viewer,
            username: joined.username,
            password: joined.password,
        })
    }

    /// Every node paired with this phone.
    ///
    /// # Errors
    /// Any error reading the fleet registry.
    pub fn nodes(&self) -> anyhow::Result<Vec<NodeInfo>> {
        let registry = FleetRegistry::load()?;
        Ok(registry
            .nodes
            .iter()
            .map(|(petname, record)| {
                let (services, any_service) = grantable_services(&record.grants);
                NodeInfo {
                    petname: petname.clone(),
                    key: record.key.clone(),
                    nickname: record.nickname.clone(),
                    paired_at: record.paired_at,
                    pending_nickname: record.pending_nickname.clone(),
                    services,
                    any_service,
                }
            })
            .collect())
    }

    /// The loopback port serving `service` on `node`, binding a listener if this is the first ask.
    ///
    /// Idempotent: asking twice returns the same port, and the port survives across launches (see
    /// [`ports`]) so the page's origin — and everything the browser keyed to it — stays put.
    ///
    /// # Errors
    /// If the node is not paired, or no loopback port can be bound.
    pub fn open(&self, node: &str, service: &str) -> anyhow::Result<u16> {
        anyhow::ensure!(
            protocol::is_service_name(service),
            "{service:?} is not a service name (one or more lowercase DNS labels, `app.nosh`)"
        );
        let route = (node.to_string(), service.to_string());
        if let Some(port) = self
            .shared
            .open
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&route)
        {
            return Ok(*port);
        }

        // Resolved now rather than per request: a listener is bound to one node, so a node
        // forgotten later should stop answering, not silently start reaching a re-used petname.
        let key = FleetRegistry::load()?
            .key_of(node)
            .with_context(|| format!("no node called {node:?} is paired with this phone"))?;

        let listener = self
            .rt
            .block_on(ports::bind_stable(node, service))
            .with_context(|| format!("binding a local port for {service}.{node}"))?;
        let port = listener.local_addr()?.port();

        self.rt.spawn(serve_service(
            listener,
            port,
            Arc::clone(&self.shared),
            key,
            service.to_string(),
        ));
        self.shared
            .open
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(route, port);
        info!(%node, %service, port, "serving a node's service on loopback");
        Ok(port)
    }

    /// The dashboards `node` publishes, and which of them this phone may open.
    ///
    /// The credential is the node's own Basic-auth pair — the human-scoped half of §5 — and it is
    /// passed in per call rather than held: on a phone it lives in the Keychain, and a copy kept
    /// here would be a second place for it to leak from.
    ///
    /// # Errors
    /// If the node is not paired, cannot be reached, or refuses the credential.
    pub fn dashboards(
        &self,
        node: &str,
        username: &str,
        password: &str,
    ) -> anyhow::Result<Catalog> {
        let (key, grants) = paired(node)?;
        let me = self.key();
        let shared = Arc::clone(&self.shared);
        self.rt.block_on(async move {
            catalog::fetch(&shared, key, &me, (username, password), &grants).await
        })
    }

    /// Ask `node` to let this phone open `service`, returning the petname it granted under.
    ///
    /// # Errors
    /// If the node is not paired, cannot be reached, no longer lists this phone, or refuses.
    pub fn allow(
        &self,
        node: &str,
        service: &str,
        username: &str,
        password: &str,
    ) -> anyhow::Result<String> {
        let (key, _) = paired(node)?;
        let me = self.key();
        let shared = Arc::clone(&self.shared);
        let petname = self.rt.block_on(async move {
            catalog::allow(&shared, key, &me, service, (username, password)).await
        })?;
        mirror_grant(node, service);
        Ok(petname)
    }

    /// Take the pairings that completed since the last call, draining the queue.
    #[must_use]
    pub fn take_pairings(&self) -> Vec<Paired> {
        std::mem::take(
            &mut *self
                .shared
                .pairings
                .lock()
                .unwrap_or_else(PoisonError::into_inner),
        )
    }

    /// Unpair `node`: drop its record, so nothing here can reach it again without a fresh pairing.
    ///
    /// The listeners already bound for it keep their ports but stop resolving — that is what the
    /// re-check in [`serve_service`] is for. The node also keeps *its* record of us until its
    /// operator revokes it; unpairing is local, like every other name decision in §2.
    ///
    /// # Errors
    /// Any error reading or writing the fleet registry.
    pub fn forget(&self, node: &str) -> anyhow::Result<bool> {
        let mut registry = FleetRegistry::load()?;
        let removed = registry.nodes.remove(node).is_some();
        if removed {
            registry.save()?;
            info!(%node, "unpaired");
        }
        Ok(removed)
    }

    /// Make good what a spell in the background may have cost. Called when the app returns to
    /// the foreground.
    ///
    /// iOS freezes the process, and two things do not survive that reliably:
    ///
    /// - **The pooled connections.** A QUIC connection that was alive when the app went away is
    ///   usually dead when it comes back — but it can still *look* open until an idle timeout
    ///   fires, and a request handed that connection stalls for the length of the timeout instead
    ///   of reconnecting. So the pool is retired and the next request dials fresh.
    /// - **The loopback listeners.** The system may take a suspended app's sockets back, and
    ///   Apple's own note on it (TN2277) is exact about what follows: *"your app will no longer be
    ///   listening for connections, even after it has been resumed. The app may or may not be
    ///   notified of this."* A listener is bound once per `(node, service)` and its port handed
    ///   out for the rest of the run, so one lost that way stayed lost until the app was killed —
    ///   every page and every chat on that node refused at the door, on any network. There is no
    ///   reliable way to ask a socket whether that happened to it, so every listener binds its
    ///   port again. Connections already accepted are not touched.
    ///
    /// It also tells iroh to look at the network again, which is free when nothing changed. What
    /// this does **not** do is replace the endpoint: that is [`reconnect`](Self::reconnect), for
    /// when the app was away long enough, or came back on a different network.
    pub fn resume(&self) {
        self.shared.listeners.send_modify(|epoch| *epoch += 1);
        if let Some(current) = self.shared.current() {
            self.shared.retire_pool(current.generation);
            // iroh watches the routing table itself, but a frozen process watches nothing, and on
            // iOS its wall-clock check for "was I asleep?" is switched off to save battery.
            self.rt
                .spawn(async move { current.endpoint.network_change().await });
        }
        debug!("back in the foreground: listeners re-bound, connection pool retired");
    }

    /// Look for a connection again, from the socket up: close the endpoint and bind a new one on
    /// the same identity. Returns at once; requests made meanwhile wait for the new endpoint.
    ///
    /// For when the network under the phone changed — a different Wi-Fi, Wi-Fi to cellular, a long
    /// spell in a pocket. See [`reconnect`] for why a fresh pool over the old endpoint is not
    /// enough then. The viewer also does this by itself when requests keep failing and no machine
    /// answers at all ([`heal`]), so a caller that never learns of a network change still recovers.
    pub fn reconnect(&self) {
        self.rt
            .spawn(reconnect(Arc::clone(&self.shared), "the app asked"));
    }

    /// How many times the transport has been replaced — a new pool or a new endpoint. Moves on
    /// every recovery step, which is what makes it useful to a test and to a log line.
    #[must_use]
    pub fn generation(&self) -> u64 {
        self.shared.current().map_or(0, |t| t.generation)
    }

    /// Stop the tasks, clear the published ticket, and close the endpoint.
    pub fn stop(self) {
        let _ = self.shared.shutdown.send(true);
        ticket::clear_published();
        let shared = Arc::clone(&self.shared);
        self.rt.block_on(async move {
            // Behind any reconnect in flight, so there is exactly one endpoint left to close.
            let _one = shared.reconnecting.lock().await;
            if let Some(last) = shared.transport.send_replace(None) {
                last.endpoint.close().await;
            }
        });
    }
}

// ---------------------------------------------------------------------------------------
// The tasks
// ---------------------------------------------------------------------------------------

/// Bind an endpoint on this phone's identity.
///
/// Everything it needs is read again on every call — the key, the relays, the resolvers — so the
/// endpoint a reconnect binds is configured exactly as the one the app launched with, including
/// the system's DNS servers as they are *now*, on the network the phone has moved to.
async fn bind_endpoint() -> anyhow::Result<Endpoint> {
    let secret = identity::load_or_create()?;
    let mut builder = Endpoint::builder(presets::N0)
        .secret_key(secret)
        // One ALPN, because a viewer accepts one thing. A phone serves no HTTP and no raw
        // forward, so answering those ALPNs would be advertising a door with no room
        // behind it — and `http:` grants a node might hold on us would then be reachable.
        .alpns(vec![join::ALPN.to_vec()]);
    // Honoured, but never load-bearing: a viewer reaches a node through *that node's* home
    // relay, which iroh dials whether or not it is in this map (it keeps an actor per relay
    // it needs, home or not). So a phone talks over the fleet's own relay the moment the
    // *nodes* are configured — this only decides where the phone itself would be reached,
    // and nothing dials a phone.
    let cfg = MeshConfig::load().unwrap_or_default();
    if let Some(mode) = relay::relay_mode(&cfg.relays) {
        info!(relays = ?cfg.relays, "adi-mesh viewer using configured relays");
        builder = builder.relay_mode(mode);
    }
    // Load-bearing here in a way the relay map is not: a phone that cannot resolve a name
    // cannot reach a node at all. See [`adi_mesh::dns`].
    builder = builder.dns_resolver(dns::resolver(&cfg.dns));
    builder.bind().await.context("binding the mesh endpoint")
}

/// Start the two tasks every endpoint has: publishing its ticket and accepting pairings.
///
/// Both end by themselves when their endpoint is closed, so a reconnect starts a new pair for the
/// new endpoint and never has to stop the old one.
fn attend(shared: &Arc<Shared>, transport: &Transport) {
    tokio::spawn(publish_ticket(
        transport.endpoint.clone(),
        transport.binding,
        Arc::clone(shared),
    ));
    tokio::spawn(accept_pairings(
        transport.endpoint.clone(),
        Arc::clone(shared),
        shared.shutdown.subscribe(),
    ));
}

/// Requests keep failing and nothing has got through for a while. Find out which of two things
/// that is, and reconnect only for the one a reconnect can fix.
///
/// **A machine that is off, and a connection that is lost, look the same from one request.** Both
/// are a dial that goes nowhere. They stop looking the same the moment a second machine is asked:
/// if one that answered earlier still does, the endpoint is carrying traffic and the trouble is
/// the first machine's own — asleep, unplugged, its network down. Replacing the endpoint then
/// would fix nothing and cost every other machine's connections, again and again for as long as
/// the app is open on a fleet with one laptop closed.
///
/// So a witness is asked first: the machine that answered most recently and has not failed
/// since ([`Shared::witness`]). Asking is one connection — a handshake, no request — which is
/// also exactly what the next request to that machine was going to need. If it answers, nothing
/// is replaced and the silence is counted from now. If it does not, or there is nobody to ask,
/// the failures are taken at their word.
async fn heal(shared: Arc<Shared>) {
    if let (Some(witness), Some(transport)) = (shared.witness(), shared.current()) {
        match timeout(WITNESS_TIMEOUT, transport.pool.get(witness)).await {
            Ok(Ok(_)) => {
                debug!(%witness, "another machine still answers; the endpoint is kept");
                // Not `succeeded`: the wait before the next look keeps growing until a request
                // that somebody actually made gets through.
                shared.answered().insert(witness, Instant::now());
                shared.health().last_ok = Instant::now();
                return;
            }
            Ok(Err(_)) | Err(_) => {
                shared.answered().remove(&witness);
            }
        }
    }
    reconnect(shared, "requests keep failing").await;
}

/// Look for a connection again, from the socket up: close the endpoint and bind a new one.
///
/// ## Why a new pool is not always enough
///
/// Retiring the pool makes the next request dial — over the *same endpoint*, with the same UDP
/// sockets, the same relay session and the same idea of what the network looks like. That is the
/// right answer to a connection that died. It is no answer when the endpoint itself is what went
/// stale, and on a phone there are three ways that happens which nothing inside iroh notices:
///
/// - **The system took the sockets back** while the app was frozen (TN2277). iroh's own recovery
///   re-binds a socket that answers `ENOTCONN` or `EPIPE`; Apple's note says the error is "in
///   many cases `EBADF`", which it treats as fatal for that socket and never re-binds.
/// - **The network changed and changed back** — or changed while the process could not watch.
///   iroh decides whether anything happened by comparing interface lists, so the same Wi-Fi after
///   a detour through cellular compares equal and is ignored, NAT mappings and relay session
///   notwithstanding; and its "did I just wake up?" check is disabled on iOS to save battery.
/// - **Its in-place repair did not take.** Seen on a Mac on 2026-10-20: after a move between two
///   networks the endpoint kept a connection on the relay that a freshly bound one, on the same
///   network, replaced with a direct path in half a second (`adi_mesh::roam`).
///
/// A new endpoint has none of that history: fresh sockets, a fresh relay session, the resolvers
/// of the network the phone is on now. It is what restarting the app does, without restarting it.
///
/// ## What it costs
///
/// Every stream on the old endpoint ends, so a page with a WebSocket open reconnects it. That is
/// why this is not done on every return to the foreground (the app decides — see `FleetModel`),
/// and why the viewer's own trigger is as reluctant as [`reconnect_is_due`] and [`heal`] make it. The
/// loopback ports are untouched: a page keeps its origin, and a request that arrives during the
/// swap waits for the new endpoint instead of being refused.
///
/// **The old endpoint is closed before the new one is bound**, never the other way round: two
/// endpoints alive on one key race each other for the same relay session.
async fn reconnect(shared: Arc<Shared>, why: &'static str) {
    let asked = Instant::now();
    let _one = shared.reconnecting.lock().await;
    if shared.stopping() {
        return;
    }
    {
        let mut health = shared.health();
        // Asked for while another reconnect was waiting its turn, which then began after this
        // ask and so has already seen whatever prompted it.
        let covered = health.reconnect_began.is_some_and(|began| began >= asked);
        // Or the same event again through a second channel. See [`RECONNECT_QUIET`]. Measured
        // from ask to ask, so that how long the last reconnect took does not stretch the window.
        let repeated = health
            .reconnect_asked
            .is_some_and(|last| asked.saturating_duration_since(last) < RECONNECT_QUIET);
        if covered || repeated {
            debug!(why, "reconnect: already done for this");
            return;
        }
        health.reconnect_asked = Some(asked);
        health.reconnect_began = Some(Instant::now());
    }
    info!(why, "reconnecting: replacing the mesh endpoint");

    // `None` first, so a request that fails on the old endpoint from here on knows it was
    // replaced and waits for the new one.
    if let Some(old) = shared.transport.send_replace(None)
        && timeout(CLOSE_WAIT, old.endpoint.close()).await.is_err()
    {
        debug!("reconnect: the old endpoint did not finish closing in time; dropping it");
    }

    let mut backoff = REBIND_BACKOFF_BASE;
    let endpoint = loop {
        if shared.stopping() {
            return;
        }
        match shared.origin.bind().await {
            Ok(endpoint) => break endpoint,
            // Not given up on: with no endpoint nothing works at all, and the requests waiting
            // on this one each have their own timeout to report through.
            Err(e) => {
                warn!(error = %e, "reconnect: could not bind a new endpoint; trying again");
                tokio::time::sleep(backoff).await;
                backoff = backoff.saturating_mul(2).min(REBIND_BACKOFF_CAP);
            }
        }
    };
    if shared.stopping() {
        endpoint.close().await;
        return;
    }

    let transport = shared.install(endpoint);
    if shared.origin.attended() {
        attend(&shared, &transport);
    }
    let mut health = shared.health();
    // The new endpoint is judged from now, not from the failures that asked for it.
    health.failures = 0;
    health.last_ok = Instant::now();
    info!(
        binding = transport.binding,
        "reconnected: a new mesh endpoint is bound"
    );
}

/// Wait for a relay, then record and publish this run's ticket.
async fn publish_ticket(endpoint: Endpoint, binding: u64, shared: Arc<Shared>) {
    let deadline = tokio::time::Instant::now() + RELAY_WAIT;
    let addr = loop {
        let addr = endpoint.addr();
        if ticket::has_relay(&addr) || tokio::time::Instant::now() >= deadline {
            break addr;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    };
    // A reconnect replaced this endpoint while its relay was still coming up: the ticket to
    // publish is the new endpoint's, and its own copy of this task will.
    if shared.rebound(binding) {
        return;
    }
    match ticket::encode(&addr) {
        Ok(token) => {
            // Published as well as held: `join`'s invite book and this token are read back through
            // the same store, so a future entry point that expects a published ticket finds one.
            if let Err(e) = ticket::publish(&token) {
                warn!(error = %e, "could not persist this phone's ticket");
            }
            *shared.ticket.lock().unwrap_or_else(PoisonError::into_inner) = Some(token);
            info!("mesh relay session up; this phone can be paired");
        }
        Err(e) => warn!(error = %e, "could not encode this phone's ticket"),
    }
}

/// Accept inbound connections. A viewer answers pairings and nothing else.
async fn accept_pairings(
    endpoint: Endpoint,
    shared: Arc<Shared>,
    mut shutdown: watch::Receiver<bool>,
) {
    loop {
        tokio::select! {
            _ = shutdown.changed() => {
                info!("viewer: stopping accept loop");
                return;
            }
            incoming = endpoint.accept() => {
                let Some(incoming) = incoming else {
                    debug!("viewer: endpoint closed; accept loop ending");
                    return;
                };
                let shared = Arc::clone(&shared);
                tokio::spawn(async move {
                    match incoming.await {
                        Ok(conn) if conn.alpn() == join::ALPN => {
                            join::serve_join_with(conn, |accepted| {
                                shared
                                    .pairings
                                    .lock()
                                    .unwrap_or_else(PoisonError::into_inner)
                                    .push(Paired {
                                        petname: accepted.petname.clone(),
                                        username: accepted.username.clone(),
                                        password: accepted.password.clone(),
                                    });
                            })
                            .await;
                        }
                        // Every other ALPN: refused by closing. A phone hosts nothing, and saying
                        // so immediately is better than a handshake that leads nowhere.
                        Ok(conn) => {
                            debug!(alpn = ?conn.alpn(), "viewer: refusing an ALPN a phone does not serve");
                            conn.close(0u32.into(), b"this peer serves nothing");
                        }
                        Err(e) => debug!(error = %e, "viewer: inbound connection failed to establish"),
                    }
                });
            }
        }
    }
}

/// Serve one `(node, service)` pair on its loopback port until shutdown.
///
/// The listener is not assumed to last. It is bound again, on the same port, whenever the app
/// comes back from the background ([`Viewer::resume`]) and whenever accepting on it fails in a way
/// that says the socket is gone — because the port is the page's origin and is never handed out
/// twice, so a listener that quietly stopped listening took its service with it for the rest of
/// the run.
///
/// Not an `async fn`, so that it starts watching for "bind again" the moment the listener is
/// handed over rather than when the task is first polled — a request to bind again that landed in
/// between would otherwise be one it never heard.
fn serve_service(
    mut listener: TcpListener,
    port: u16,
    shared: Arc<Shared>,
    key: EndpointId,
    service: String,
) -> impl Future<Output = ()> + Send {
    let mut shutdown = shared.shutdown.subscribe();
    let mut rebind = shared.listeners.subscribe();
    async move {
        loop {
            let lost = tokio::select! {
                _ = shutdown.changed() => return,
                asked = rebind.changed() => {
                    if asked.is_err() {
                        return;
                    }
                    true
                }
                accepted = listener.accept() => match accepted {
                    Ok((tcp, _)) => {
                        let shared = Arc::clone(&shared);
                        let service = service.clone();
                        tokio::spawn(async move {
                            if let Err(e) = proxy_one(tcp, &shared, key, &service).await {
                                debug!(%service, error = %e, "request ended with an error");
                            }
                        });
                        false
                    }
                    Err(e) if accept_error_is_passing(&e) => {
                        debug!(%service, error = %e, "accept failed; the listener is kept");
                        tokio::time::sleep(ACCEPT_ERROR_BACKOFF).await;
                        false
                    }
                    Err(e) => {
                        warn!(%service, port, error = %e, "the listener is gone; binding the port again");
                        true
                    }
                }
            };
            if lost {
                // Dropped before the bind, not after: two listeners cannot hold one port.
                drop(listener);
                let Some(fresh) = bind_again(port, &service, &mut shutdown).await else {
                    return;
                };
                listener = fresh;
                let rebound = shared.rebound.fetch_add(1, Ordering::Relaxed) + 1;
                debug!(%service, port, rebound, "listening on the port again");
            }
        }
    }
}

/// An accept error that says something about one connection, or about a moment, and nothing
/// about the listening socket: the client hung up first, a signal landed, memory was short.
/// Anything else — and after a suspension that is `EBADF`, or whatever the system chooses to say
/// about a socket it took back — means the listener is not coming back by itself.
fn accept_error_is_passing(error: &std::io::Error) -> bool {
    use std::io::ErrorKind::{
        ConnectionAborted, ConnectionReset, Interrupted, OutOfMemory, WouldBlock,
    };
    matches!(
        error.kind(),
        ConnectionAborted | ConnectionReset | Interrupted | OutOfMemory | WouldBlock
    )
}

/// Bind `port` again, for as long as it takes. `None` only when the viewer is stopping.
///
/// The same port and no other: it is the origin of whatever page is open on it, and of every URL
/// the app has already built. So this does not fall back to a fresh one the way a first bind does
/// ([`ports::bind_stable`]) — a moved port would be a service nobody is pointing at.
async fn bind_again(
    port: u16,
    service: &str,
    shutdown: &mut watch::Receiver<bool>,
) -> Option<TcpListener> {
    let mut backoff = REBIND_BACKOFF_BASE;
    let mut said = false;
    loop {
        match TcpListener::bind(loopback(port)).await {
            Ok(listener) => return Some(listener),
            Err(e) => {
                if !said {
                    warn!(%service, port, error = %e, "could not bind the port again yet; retrying");
                    said = true;
                }
            }
        }
        tokio::select! {
            _ = shutdown.changed() => return None,
            () = tokio::time::sleep(backoff) => {}
        }
        backoff = backoff.saturating_mul(2).min(REBIND_BACKOFF_CAP);
    }
}

/// A stream to one service on one node, with the node's answer to "may I?" already read.
struct Opened {
    send: SendStream,
    recv: RecvStream,
    status: HttpStatus,
    /// The transport it was opened over, for charging a later failure to the right one.
    generation: u64,
}

/// Why a node could not be asked at all. A node that *refuses* is not this: that is an answer.
#[derive(Debug)]
enum Unreachable {
    /// Dialling it, or opening the stream, failed outright.
    Failed(anyhow::Error),
    /// Nothing came back within [`STEP_TIMEOUT`].
    TimedOut,
}

/// Open a stream to `service` on the node `key` and read whether the node admits it.
///
/// Everything a request does before its first byte of HTTP, in one place, because both ways of
/// talking to a node — a browser's connection ([`proxy_one`]) and the app's own questions to the
/// control panel ([`catalog`]) — need the same three things from it:
///
/// - **A bound.** The whole of it fits in [`STEP_TIMEOUT`], however many attempts that is.
/// - **Another go when the ground moved.** If the endpoint was replaced by a [`reconnect`] while
///   an attempt was in flight, the failure belongs to the old one. Nothing has been sent to the
///   node's service yet and nothing has been answered to the caller, so the attempt simply starts
///   over on the new endpoint, waiting for it if it is still being bound. This is what turns "the
///   phone changed network mid-request" from an error page into a request that took a second
///   longer.
/// - **The bookkeeping.** A round trip counts for the transport's health and a failure against
///   it; see [`Shared::failed`] for what enough failures lead to.
async fn open_stream(
    shared: &Arc<Shared>,
    key: EndpointId,
    service: &str,
) -> Result<Opened, Unreachable> {
    // Which transport the attempt in flight is on. Outside the future below so that it can still
    // be read when the timeout drops that future mid-attempt. Zero is no transport at all.
    let used = AtomicU64::new(0);
    let attempts = async {
        let mut again = REPLACED_RETRIES;
        loop {
            let transport = shared.transport().await?;
            let generation = transport.generation;
            used.store(generation, Ordering::Relaxed);
            let attempt = async {
                let conn = transport.pool.get(key).await?;
                let (mut send, mut recv) = conn.open_bi().await?;
                protocol::write_http_request(&mut send, service).await?;
                let status = protocol::read_http_status(&mut recv).await?;
                anyhow::Ok((send, recv, status))
            };
            match attempt.await {
                Ok((send, recv, status)) => {
                    return anyhow::Ok(Opened {
                        send,
                        recv,
                        status,
                        generation,
                    });
                }
                Err(e) if again > 0 && shared.rebound(transport.binding) => {
                    again -= 1;
                    debug!(%service, error = %e, "the endpoint was replaced mid-request; going again");
                }
                Err(e) => return Err(e),
            }
        }
    };
    match timeout(STEP_TIMEOUT, attempts).await {
        Ok(Ok(opened)) => {
            shared.succeeded(key);
            Ok(opened)
        }
        // A failure *or* a timeout means the pooled connection cannot be trusted: after the app is
        // suspended every one of them is dead while still reporting itself usable, so leaving them
        // in place would make the next request hang exactly as this one did.
        Ok(Err(e)) => {
            shared.failed(key, used.load(Ordering::Relaxed));
            Err(Unreachable::Failed(e))
        }
        Err(_) => {
            shared.failed(key, used.load(Ordering::Relaxed));
            Err(Unreachable::TimedOut)
        }
    }
}

/// One browser connection: a pooled stream to the node, the service label, then raw bytes.
///
/// Note what is *missing* compared to the front door's gateway: it reads the request head to find a
/// `Host` to route on, and must then forward those bytes verbatim. Here the target was fixed when
/// the listener was bound, so not one byte of the request is parsed — the browser's bytes go
/// straight through, which is also why a WebSocket upgrade needs no special case.
async fn proxy_one(
    mut tcp: TcpStream,
    shared: &Arc<Shared>,
    key: EndpointId,
    service: &str,
) -> anyhow::Result<()> {
    // Everything up to the first byte of HTTP is bounded (see [`STEP_TIMEOUT`]); the splice that
    // follows is not, because by then the two ends are talking and a long-lived stream — a
    // WebSocket, a log tail — is the point rather than a stall.
    let opened = match open_stream(shared, key, service).await {
        Ok(opened) => opened,
        Err(Unreachable::Failed(e)) => {
            warn!(%service, error = %e, "cannot reach the node");
            return gateway_error(
                &mut tcp,
                "This node is not reachable right now.",
                &format!("{e:#}"),
                Retry::Soon,
            )
            .await;
        }
        Err(Unreachable::TimedOut) => {
            warn!(%service, "timed out reaching the node");
            return gateway_error(
                &mut tcp,
                "This node did not answer.",
                "The connection timed out.",
                Retry::Soon,
            )
            .await;
        }
    };

    match opened.status {
        HttpStatus::Ok => {
            let _live = LiveSplice::begin(&shared.splices);
            tunnel::splice(tcp, opened.send, opened.recv).await;
            Ok(())
        }
        refused => {
            info!(%service, reason = refused.reason(), "node refused");
            gateway_error(
                &mut tcp,
                "The node refused this service.",
                refused.reason(),
                Retry::Never,
            )
            .await
        }
    }
}

/// Whether the page that reports a failure should ask again by itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Retry {
    /// The node could not be reached. That passes — the phone finds a network, the machine wakes —
    /// and a page that needed a person to reload it would sit there after it had.
    Soon,
    /// The node answered, and the answer was no. Asking again changes nothing.
    Never,
}

/// How long the error page waits before it loads itself again.
const RETRY_PAGE_AFTER: Duration = Duration::from_secs(4);

/// Answer the browser with a `502` when the mesh could not carry the request.
///
/// Plain and small on purpose: this page is read on a phone, by whoever just tapped a service, and
/// its whole job is to distinguish "the node is off" from "you may not have this".
///
/// With [`Retry::Soon`] it reloads itself, which is the only thing that can: the web view showing
/// it has no reload control, and nothing outside the page knows it is an error page — the app sees
/// a navigation that finished. The refresh only means anything where this is the document; as the
/// body of a failed `fetch` it is text nobody renders.
async fn gateway_error(
    tcp: &mut TcpStream,
    headline: &str,
    detail: &str,
    retry: Retry,
) -> anyhow::Result<()> {
    let body = error_page(headline, detail, retry);
    let head = format!(
        "HTTP/1.1 502 Bad Gateway\r\nContent-Type: text/html; charset=utf-8\r\n\
         Cache-Control: no-store\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    tcp.write_all(head.as_bytes()).await?;
    tcp.write_all(body.as_bytes()).await?;
    tcp.shutdown().await?;
    Ok(())
}

/// The body of [`gateway_error`]'s page.
fn error_page(headline: &str, detail: &str, retry: Retry) -> String {
    let (refresh, trying) = match retry {
        Retry::Soon => (
            format!(
                "<meta http-equiv=refresh content={}>",
                RETRY_PAGE_AFTER.as_secs()
            ),
            "<p class=again>Trying again in a moment…</p>",
        ),
        Retry::Never => (String::new(), ""),
    };
    format!(
        "<!doctype html><meta charset=utf-8>\
         <meta name=viewport content=\"width=device-width,initial-scale=1\">{refresh}\
         <title>Unreachable</title>\
         <style>body{{font:-apple-system-body,system-ui,sans-serif;margin:0;padding:2rem;\
         color:#111;background:#fff}}h1{{font-size:1.25rem;margin:0 0 .5rem}}\
         p{{color:#666;margin:0;line-height:1.5}}.again{{margin-top:1rem}}\
         @media(prefers-color-scheme:dark){{body{{color:#eee;background:#000}}p{{color:#999}}}}</style>\
         <h1>{headline}</h1><p>{detail}</p>{trying}",
        detail = escape_html(detail),
    )
}

/// `detail` is an error's own words, and an error can quote anything — a hostname, a header.
fn escape_html(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

// ---------------------------------------------------------------------------------------
// Grants → a service list
// ---------------------------------------------------------------------------------------

/// A paired node's key, and this phone's own record of what it was granted there.
///
/// Read fresh from the registry on every call rather than cached: a node forgotten between two
/// taps must stop being reachable at the next one, not at the next launch.
fn paired(node: &str) -> anyhow::Result<(EndpointId, Vec<Grant>)> {
    let registry = FleetRegistry::load()?;
    let record = registry
        .get(node)
        .with_context(|| format!("no node called {node:?} is paired with this phone"))?;
    let key = record
        .endpoint_id()
        .with_context(|| format!("the key recorded for {node:?} is not a valid endpoint id"))?;
    Ok((key, record.grants.clone()))
}

/// Record on this side the grant a node has just added for this phone.
///
/// Pairing writes the same rule into both registries (`join.rs`), and this keeps that property
/// true for a grant added afterwards — otherwise the node would happily serve a dashboard that
/// this phone's own node list never mentions, and `http:*` would be the only way to see one.
///
/// It is a mirror and never a source: [`Viewer::open`] does not consult it, and the node decides
/// every request on its own copy. So a failure to write it is logged and
/// nothing more — the dashboard still opens.
fn mirror_grant(node: &str, service: &str) {
    let mut registry = match FleetRegistry::load() {
        Ok(registry) => registry,
        Err(e) => {
            warn!(error = %e, "could not mirror the new grant locally");
            return;
        }
    };
    let Some(record) = registry.get_mut(node) else {
        return;
    };
    if !record.grant(Grant::Http(Scope::One(service.to_string()))) {
        return; // Already held here; nothing to write.
    }
    if let Err(e) = registry.save() {
        warn!(error = %e, "could not save the mirrored grant");
    }
}

/// The service labels a grant list names, and whether it also allows any label at all.
///
/// The protocol has no "list your services" call, so this is everything a viewer can say about a
/// node from its own registry. `http:*` therefore returns `any_service`, and the UI turns that
/// into a field a human types into rather than a list it pretends to know.
///
/// Dashboards are the one thing that *is* enumerable, and not from here: [`catalog`] asks the
/// node's control panel, which knows them by name.
fn grantable_services(grants: &[Grant]) -> (Vec<String>, bool) {
    let mut named = Vec::new();
    let mut any = false;
    for grant in grants {
        match grant {
            Grant::Http(Scope::One(label)) => {
                if !named.iter().any(|held| held == label) {
                    named.push(label.clone());
                }
            }
            Grant::Http(Scope::Any) => any = true,
            // `tcp:` and `ctl:` are not things a browser can open, so they are not offered as one.
            Grant::Tcp(_) | Grant::Ctl(_) => {}
        }
    }
    named.sort();
    (named, any)
}

/// The address family every listener binds on: loopback, inside the app sandbox.
const fn loopback(port: u16) -> SocketAddr {
    SocketAddr::new(std::net::IpAddr::V4(Ipv4Addr::LOCALHOST), port)
}

#[cfg(test)]
mod tests {
    use std::future::Future;
    use std::pin::Pin;

    use iroh::address_lookup::memory::MemoryLookup;
    use iroh::{EndpointAddr, RelayMode, SecretKey};
    use tokio::io::AsyncReadExt as _;

    use super::*;

    /// What [`Origin::Given`] hands back: an endpoint being bound.
    pub(super) type Binding = Pin<Box<dyn Future<Output = anyhow::Result<Endpoint>> + Send>>;

    // -----------------------------------------------------------------------------------
    // A machine made up on the spot, and a viewer that knows where it is
    // -----------------------------------------------------------------------------------

    /// A node that admits every request and answers it with `ok`. No relay and no discovery: it
    /// is reached at the loopback address in [`at`], which is all of the network these tests use.
    async fn node() -> Endpoint {
        let endpoint = Endpoint::builder(presets::Minimal)
            .relay_mode(RelayMode::Disabled)
            .alpns(vec![protocol::HTTP_ALPN.to_vec()])
            .bind_addr(loopback(0))
            .expect("a loopback address")
            .bind()
            .await
            .expect("the node binds");
        let accepting = endpoint.clone();
        tokio::spawn(async move {
            while let Some(incoming) = accepting.accept().await {
                tokio::spawn(async move {
                    let Ok(conn) = incoming.await else { return };
                    while let Ok((mut send, mut recv)) = conn.accept_bi().await {
                        tokio::spawn(async move {
                            if protocol::read_http_request(&mut recv).await.is_err() {
                                return;
                            }
                            let _ = protocol::write_http_status(&mut send, HttpStatus::Ok).await;
                            // The request head, to its blank line — then the whole of the answer.
                            let mut head = Vec::new();
                            let mut byte = [0u8; 1];
                            while !head.ends_with(b"\r\n\r\n") {
                                if recv.read_exact(&mut byte).await.is_err() {
                                    return;
                                }
                                head.push(byte[0]);
                            }
                            let _ = send
                                .write_all(
                                    b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\
                                      Connection: close\r\n\r\nok",
                                )
                                .await;
                            let _ = send.finish();
                        });
                    }
                });
            }
        });
        endpoint
    }

    /// Where `node` is listening, as an address a viewer can be told.
    fn at(node: &Endpoint) -> EndpointAddr {
        let port = node
            .bound_sockets()
            .iter()
            .find(|addr| addr.is_ipv4())
            .expect("the node is bound on IPv4")
            .port();
        EndpointAddr::new(node.id()).with_ip_addr(loopback(port))
    }

    /// The node's key at an address where nothing answers: a dial to it neither fails nor
    /// succeeds, which is what a path that has stopped existing looks like.
    fn nowhere(node: &Endpoint) -> EndpointAddr {
        EndpointAddr::new(node.id()).with_ip_addr(loopback(9))
    }

    /// A viewer whose every endpoint — the first, and each one a reconnect binds — is bound on
    /// one key and told what `knows` says the fleet looks like at that moment.
    fn viewer(knows: impl Fn() -> Vec<EndpointAddr> + Send + Sync + 'static) -> Arc<Shared> {
        let secret = SecretKey::generate();
        Arc::new(Shared::new(Origin::Given(Box::new(move || {
            let secret = secret.clone();
            let fleet = knows();
            Box::pin(async move {
                Endpoint::builder(presets::Minimal)
                    .secret_key(secret)
                    .relay_mode(RelayMode::Disabled)
                    .address_lookup(MemoryLookup::from_endpoint_info(fleet))
                    .bind()
                    .await
                    .context("binding a test endpoint")
            })
        }))))
    }

    /// Bind the viewer's first endpoint, as [`Viewer::start`] does.
    async fn started(shared: &Arc<Shared>) -> Arc<Transport> {
        shared.install(shared.origin.bind().await.expect("the viewer binds"))
    }

    /// Serve `node`'s `app` on a loopback port, as [`Viewer::open`] does. Returns the port.
    async fn opened(shared: &Arc<Shared>, node: &Endpoint) -> u16 {
        let listener = TcpListener::bind(loopback(0)).await.expect("a port");
        let port = listener.local_addr().expect("its address").port();
        tokio::spawn(serve_service(
            listener,
            port,
            Arc::clone(shared),
            node.id(),
            "app".to_string(),
        ));
        port
    }

    /// One request through the loopback port, the way a browser makes it. The whole reply.
    async fn get(port: u16) -> String {
        let mut tcp = TcpStream::connect(loopback(port))
            .await
            .expect("the port accepts");
        tcp.write_all(b"GET / HTTP/1.1\r\nHost: phone\r\n\r\n")
            .await
            .expect("the request is written");
        let mut reply = String::new();
        tcp.read_to_string(&mut reply)
            .await
            .expect("the reply is read");
        reply
    }

    /// `body`, or a failure that names what never happened instead of hanging the test run.
    async fn soon<T>(what: &str, body: impl Future<Output = T>) -> T {
        timeout(Duration::from_secs(10), body)
            .await
            .unwrap_or_else(|_| panic!("timed out waiting for {what}"))
    }

    /// A reconnect, run to its end. Boxed: it is a large future, and the real callers spawn it.
    async fn reconnected(shared: &Arc<Shared>, why: &'static str) {
        soon(
            "the reconnect",
            Box::pin(reconnect(Arc::clone(shared), why)),
        )
        .await;
    }

    // -----------------------------------------------------------------------------------
    // Reconnecting
    // -----------------------------------------------------------------------------------

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_reconnect_replaces_the_endpoint_and_the_same_port_keeps_working() {
        let node = node().await;
        let addr = at(&node);
        let shared = viewer(move || vec![addr.clone()]);
        let first = started(&shared).await;
        let port = opened(&shared, &node).await;

        assert!(soon("the first answer", get(port)).await.ends_with("ok"));

        reconnected(&shared, "a test").await;
        let second = shared.current().expect("a transport after the reconnect");
        assert!(
            first.endpoint.is_closed(),
            "the old endpoint must be closed, not left racing the new one for the relay"
        );
        assert!(!second.endpoint.is_closed());
        assert_eq!(
            first.endpoint.id(),
            second.endpoint.id(),
            "a reconnect is the same phone: the key the nodes authorized must not change"
        );
        assert!(shared.rebound(first.binding));
        assert_eq!(second.binding, first.binding + 1);

        // The page's origin did not move, and what is behind it works again.
        assert!(soon("the second answer", get(port)).await.ends_with("ok"));
        assert_eq!(shared.health().failures, 0);
    }

    /// The case the whole change exists for. A request is in flight on a path that has stopped
    /// existing; the phone finds a network again; the request is answered — late, and not with
    /// an error page that someone has to reload.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_request_in_flight_is_carried_by_the_endpoint_that_replaces_its_own() {
        let node = node().await;
        let (lost, found) = (nowhere(&node), at(&node));
        // The first endpoint is told an address nothing answers at; every later one, the real one.
        let binds = AtomicUsize::new(0);
        let shared = viewer(move || {
            if binds.fetch_add(1, Ordering::Relaxed) == 0 {
                vec![lost.clone()]
            } else {
                vec![found.clone()]
            }
        });
        started(&shared).await;
        let port = opened(&shared, &node).await;

        let began = Instant::now();
        let request = tokio::spawn(get(port));
        // Long enough to be dialling into the void, well short of the step timeout.
        tokio::time::sleep(Duration::from_millis(400)).await;
        assert!(!request.is_finished(), "nothing answers at that address");

        reconnected(&shared, "a test").await;
        let reply = soon("the answer", request).await.expect("the request task");
        assert!(
            reply.ends_with("ok"),
            "the request should have been answered over the new endpoint, got: {reply}"
        );
        assert!(
            began.elapsed() < STEP_TIMEOUT,
            "it was answered because it was carried over, not because it timed out and was retried"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn the_same_event_reported_twice_is_one_reconnect() {
        let node = node().await;
        let addr = at(&node);
        let shared = viewer(move || vec![addr.clone()]);
        let first = started(&shared).await;

        // The scene phase and the path monitor, a moment apart.
        tokio::join!(
            reconnected(&shared, "the scene phase"),
            reconnected(&shared, "the path monitor"),
        );
        let second = shared.current().expect("a transport");
        assert_eq!(
            second.generation,
            first.generation + 1,
            "the second report must not throw away what the first one built"
        );

        reconnected(&shared, "and again").await;
        assert_eq!(
            shared.current().expect("a transport").generation,
            second.generation,
            "nor one that arrives just after it finished"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn enough_failures_with_nothing_getting_through_reconnect_unasked() {
        let node = node().await;
        let addr = at(&node);
        let shared = viewer(move || vec![addr.clone()]);
        let first = started(&shared).await;

        // One failure after a long silence: a blip. The pool goes, the endpoint stays.
        shared.health().last_ok = Instant::now() - SILENCE_BEFORE_RECONNECT * 2;
        shared.failed(node.id(), first.generation);
        let after_one = shared.current().expect("a transport");
        assert_eq!(after_one.generation, first.generation + 1, "a new pool");
        assert!(!shared.rebound(first.binding), "on the same endpoint");

        // A second, with still nothing in between and nobody else to ask: the viewer goes
        // looking for a connection.
        shared.failed(node.id(), after_one.generation);
        soon("the endpoint to be replaced", async {
            while !first.endpoint.is_closed() || shared.current().is_none() {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await;
        assert!(shared.rebound(first.binding));
        let health = shared.health();
        assert_eq!(
            health.failures, 0,
            "the new endpoint starts with a clean sheet"
        );
        assert_eq!(
            health.backoff,
            AUTO_RECONNECT_BASE * 2,
            "and the next unasked reconnect has to wait longer than this one did"
        );
    }

    /// One machine asleep is not a lost connection, and must not be treated as one for as long
    /// as another still answers — which is every fleet with a laptop in it, most of the day.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_machine_that_still_answers_keeps_the_endpoint_when_another_does_not() {
        let (awake, asleep) = (node().await, node().await);
        let (here, gone) = (at(&awake), nowhere(&asleep));
        let shared = viewer(move || vec![here.clone(), gone.clone()]);
        let first = started(&shared).await;

        // The one that is awake answers a request, as it did when the app listed its apps.
        let opened = soon("the first answer", open_stream(&shared, awake.id(), "app")).await;
        assert!(opened.is_ok(), "the machine that is awake answers");
        assert_eq!(shared.witness(), Some(awake.id()));
        drop(opened);

        // The other keeps failing, long enough to look like a lost connection.
        shared.health().last_ok = Instant::now() - SILENCE_BEFORE_RECONNECT * 2;
        shared.health().failures = FAILURES_BEFORE_RECONNECT;
        soon(
            "the question to the witness",
            Box::pin(heal(Arc::clone(&shared))),
        )
        .await;

        assert!(
            !shared.rebound(first.binding),
            "the endpoint is carrying traffic; replacing it would fix nothing and cut everything"
        );
        assert!(!first.endpoint.is_closed());
        assert!(
            shared.health().last_ok.elapsed() < Duration::from_secs(5),
            "and the silence is counted from the witness's answer"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_witness_that_has_stopped_answering_too_is_a_lost_connection() {
        let node = node().await;
        let addr = at(&node);
        let shared = viewer(move || vec![addr.clone()]);
        let first = started(&shared).await;
        let opened = soon("the first answer", open_stream(&shared, node.id(), "app")).await;
        assert!(opened.is_ok());
        drop(opened);
        assert_eq!(shared.witness(), Some(node.id()));

        // The machine goes away. Nobody has asked it anything since, so it is still the witness.
        node.close().await;
        soon(
            "the witness to be given up on, and the endpoint replaced",
            Box::pin(heal(Arc::clone(&shared))),
        )
        .await;

        assert!(shared.rebound(first.binding));
        assert_eq!(
            shared.witness(),
            None,
            "a machine that did not answer cannot vouch for the next endpoint either"
        );
    }

    #[test]
    fn a_reconnect_is_not_ordered_over_something_ordinary() {
        let now = Instant::now();
        let quiet = |failures, since_ok: Duration, next_auto: Instant| Health {
            failures,
            last_ok: now - since_ok,
            next_auto,
            backoff: AUTO_RECONNECT_BASE,
            reconnect_asked: None,
            reconnect_began: None,
        };
        let long = SILENCE_BEFORE_RECONNECT * 2;

        assert!(reconnect_is_due(&quiet(2, long, now), now, 0));
        assert!(
            !reconnect_is_due(&quiet(1, long, now), now, 0),
            "one failure is a blip"
        );
        assert!(
            !reconnect_is_due(&quiet(9, Duration::from_secs(3), now), now, 0),
            "something answered a moment ago: one node is asleep, the connection is not lost"
        );
        assert!(
            !reconnect_is_due(&quiet(9, long, now), now, 1),
            "a page is still being carried: its socket is not to be cut for a quiet neighbour"
        );
        assert!(
            !reconnect_is_due(&quiet(9, long, now + Duration::from_secs(1)), now, 0),
            "the last unasked reconnect was too recent"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_pool_is_retired_once_per_failure_batch_not_once_per_failure() {
        let node = node().await;
        let addr = at(&node);
        let shared = viewer(move || vec![addr.clone()]);
        let first = started(&shared).await;

        // Three requests that were all waiting on the same dead connection time out together.
        assert!(shared.retire_pool(first.generation));
        assert!(!shared.retire_pool(first.generation));
        assert!(!shared.retire_pool(first.generation));
        let current = shared.current().expect("a transport");
        assert_eq!(
            current.generation,
            first.generation + 1,
            "the second and third must not retire the pool the first one's retry dials into"
        );
        assert!(
            !shared.rebound(first.binding),
            "and a new pool is not a new endpoint: nothing in flight starts over for it"
        );
    }

    // -----------------------------------------------------------------------------------
    // The loopback listeners
    // -----------------------------------------------------------------------------------

    /// What [`Viewer::resume`] does to a listener, with a page's connection still open on the
    /// port — which is the state it is always in when the app comes back.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_listener_is_bound_again_on_its_own_port_under_an_open_connection() {
        let node = node().await;
        let addr = at(&node);
        // Never started: a request just waits, which keeps its connection open for the test.
        let shared = viewer(move || vec![addr.clone()]);
        let port = opened(&shared, &node).await;

        let held = TcpStream::connect(loopback(port))
            .await
            .expect("the port accepts");
        for round in 1..=3 {
            shared.listeners.send_modify(|epoch| *epoch += 1);
            soon("the listener to be bound again", async {
                while shared.rebound.load(Ordering::Relaxed) < round {
                    tokio::time::sleep(Duration::from_millis(5)).await;
                }
            })
            .await;
            // A different socket now, on the same port — which the connection still open from
            // before did not stop it from taking.
            TcpStream::connect(loopback(port))
                .await
                .expect("the port accepts again");
        }
        drop(held);
    }

    #[tokio::test]
    async fn binding_again_waits_for_its_port_rather_than_taking_another() {
        let holder = TcpListener::bind(loopback(0)).await.expect("a port");
        let port = holder.local_addr().expect("its address").port();
        let stop = watch::Sender::new(false);
        let mut stopping = stop.subscribe();

        let waiting = tokio::spawn(async move { bind_again(port, "app", &mut stopping).await });
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert!(!waiting.is_finished(), "the port is still held");

        drop(holder);
        let fresh = soon("the bind", waiting)
            .await
            .expect("the bind task")
            .expect("a listener, since nothing was stopped");
        assert_eq!(
            fresh.local_addr().expect("its address").port(),
            port,
            "a moved port is a service nobody is pointing at"
        );
    }

    #[test]
    fn only_an_error_about_the_listener_itself_costs_the_listener() {
        use std::io::{Error, ErrorKind};
        assert!(accept_error_is_passing(&Error::from(
            ErrorKind::ConnectionAborted
        )));
        assert!(accept_error_is_passing(&Error::from(
            ErrorKind::Interrupted
        )));
        // `EBADF`: what Apple's note says a socket the system took back mostly answers with.
        assert!(!accept_error_is_passing(&Error::from_raw_os_error(9)));
        assert!(!accept_error_is_passing(&Error::from(
            ErrorKind::NotConnected
        )));
        assert!(!accept_error_is_passing(&Error::from(
            ErrorKind::InvalidInput
        )));
    }

    // -----------------------------------------------------------------------------------
    // The page that says a node could not be reached
    // -----------------------------------------------------------------------------------

    #[test]
    fn an_unreachable_node_s_page_asks_again_and_a_refusal_does_not() {
        let unreachable = error_page("This node did not answer.", "timed out", Retry::Soon);
        assert!(unreachable.contains("http-equiv=refresh"));
        assert!(unreachable.contains("Trying again"));

        let refused = error_page("The node refused this service.", "no grant", Retry::Never);
        assert!(
            !refused.contains("http-equiv=refresh"),
            "a refusal is an answer, and asking for it every four seconds changes nothing"
        );
    }

    #[test]
    fn an_error_s_own_words_cannot_write_markup_into_the_page() {
        let page = error_page("x", "dialling <b>&\"node\"</b>", Retry::Never);
        assert!(page.contains("dialling &lt;b&gt;&amp;\"node\"&lt;/b&gt;"));
    }

    // -----------------------------------------------------------------------------------
    // Grants → a service list
    // -----------------------------------------------------------------------------------

    #[test]
    fn named_grants_become_a_service_list_and_a_wildcard_becomes_a_flag() {
        let grants: Vec<Grant> = ["http:nosh", "http:app", "tcp:127.0.0.1:22", "ctl:read"]
            .iter()
            .map(|g| g.parse().expect("grant"))
            .collect();
        let (services, any) = grantable_services(&grants);
        // Sorted, and neither the raw forward nor the control scope is offered as a page.
        assert_eq!(services, vec!["app".to_string(), "nosh".to_string()]);
        assert!(!any, "no wildcard was granted");

        let wild: Vec<Grant> = ["http:*", "http:app"]
            .iter()
            .map(|g| g.parse().expect("grant"))
            .collect();
        let (services, any) = grantable_services(&wild);
        assert_eq!(services, vec!["app".to_string()]);
        assert!(
            any,
            "http:* means the list is a starting point, not the whole"
        );
    }

    #[test]
    fn a_duplicated_grant_is_listed_once() {
        let grants: Vec<Grant> = ["http:app", "http:app"]
            .iter()
            .map(|g| g.parse().expect("grant"))
            .collect();
        assert_eq!(grantable_services(&grants).0, vec!["app".to_string()]);
    }

    #[test]
    fn no_grants_offers_nothing() {
        assert_eq!(grantable_services(&[]), (Vec::new(), false));
    }
}
