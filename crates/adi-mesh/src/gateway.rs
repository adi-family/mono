//! The mesh HTTP gateway — both ends of `adi/mesh/http/1` (`docs/fleet.md` §3, §5, §7, C5+C6).
//!
//! One process plays both parts, because every adi machine is potentially both a viewer and a
//! node:
//!
//! - **The calling side** ([`serve`]) is a loopback TCP listener. The front door forwards every
//!   `*.n.adi` host here verbatim (hive's `proxy.mesh_gateway`), and this is where a hostname
//!   stops being a name and becomes a *peer key*: the host is split into `(service, node)`, the
//!   node is resolved through the local [fleet registry](crate::fleet), and one bi-stream is
//!   opened on a **pooled** connection to that peer.
//! - **The node side** ([`serve_peer`]) is what a peer reaches. It resolves the service label
//!   against this machine's own hive route table, checks the peer's grants, enforces Basic auth,
//!   and splices the HTTP bytes to the local service.
//!
//! Three decisions are worth stating, because each is a way this could have been wrong.
//!
//! **Nothing in the head is rewritten.** Not `Host`, not the request target. The node cannot know
//! what the viewer calls it, so a rewritten `Host` would send its absolute redirects to a
//! same-named host on the *viewer's* machine, and a rewritten path would make a dashboard's `/api`
//! answer at a URL the page never asked for (`docs/fleet.md` §3, §4). What the gateway matched and
//! what the service reads are the same bytes.
//!
//! **Authorization comes before resolution.** [`admit`] asks "may this peer have `nosh`?" before
//! it asks "do we serve `nosh`?", so an unpaired peer learns nothing about which services exist
//! here — it gets [`HttpStatus::NotAuthorized`] whether the label is real or invented. Both layers
//! of §5 are enforced on the node, the side that owns the data: the mesh grant is machine-scoped
//! (any process on a paired laptop can reach us), and the password is what makes it human-scoped.
//!
//! **Connections are pooled, streams are not.** QUIC multiplexes streams natively, so one iroh
//! [`Connection`] per peer with a bi-stream per HTTP connection costs one handshake per peer
//! rather than one per request — the thing [`crate::client`]'s dial-per-accept gets wrong, and the
//! reason the [`Pool`] exists. Dialling sits behind the [`Dialer`] trait so the pool's single-flight
//! and backoff behaviour can be tested against a counter instead of the network.
//!
//! ## Why the ALPN is dispatched by hand
//!
//! An endpoint now answers two ALPNs, and iroh ships
//! [`protocol::Router`](iroh::protocol::Router) for exactly that. It was not adopted, for two
//! reasons that are about ownership rather than taste. It takes over `Endpoint::accept()`
//! *exclusively* — a QUIC endpoint has one accept loop, and [`crate::host`] already owns it — so
//! adopting it would mean rewriting the finished forward role as a `ProtocolHandler` for no
//! behavioural gain. And its `shutdown()` closes the endpoint, which is [`crate::Daemon`]'s job:
//! two owners of one lifecycle is how a "stop the mesh" ends up racing a half-closed endpoint.
//!
//! So the branch lives in the accept loop that already exists, one line of `conn.alpn()`. The
//! handshake has already chosen the protocol by then, which is why nothing on the wire needs a
//! discriminator, and why an old peer that speaks only `adi/mesh/forward/0` is unaffected.

use std::collections::HashMap;
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::sync::{Mutex as SyncMutex, PoisonError, RwLock};
use std::time::Duration;

use adi_hive::config::Hive;
use adi_hive::proxy::{Decision, Router as HiveRouter};
use anyhow::Context as _;
use iroh::endpoint::{Connection, RecvStream, SendStream};
use iroh::{Endpoint, EndpointId};
use std::sync::Arc;
use tokio::io::{AsyncRead, AsyncReadExt as _, AsyncWrite, AsyncWriteExt as _};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{Mutex, watch};
use tokio::time::Instant;
use tracing::{debug, info, warn};

use crate::fleet::{FleetRegistry, NodeRecord, Target};
use crate::protocol::{self, HttpStatus};
use crate::{auth, tunnel};

// ---------------------------------------------------------------------------------------
// Where the gateway listens
// ---------------------------------------------------------------------------------------

/// The default loopback port for the calling side.
///
/// Re-exported from [`adi_config`] rather than chosen here, because the number is only useful if
/// **two crates that cannot see each other agree on it**: `adi-core` writes it into the front
/// door's `proxy.mesh_gateway`, and this module binds it. The two ends never handshake — the
/// front door reads a file written once, the gateway binds at start-up — so a number typed in
/// both places would diverge silently, surfacing as a `502` with nothing listening. Both crates
/// already depend on `adi-config`, so it holds the value and the compiler keeps them in step.
/// The reasoning for the value itself is documented there.
///
/// It remains only a *default*: [`ADDR_ENV`] overrides it here, and the front door states the
/// real address in its own config, so the two are always configured together.
pub use adi_config::MESH_GATEWAY_PORT as DEFAULT_PORT;

/// Environment override for the gateway's listen address, e.g. `127.0.0.1:14081`. An env var
/// rather than a `mesh.toml` field because the address is really the front door's business —
/// `proxy.mesh_gateway` is where an operator states it — and this end only has to agree.
pub const ADDR_ENV: &str = "ADI_MESH_GATEWAY_ADDR";

/// Caps per-connection memory against a client that never sends the blank line.
const MAX_HEAD: usize = 16 * 1024;

/// So a silent client (or peer) cannot tie up a task forever.
const HEAD_TIMEOUT: Duration = Duration::from_secs(10);

/// How often the on-disk registry and route table are re-read, so a freshly paired node or a
/// newly added service starts working without restarting the mesh.
///
/// Deliberately a touch slower than the front door's own 3-second config tick (`adi-hive`'s
/// `RELOAD_INTERVAL`): the two read the same `hive.yaml`, and the gateway has no reason to be the
/// more eager of the pair.
const RELOAD_INTERVAL: Duration = Duration::from_secs(5);

/// After an accept error, pause briefly so a persistent failure cannot spin the loop hot.
const ACCEPT_ERROR_BACKOFF: Duration = Duration::from_millis(100);

/// The zone a node's own services live in locally (`nosh.adi`), and therefore the host a service
/// *label* off the wire is resolved as. `docs/fleet.md` §1: the same service the fleet addresses
/// as `nosh.laptop-b.n.adi` is `nosh.adi` on the node itself.
const LOCAL_ZONE: &str = "adi";

/// The gateway's default listen address.
#[must_use]
pub fn default_addr() -> SocketAddr {
    SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, DEFAULT_PORT))
}

/// The address to listen on: [`ADDR_ENV`] when it parses, [`default_addr`] otherwise.
#[must_use]
pub fn configured_addr() -> SocketAddr {
    addr_from_env(std::env::var(ADDR_ENV).ok().as_deref())
}

/// The pure half of [`configured_addr`], so the fallback behaviour is testable without touching
/// the process environment. An unparseable value warns and falls back rather than refusing to
/// start: a typo in an override must not cost you the whole mesh.
fn addr_from_env(raw: Option<&str>) -> SocketAddr {
    match raw.map(str::trim).filter(|raw| !raw.is_empty()) {
        None => default_addr(),
        Some(raw) => raw.parse().unwrap_or_else(|e| {
            warn!(%raw, error = %e, "{ADDR_ENV} is not a socket address; using the default");
            default_addr()
        }),
    }
}

// ---------------------------------------------------------------------------------------
// The shared state both sides read
// ---------------------------------------------------------------------------------------

/// A value re-read from disk on a tick, handed out as a snapshot.
///
/// A snapshot rather than a lock held across the request: a connection that started under one
/// route table finishes under it, and a reload never blocks a request in flight.
#[derive(Debug)]
struct Snapshot<T> {
    current: RwLock<Arc<T>>,
}

impl<T> Snapshot<T> {
    fn new(value: T) -> Self {
        Self {
            current: RwLock::new(Arc::new(value)),
        }
    }

    fn get(&self) -> Arc<T> {
        Arc::clone(&self.current.read().unwrap_or_else(PoisonError::into_inner))
    }

    fn set(&self, value: T) {
        *self.current.write().unwrap_or_else(PoisonError::into_inner) = Arc::new(value);
    }
}

/// This machine's own hive route table, reduced to the one question the node side asks of it:
/// *which local port serves this service label, for this request target?*
///
/// It is [`adi_hive`]'s own [`HiveRouter`], not a lookup of our own. A second implementation
/// would be a second set of rules to keep in step, and the first time they drifted a service
/// would resolve differently depending on whether you came in through the front door or the mesh.
#[derive(Debug)]
pub struct Routes {
    router: HiveRouter,
}

impl Default for Routes {
    /// An empty table: every label is [`HttpStatus::ServiceUnknown`]. This is what a node falls
    /// back to when its `hive.yaml` cannot be read — fail-closed, never "serve everything".
    fn default() -> Self {
        Self {
            router: HiveRouter::new(&[], None),
        }
    }
}

/// The front-door config this machine runs, or `None` when it has none.
///
/// Preference, not a merge: these are two descriptions of *one* front door, and a machine runs
/// whichever it has. Merging them would invent routes no front door serves.
fn front_door_config() -> Option<std::path::PathBuf> {
    front_door_config_in(
        &adi_config::Config::open(),
        &adi_hive::config::default_config_path(),
    )
}

/// The pure half of [`front_door_config`], so the preference is testable against a temp store
/// instead of the machine's real one.
fn front_door_config_in(
    store: &adi_config::Config,
    hand_managed: &std::path::Path,
) -> Option<std::path::PathBuf> {
    if hand_managed.is_file() {
        return Some(hand_managed.to_path_buf());
    }
    let generated = store
        .module(adi_config::FRONTDOOR_MODULE)
        .raw_path(adi_config::FRONTDOOR_CONFIG_FILE);
    generated.is_file().then_some(generated)
}

impl Routes {
    /// Build the table from an already-parsed hive.
    #[must_use]
    pub fn from_hive(hive: &Hive) -> Self {
        // No mesh gateway is passed on purpose: this table answers questions about *local*
        // services only. A `*.n.adi` host reaching it would mean a node forwarding a fleet name
        // back into the fleet, which is a loop, and the `None` makes that unrepresentable.
        Self {
            router: HiveRouter::new(&hive.resolve().routes, None),
        }
    }

    /// Load the config the machine's front door actually runs, and build the table.
    ///
    /// **Two configs exist and only one of them is present on a node.** The hand-managed
    /// `hive/hive.yaml` is the richer one — it imports every project and dashboard — and where it
    /// exists it is what the front door runs, so it wins. A freshly installed node has only the
    /// config `adi-core` *generates* (`dns/hive-frontdoor.yaml`); reading solely the canonical
    /// path there yields an empty route table, and the node then refuses every request with
    /// [`HttpStatus::ServiceUnknown`] — reachable, paired, authorized, and serving nothing.
    /// That failure is invisible on a developer machine, where the hand-managed file happens to
    /// exist, and total on the machine this feature is for.
    ///
    /// # Errors
    /// Propagates any read/parse error from [`Hive::load`]. A machine with *neither* config is
    /// not an error: it has no front door yet, so it serves nothing, and the gateway keeps
    /// running to say so.
    pub fn load() -> anyhow::Result<Self> {
        let Some(path) = front_door_config() else {
            debug!("no hive config on this machine; the gateway serves no local service");
            return Ok(Self::from_hive(&Hive::default()));
        };
        let mut hive = Hive::load(&path)
            .with_context(|| format!("reading the node's hive config at {}", path.display()))?;
        // The same step the front door runs before resolving. A service that declares no port —
        // which is *every* dashboard, since adi-hive leases theirs — has none to resolve until
        // this fills it in, and `resolve` then drops it as "no HTTP port". Skipping this is why
        // a node served its control panel over the mesh but refused every dashboard: the routes
        // were in the config, and silently discarded one step before the table.
        //
        // Reserving is idempotent and keyed identically (`<dashboard-id>/<service>`), so this
        // reads back the supervisor's existing lease rather than inventing a second port.
        let allocated = hive.allocate_missing_ports(&adi_ports_manager::Ports::new());
        if !allocated.is_empty() {
            debug!(?allocated, "gateway: filled in leased ports for the route table");
        }
        Ok(Self::from_hive(&hive))
    }

    /// The loopback address serving `service` for `target`, or `None` when this node serves no
    /// such service.
    ///
    /// `target` is the raw request target, so a service that claims a path prefix on another's
    /// host (a dashboard's `/api` backend, `docs/fleet.md` §4) resolves the same way here as at
    /// the front door — otherwise "one origin per dashboard" would hold locally and quietly break
    /// over the mesh.
    #[must_use]
    pub fn resolve(&self, service: &str, target: &str) -> Option<SocketAddr> {
        match self.router.route(&format!("{service}.{LOCAL_ZONE}"), target) {
            Decision::Service(addr) => Some(addr),
            _ => None,
        }
    }
}

/// One paired peer, as the node side needs it for a single connection: what we call it, and the
/// policy attached to it.
#[derive(Debug, Clone)]
pub struct Peer {
    /// What *this* machine calls the peer (`docs/fleet.md` §2).
    pub petname: String,
    /// Its grants and credential.
    pub record: NodeRecord,
}

/// What the node side needs to know about this machine. A trait so the decision logic can be
/// exercised against a stub — a registry and a route table in memory — with no config on disk and
/// no endpoint bound.
pub trait NodeSide: Send + Sync + 'static {
    /// The paired peer holding this key, or `None` if it is a stranger.
    fn peer(&self, key: &EndpointId) -> Option<Peer>;

    /// Where `service` lives locally for this request target.
    fn resolve(&self, service: &str, target: &str) -> Option<SocketAddr>;

    /// The Basic-auth realm — this node's name, which is what a browser shows in its prompt.
    fn realm(&self) -> String;
}

/// The gateway: both ends of `adi/mesh/http/1`, plus the state they share.
///
/// One object rather than two because the registry is the same file for both — the calling side
/// asks it "what key is `laptop-b`?" and the node side asks it "may this key have `nosh`?" — and
/// two independently reloaded copies of one file is a way for them to disagree.
#[derive(Debug)]
pub struct Gateway {
    /// This machine's key. It appears on the not-paired page because it is the thing the far
    /// side's operator has to authorize; a name grants nothing (`docs/fleet.md` §2).
    local_key: EndpointId,
    /// The Basic-auth realm: this machine's own nickname ([`crate::node`]). A snapshot like the
    /// rest, so `mesh name` takes effect on the next tick — the alternative is a browser prompt
    /// that keeps naming the node by its old name until somebody restarts the mesh.
    realm: Snapshot<String>,
    registry: Snapshot<FleetRegistry>,
    routes: Snapshot<Routes>,
    pool: Pool<IrohDialer>,
}

impl Gateway {
    /// Build the gateway over an already-bound endpoint, loading the registry and route table.
    ///
    /// Neither load is fatal: a machine with no `fleet.toml` yet has no peers to reach and none to
    /// serve, and both empty defaults deny rather than allow.
    #[must_use]
    pub fn new(endpoint: Endpoint) -> Self {
        let local_key = endpoint.id();
        let registry = FleetRegistry::load().unwrap_or_else(|e| {
            warn!(error = %e, "gateway: could not read the fleet registry; no node is reachable");
            FleetRegistry::default()
        });
        let routes = Routes::load().unwrap_or_else(|e| {
            debug!(error = %e, "gateway: no local hive route table; this node serves nothing over the mesh");
            Routes::default()
        });
        Self {
            local_key,
            realm: Snapshot::new(crate::node::nickname()),
            registry: Snapshot::new(registry),
            routes: Snapshot::new(routes),
            pool: Pool::new(IrohDialer { endpoint }),
        }
    }

    /// Re-read the registry and the route table. Blocking file I/O — call it off the runtime.
    ///
    /// A failed load keeps the previous snapshot: a half-written `fleet.toml` must not silently
    /// unpair every node.
    pub fn refresh(&self) {
        match FleetRegistry::load() {
            Ok(registry) => self.registry.set(registry),
            Err(e) => warn!(error = %e, "gateway: keeping the previous fleet registry"),
        }
        match Routes::load() {
            Ok(routes) => self.routes.set(routes),
            Err(e) => debug!(error = %e, "gateway: keeping the previous route table"),
        }
        self.realm.set(crate::node::nickname());
    }

    /// This machine's key, as shown on the error pages.
    #[must_use]
    pub fn local_key(&self) -> EndpointId {
        self.local_key
    }
}

impl NodeSide for Gateway {
    fn peer(&self, key: &EndpointId) -> Option<Peer> {
        self.registry.get().by_key(key).map(|(petname, record)| Peer {
            petname: petname.to_string(),
            record: record.clone(),
        })
    }

    fn resolve(&self, service: &str, target: &str) -> Option<SocketAddr> {
        self.routes.get().resolve(service, target)
    }

    /// The realm a node challenges with: its own nickname, which is the same string it offered
    /// at pairing ([`crate::node::nickname`]).
    ///
    /// It used to be `$ADI_NODE_NAME` or, failing that, `adi node <short key>`. A short key is
    /// honest but it is not what the operator agreed at pairing, so the browser prompt named
    /// something they could not match against their fleet list. Now one accessor answers both
    /// questions, and a realm that disagreed with the offered nickname is unrepresentable.
    fn realm(&self) -> String {
        self.realm.get().as_ref().clone()
    }
}

/// Re-read the gateway's on-disk state every [`RELOAD_INTERVAL`] until shutdown, so pairing a
/// node or adding a service takes effect without restarting the mesh.
pub async fn reload(gateway: Arc<Gateway>, mut shutdown: watch::Receiver<bool>) {
    loop {
        tokio::select! {
            _ = shutdown.changed() => return,
            () = tokio::time::sleep(RELOAD_INTERVAL) => {
                let gateway = Arc::clone(&gateway);
                // Off the runtime: both loads are file reads, and the hive one expands import
                // globs, which is a directory walk.
                let _ = tokio::task::spawn_blocking(move || gateway.refresh()).await;
            }
        }
    }
}

// ---------------------------------------------------------------------------------------
// C6 — the node side
// ---------------------------------------------------------------------------------------

/// Serve one peer's connection: a task per bi-stream, for as long as the peer keeps it open.
///
/// A connection is a *peer*, not a request — that is the whole point of pooling it — so this
/// loops instead of handling one stream and closing. It ends when the peer (or the endpoint)
/// closes the connection.
pub async fn serve_peer<N: NodeSide>(conn: Connection, node: Arc<N>) {
    let peer = conn.remote_id();
    debug!(%peer, "gateway: peer connection open");
    loop {
        match conn.accept_bi().await {
            Ok((send, recv)) => {
                let node = Arc::clone(&node);
                tokio::spawn(async move {
                    if let Err(e) = serve_stream(peer, node, send, recv).await {
                        debug!(%peer, error = %e, "gateway: peer stream ended with error");
                    }
                });
            }
            Err(e) => {
                debug!(%peer, error = %e, "gateway: peer connection closed");
                return;
            }
        }
    }
}

/// One bi-stream: one HTTP connection from a peer.
async fn serve_stream<N: NodeSide>(
    peer: EndpointId,
    node: Arc<N>,
    mut send: SendStream,
    mut recv: RecvStream,
) -> anyhow::Result<()> {
    let Some(upstream) = negotiate(peer, node.as_ref(), &mut send, &mut recv).await? else {
        // Already answered — a status byte, or a `401` on an accepted stream. Finishing the
        // stream is enough: the connection is shared and stays up, so the peer reliably reads
        // what we wrote without us lingering on it the way a per-tunnel connection must.
        let _ = send.finish();
        return Ok(());
    };
    tunnel::splice(upstream, send, recv).await;
    Ok(())
}

/// The node side up to the point where bytes flow: read the frame, authorize, resolve, connect,
/// answer the status, gate on Basic auth, and hand over the head **verbatim**.
///
/// Returns the connected local service with the request head already written to it, or `None`
/// when it has answered the caller itself (a refusal status, or a `401`).
///
/// # Errors
/// Any stream I/O error; a malformed request frame.
async fn negotiate<N, W, R>(
    peer: EndpointId,
    node: &N,
    send: &mut W,
    recv: &mut R,
) -> anyhow::Result<Option<TcpStream>>
where
    N: NodeSide + ?Sized,
    W: AsyncWrite + Unpin,
    R: AsyncRead + Unpin,
{
    let service = protocol::read_http_request(recv).await?;
    let (peer_record, addr) = match admit(node, &peer, &service) {
        Ok(admitted) => admitted,
        Err(status) => {
            debug!(%peer, %service, reason = status.reason(), "gateway: refusing peer request");
            protocol::write_http_status(send, status).await?;
            return Ok(None);
        }
    };

    // Connect before answering, so `UpstreamUnavailable` means what the table says it means:
    // the service is configured but nothing is listening.
    let mut upstream = match TcpStream::connect(addr).await {
        Ok(upstream) => upstream,
        Err(e) => {
            warn!(%peer, %service, %addr, error = %e, "gateway: local service not listening");
            protocol::write_http_status(send, HttpStatus::UpstreamUnavailable).await?;
            return Ok(None);
        }
    };
    protocol::write_http_status(send, HttpStatus::Ok).await?;

    let head = read_head(recv).await?;
    if !authenticated(&head, &peer_record) {
        debug!(peer = %peer_record.petname, %service, "gateway: 401 — no usable credentials");
        // A `401` is an ordinary HTTP response on an `Ok` stream (`docs/fleet.md` §7): the
        // transport worked, the human did not authenticate.
        send.write_all(auth::unauthorized_response(&node.realm()).as_bytes())
            .await?;
        send.flush().await?;
        return Ok(None);
    }

    // Re-resolve against the real target now that we have it: the first resolution used the
    // host's fallback route, which is the wrong upstream for a service claiming a path prefix.
    if let Some(target) = request_target(&head)
        && let Some(better) = node.resolve(&service, &target)
        && better != addr
    {
        match TcpStream::connect(better).await {
            Ok(reconnected) => upstream = reconnected,
            Err(e) => {
                warn!(%service, %better, error = %e, "gateway: path-routed upstream not listening");
                send.write_all(BAD_GATEWAY.as_bytes()).await?;
                send.flush().await?;
                return Ok(None);
            }
        }
    }

    upstream.write_all(&head).await?;
    Ok(Some(upstream))
}

/// May this peer have this service, and where does it live?
///
/// The order is the policy: an unknown peer and an unauthorized one are the same answer, and
/// both are given *before* the route table is consulted, so nobody can enumerate a node's
/// services by watching `ServiceUnknown` and `NotAuthorized` differ.
///
/// # Errors
/// The [`HttpStatus`] to send back.
fn admit<N: NodeSide + ?Sized>(
    node: &N,
    peer: &EndpointId,
    service: &str,
) -> Result<(Peer, SocketAddr), HttpStatus> {
    // Default-deny, both times: an unpaired key has no record, and a record with no matching
    // grant allows nothing. There is no "empty means everyone" path here (`docs/fleet.md` §5).
    let peer = node.peer(peer).ok_or(HttpStatus::NotAuthorized)?;
    if !peer.record.allows(Target::Http(service)) {
        return Err(HttpStatus::NotAuthorized);
    }
    let addr = node
        .resolve(service, "/")
        .ok_or(HttpStatus::ServiceUnknown)?;
    Ok((peer, addr))
}

/// Does this request head carry credentials that verify against the peer's stored one?
///
/// Nothing about the request is inspected but the credentials — not the method, not the path, not
/// `Upgrade` — so a WebSocket handshake is gated by *not* adding an exception (`docs/fleet.md`
/// §5). Default-deny: a peer with no password configured authenticates nobody.
fn authenticated(head: &[u8], peer: &Peer) -> bool {
    auth::parse_basic_credentials(head)
        .is_some_and(|(user, password)| peer.record.verify_password(&user, &password))
}

/// The plain `502` a node sends when a path-routed upstream turns out to be dead *after* the
/// stream was accepted. HTTP-level, because by then the transport has already said `Ok`.
const BAD_GATEWAY: &str = "HTTP/1.1 502 Bad Gateway\r\n\
     Content-Type: text/plain; charset=utf-8\r\n\
     Content-Length: 40\r\n\
     Connection: close\r\n\
     \r\n\
     502 Bad Gateway: the service is not up.\n";

// ---------------------------------------------------------------------------------------
// C5 — the calling side
// ---------------------------------------------------------------------------------------

/// Bind the gateway's loopback listener.
///
/// # Errors
/// Any bind error — most often the port already being in use.
pub async fn bind(addr: SocketAddr) -> std::io::Result<TcpListener> {
    TcpListener::bind(addr).await
}

/// Accept loop for the calling side: a task per connection until shutdown.
pub async fn serve(listener: TcpListener, gateway: Arc<Gateway>, mut shutdown: watch::Receiver<bool>) {
    loop {
        tokio::select! {
            _ = shutdown.changed() => {
                info!("gateway: stopping");
                return;
            }
            accepted = listener.accept() => match accepted {
                Ok((tcp, client)) => {
                    let gateway = Arc::clone(&gateway);
                    tokio::spawn(async move {
                        if let Err(e) = handle_client(tcp, &gateway).await {
                            debug!(%client, error = %e, "gateway: client connection ended with error");
                        }
                    });
                }
                Err(e) => {
                    warn!(error = %e, "gateway: accept failed");
                    tokio::time::sleep(ACCEPT_ERROR_BACKOFF).await;
                }
            }
        }
    }
}

/// One browser connection: name → peer → bi-stream → bytes.
async fn handle_client(mut tcp: TcpStream, gateway: &Gateway) -> anyhow::Result<()> {
    let head = read_head(&mut tcp).await?;
    if head.is_empty() {
        return Ok(()); // The client hung up before sending anything.
    }
    let Some(host) = header_value(&head, "host") else {
        return Ok(respond(&mut tcp, 400, "Bad Request", &bad_request_page("")).await?);
    };
    let Some((service, node)) = protocol::parse_fleet_host(&host) else {
        // The front door only sends `*.n.adi` here, so this is a misconfiguration (or somebody
        // talking to the gateway directly) rather than a routing decision to make.
        return Ok(respond(&mut tcp, 400, "Bad Request", &bad_request_page(&host)).await?);
    };

    let Some(key) = gateway.registry.get().key_of(&node) else {
        info!(%host, %node, "gateway: no such node in the fleet registry");
        let page = not_paired_page(&host, &node, &gateway.local_key.to_string());
        return Ok(respond(&mut tcp, 502, "Bad Gateway", &page).await?);
    };

    let conn = match gateway.pool.get(key).await {
        Ok(conn) => conn,
        Err(e) => {
            warn!(%host, %node, error = %e, "gateway: cannot reach the node");
            let page = unreachable_page(&host, &node, &key.to_string(), &e.to_string());
            return Ok(respond(&mut tcp, 502, "Bad Gateway", &page).await?);
        }
    };

    let (mut send, mut recv) = match conn.open_bi().await {
        Ok(streams) => streams,
        Err(e) => {
            // The pooled connection died between the liveness check and now; the next request
            // re-dials it. Nothing to retry here — the browser will.
            warn!(%host, %node, error = %e, "gateway: could not open a stream to the node");
            let page = unreachable_page(&host, &node, &key.to_string(), &e.to_string());
            return Ok(respond(&mut tcp, 502, "Bad Gateway", &page).await?);
        }
    };

    protocol::write_http_request(&mut send, &service).await?;
    match protocol::read_http_status(&mut recv).await? {
        HttpStatus::Ok => {
            // The head verbatim, `Host` and target untouched — see the module docs.
            send.write_all(&head).await?;
            debug!(%host, %node, %service, "gateway: proxying");
            tunnel::splice(tcp, send, recv).await;
            Ok(())
        }
        refused => {
            info!(%host, %node, %service, reason = refused.reason(), "gateway: node refused");
            let page = refused_page(&host, &node, &key.to_string(), refused.reason());
            Ok(respond(&mut tcp, 502, "Bad Gateway", &page).await?)
        }
    }
}

// ---------------------------------------------------------------------------------------
// The connection pool
// ---------------------------------------------------------------------------------------

/// How a peer is reached. The seam that keeps the [`Pool`]'s interesting behaviour — one dial for
/// N concurrent callers, re-dial after a close, back off after a failure — testable without a
/// network, a relay, or a second endpoint.
pub trait Dialer: Send + Sync + 'static {
    /// The connection this dialer produces.
    type Conn: Send + Sync + 'static;

    /// Dial `peer`.
    ///
    /// # Errors
    /// Whatever the transport reports; the pool turns a failure into backoff, not a retry loop.
    fn dial(
        &self,
        peer: EndpointId,
    ) -> impl std::future::Future<Output = anyhow::Result<Self::Conn>> + Send;

    /// Is a pooled connection still worth handing out?
    fn is_usable(&self, conn: &Self::Conn) -> bool;
}

/// The production dialer: an iroh connection on the gateway ALPN.
#[derive(Debug)]
pub struct IrohDialer {
    endpoint: Endpoint,
}

impl IrohDialer {
    /// Dial peers over `endpoint`.
    ///
    /// [`Gateway`] builds one for itself, but the pool is also useful on its own — a viewer that
    /// has no local route table to serve (the iOS app) wants the dialling and backoff behaviour
    /// without the node side that owns it here.
    #[must_use]
    pub fn new(endpoint: Endpoint) -> Self {
        Self { endpoint }
    }
}

impl Dialer for IrohDialer {
    type Conn = Connection;

    async fn dial(&self, peer: EndpointId) -> anyhow::Result<Connection> {
        // By key alone: discovery resolves it to current addresses, and the key is the identity
        // of record (`docs/fleet.md` §2) — a stale address list would be a second source of truth.
        Ok(self.endpoint.connect(peer, protocol::HTTP_ALPN).await?)
    }

    fn is_usable(&self, conn: &Connection) -> bool {
        conn.close_reason().is_none()
    }
}

/// Base delay after a peer's first failed dial.
const BACKOFF_BASE: Duration = Duration::from_millis(500);

/// Ceiling on the backoff, so a node that is simply switched off is still retried now and then.
const BACKOFF_CAP: Duration = Duration::from_secs(30);

/// One live connection per peer, a bi-stream per HTTP connection.
///
/// The single-flight is the per-peer lock: concurrent callers for one peer queue on it, the first
/// dials, and the rest wake up to the connection it stored. That is also why a *failing* peer
/// cannot spin — the same lock serialises the failures, and the backoff deadline refuses the next
/// attempt outright instead of dialling again.
#[derive(Debug)]
pub struct Pool<D: Dialer> {
    dialer: D,
    base: Duration,
    cap: Duration,
    peers: SyncMutex<HashMap<EndpointId, Arc<Slot<D::Conn>>>>,
}

/// One peer's slot. The lock is `tokio`'s because it is held across the dial.
#[derive(Debug)]
struct Slot<C> {
    state: Mutex<SlotState<C>>,
}

#[derive(Debug)]
struct SlotState<C> {
    conn: Option<Arc<C>>,
    failures: u32,
    /// When the next dial may be attempted, after a failure.
    retry_after: Option<Instant>,
}

impl<C> Default for Slot<C> {
    fn default() -> Self {
        Self {
            state: Mutex::new(SlotState {
                conn: None,
                failures: 0,
                retry_after: None,
            }),
        }
    }
}

impl<D: Dialer> Pool<D> {
    /// A pool with the standard backoff.
    #[must_use]
    pub fn new(dialer: D) -> Self {
        Self::with_backoff(dialer, BACKOFF_BASE, BACKOFF_CAP)
    }

    /// A pool with an explicit backoff schedule (the tests use a short one).
    #[must_use]
    pub fn with_backoff(dialer: D, base: Duration, cap: Duration) -> Self {
        Self {
            dialer,
            base,
            cap,
            peers: SyncMutex::new(HashMap::new()),
        }
    }

    /// The live connection to `peer`, dialling one if there is none or the last one died.
    ///
    /// # Errors
    /// The dial's own error, or a refusal while this peer is in backoff — a caller that gets one
    /// should render an error page, not retry in a loop.
    pub async fn get(&self, peer: EndpointId) -> anyhow::Result<Arc<D::Conn>> {
        let slot = {
            let mut peers = self.peers.lock().unwrap_or_else(PoisonError::into_inner);
            Arc::clone(peers.entry(peer).or_default())
        };
        // Held across the dial on purpose: this *is* the single flight.
        let mut state = slot.state.lock().await;

        if let Some(conn) = &state.conn {
            if self.dialer.is_usable(conn) {
                return Ok(Arc::clone(conn));
            }
            debug!(%peer, "pool: pooled connection is closed; re-dialling");
            state.conn = None;
        }

        if let Some(at) = state.retry_after
            && let Some(left) = at.checked_duration_since(Instant::now())
        {
            anyhow::bail!(
                "not dialling {peer}: {} dial(s) failed, retrying in {left:.1?}",
                state.failures
            );
        }

        match self.dialer.dial(peer).await {
            Ok(conn) => {
                let conn = Arc::new(conn);
                state.conn = Some(Arc::clone(&conn));
                state.failures = 0;
                state.retry_after = None;
                Ok(conn)
            }
            Err(e) => {
                state.failures = state.failures.saturating_add(1);
                let delay = backoff_delay(state.failures, self.base, self.cap);
                state.retry_after = Some(Instant::now() + delay);
                Err(e.context(format!("dialling {peer} (next attempt in {delay:.1?})")))
            }
        }
    }
}

/// Exponential backoff: `base`, doubling per consecutive failure, capped at `cap`.
fn backoff_delay(failures: u32, base: Duration, cap: Duration) -> Duration {
    if failures == 0 {
        return Duration::ZERO;
    }
    // Saturating throughout: 30 consecutive failures must not overflow into a short delay.
    let doublings = (failures - 1).min(16);
    base.saturating_mul(1u32 << doublings).min(cap)
}

// ---------------------------------------------------------------------------------------
// HTTP head handling
// ---------------------------------------------------------------------------------------

/// Read until the blank line ending the head, a size cap, or a timeout.
///
/// The buffer is returned as read — it may already hold the first body bytes — because it is
/// forwarded verbatim, and slicing it at the blank line would mean re-assembling what the sender
/// wrote.
///
/// # Errors
/// Any read error, or [`std::io::ErrorKind::TimedOut`] if the sender stalls mid-head.
async fn read_head<R: AsyncRead + Unpin>(r: &mut R) -> std::io::Result<Vec<u8>> {
    let mut buf = Vec::with_capacity(1024);
    let mut chunk = [0u8; 1024];
    loop {
        let read = tokio::time::timeout(HEAD_TIMEOUT, r.read(&mut chunk))
            .await
            .map_err(|_| {
                std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    "timed out reading the request head",
                )
            })??;
        if read == 0 {
            break;
        }
        buf.extend_from_slice(&chunk[..read]);
        if head_complete(&buf) || buf.len() >= MAX_HEAD {
            break;
        }
    }
    Ok(buf)
}

/// Has the head's terminating blank line arrived? Searched over the whole buffer so a `\r\n\r\n`
/// split across two reads still counts.
fn head_complete(buf: &[u8]) -> bool {
    buf.windows(4).any(|w| w == b"\r\n\r\n")
}

/// A header's value, matched case-insensitively as HTTP requires.
fn header_value(head: &[u8], name: &str) -> Option<String> {
    let text = String::from_utf8_lossy(head);
    // `skip(1)`: the request line is not a header, and it contains a `:` in `HTTP/1.1`.
    for line in text.split("\r\n").skip(1) {
        if line.is_empty() {
            break; // End of the head.
        }
        if let Some((field, value)) = line.split_once(':')
            && field.trim().eq_ignore_ascii_case(name)
        {
            let value = value.trim();
            if !value.is_empty() {
                return Some(value.to_string());
            }
        }
    }
    None
}

/// The request target off the request line (`METHOD SP target SP HTTP/1.1`), used only to pick
/// among a host's path-prefixed routes.
fn request_target(head: &[u8]) -> Option<String> {
    let text = String::from_utf8_lossy(head);
    let mut parts = text.split("\r\n").next()?.split(' ').filter(|p| !p.is_empty());
    let _method = parts.next()?;
    let target = parts.next()?;
    // No version means a truncated request line, not a target worth routing on.
    parts.next()?;
    Some(target.to_string())
}

/// Write a self-contained error page and close.
///
/// # Errors
/// Any write error on the client connection.
async fn respond<W: AsyncWrite + Unpin>(
    w: &mut W,
    code: u16,
    reason: &str,
    body: &str,
) -> std::io::Result<()> {
    let response = format!(
        "HTTP/1.1 {code} {reason}\r\n\
         Content-Type: text/html; charset=utf-8\r\n\
         Content-Length: {len}\r\n\
         Cache-Control: no-store\r\n\
         Connection: close\r\n\
         \r\n\
         {body}",
        len = body.len()
    );
    w.write_all(response.as_bytes()).await?;
    w.flush().await?;
    let _ = w.shutdown().await;
    Ok(())
}

// ---------------------------------------------------------------------------------------
// The error pages
// ---------------------------------------------------------------------------------------

/// The page a viewer gets when the hostname names a node this machine has not paired with.
///
/// This page is the gateway's, not the front door's: the front door sees a reserved suffix and
/// hands the connection on, deliberately knowing nothing about peers (`docs/fleet.md` A4). What
/// it cannot say, and this can, is *which key* to authorize — a name grants nothing, so the key
/// is the only actionable thing on the screen.
#[must_use]
fn not_paired_page(host: &str, node: &str, local_key: &str) -> String {
    page(
        host,
        "node not paired",
        &format!(
            "node `{node}` is not paired with this machine — ask its administrator to pair the key below, \
             then pair it back here. Authorization is by key; the name is only what you call it."
        ),
        &[("this machine's key", local_key)],
    )
}

/// The page for a node that is paired but could not be reached at all.
#[must_use]
fn unreachable_page(host: &str, node: &str, node_key: &str, detail: &str) -> String {
    page(
        host,
        "node unreachable",
        &format!(
            "`{node}` is paired with this machine, but the mesh could not reach it. It may be offline, \
             or without a route to a relay."
        ),
        &[("node key", node_key), ("detail", detail)],
    )
}

/// The page for a node that answered — with a refusal. Carries the node's own reason, so the
/// difference between "no such service there", "you hold no grant" and "its service is down" is
/// visible instead of collapsed into one generic failure.
#[must_use]
fn refused_page(host: &str, node: &str, node_key: &str, reason: &str) -> String {
    page(
        host,
        "node refused the request",
        &format!("`{node}` answered, but refused: {reason}."),
        &[("node key", node_key)],
    )
}

/// The page for a request that never named a fleet host — somebody talking to the gateway port
/// directly, or a front door forwarding more than `*.n.adi`.
#[must_use]
fn bad_request_page(host: &str) -> String {
    page(
        host,
        "not a fleet hostname",
        "The mesh gateway answers hostnames of the form `<service>.<node>.n.adi` only. Nothing else \
         reaches a remote node, and nothing local is served here.",
        &[],
    )
}

/// The shared page shell. Styled after [`adi_hive::notfound::mesh_unavailable`] so the front
/// door's "no gateway here" page and the gateway's own pages read as one family; self-contained,
/// because a page explaining that nothing is reachable cannot fetch an asset to say it.
///
/// Everything interpolated came off the wire (a `Host` header is whatever the client wrote), so
/// every piece is HTML-escaped.
fn page(host: &str, reason: &str, message: &str, rows: &[(&str, &str)]) -> String {
    let host = escape(host);
    let reason = escape(reason);
    let message = escape(message);
    let rows = rows
        .iter()
        .filter(|(_, value)| !value.is_empty())
        .fold(String::new(), |mut html, (label, value)| {
            use std::fmt::Write as _;
            let _ = write!(
                html,
                "<div class=\"row\"><span class=\"k\">{}</span><code>{}</code></div>",
                escape(label),
                escape(value)
            );
            html
        });
    format!(
        r#"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>{host} — {reason}</title>
<style>
  /* Mirrors the adi design-system tokens; inlined because this page makes no external
     requests. Keep in sync with crates/adi-css/scss/_tokens.scss. */
  :root {{ --bg:#fafafb; --fg:#0d0f12; --muted:#6b7280; --line:#e5e7eb; --accent:#c96422; }}
  @media (prefers-color-scheme: dark) {{
    :root {{ --bg:#0a0b0d; --fg:#e9ecf1; --muted:#8b919c; --line:#23262b; --accent:#e08a4a; }}
  }}
  * {{ box-sizing: border-box; }}
  html, body {{ height: 100%; }}
  body {{
    margin:0; min-height:100vh; display:flex; align-items:center; justify-content:center;
    padding:40px 24px; background:var(--bg); color:var(--fg);
    letter-spacing:-.006em; -webkit-font-smoothing:antialiased;
    font:13.5px/1.5 ui-sans-serif, system-ui, -apple-system, "Segoe UI", Roboto, Helvetica, Arial, sans-serif;
  }}
  .wrap {{ display:flex; flex-direction:column; align-items:center; gap:14px;
    text-align:center; max-width:36rem; }}
  .link {{ width:min(196px, 46vw); height:auto; color:var(--fg); overflow:visible; }}
  .link .far {{ opacity:.32; }}
  .link .span {{ stroke-dasharray:6 7; animation:meshDrift 2.6s linear infinite; }}
  @keyframes meshDrift {{ to {{ stroke-dashoffset:-26; }} }}
  @media (prefers-reduced-motion: reduce) {{ .link .span {{ animation:none; }} }}
  .line {{ display:flex; align-items:center; margin-top:6px; }}
  .code {{ font-size:20px; font-weight:600; letter-spacing:-.02em;
    font-variant-numeric:tabular-nums; }}
  .reason {{ margin-left:14px; padding-left:14px; border-left:1px solid var(--line);
    color:var(--muted); }}
  .host {{ margin:0; font:600 15px/1.3 ui-monospace, SFMono-Regular, Menlo, Consolas, monospace;
    color:var(--accent); word-break:break-all; }}
  .msg {{ margin:0; color:var(--muted); }}
  .facts {{ width:100%; border-top:1px solid var(--line); padding-top:14px; margin-top:4px;
    display:flex; flex-direction:column; gap:8px; }}
  .row {{ display:flex; gap:12px; align-items:baseline; text-align:left; }}
  .k {{ flex:0 0 10.5rem; color:var(--muted); }}
  code {{ font:12.5px/1.4 ui-monospace, SFMono-Regular, Menlo, Consolas, monospace;
    color:var(--fg); word-break:break-all; }}
</style>
</head>
<body>
  <div class="wrap">
    <svg class="link" viewBox="0 0 240 90" fill="none" role="img"
         aria-label="two machines, not connected">
      <g class="near">
        <path d="M24 45 L44 33 L64 45 L44 57 Z" stroke="currentColor" stroke-width="2"
              stroke-linejoin="round"/>
        <circle cx="44" cy="45" r="5" fill="var(--accent)"/>
      </g>
      <line class="span" x1="74" y1="45" x2="166" y2="45" stroke="currentColor" stroke-width="2"
            stroke-linecap="round" opacity=".45"/>
      <g class="far">
        <path d="M176 45 L196 33 L216 45 L196 57 Z" stroke="currentColor" stroke-width="2"
              stroke-linejoin="round"/>
        <circle cx="196" cy="45" r="5" fill="currentColor"/>
      </g>
    </svg>

    <div class="line">
      <span class="code">502</span>
      <span class="reason">{reason}</span>
    </div>

    <p class="host">{host}</p>
    <p class="msg">{message}</p>

    <div class="facts">{rows}</div>
  </div>
</body>
</html>
"#
    )
}

/// Escape the five characters that could break out of the markup. Local and small on purpose —
/// the same choice [`adi_hive::notfound`] makes, and for the same reason.
fn escape(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for c in raw.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    use adi_hive::config::{Recreate, Rollout, ServiceProxy, ServiceSpec};
    use base64::Engine as _;
    use tokio::io::DuplexStream;

    use super::*;
    use crate::fleet::Grant;

    // -- fixtures -------------------------------------------------------------------------

    fn some_key() -> EndpointId {
        iroh::SecretKey::generate().public()
    }

    /// A route table built the way the node builds its own: from a hive spec, through
    /// [`adi_hive`]'s resolver. `(service name, host, path prefix, port)`.
    fn routes(services: &[(&str, &str, Option<&str>, u16)]) -> Routes {
        let mut hive = Hive::default();
        for (name, host, path, port) in services {
            hive.services.insert(
                (*name).to_string(),
                ServiceSpec {
                    proxy: Some(ServiceProxy {
                        host: (*host).to_string(),
                        path: path.map(str::to_string),
                    }),
                    rollout: Some(Rollout {
                        recreate: Some(Recreate {
                            ports: BTreeMap::from([("http".to_string(), *port)]),
                        }),
                    }),
                    ..ServiceSpec::default()
                },
            );
        }
        Routes::from_hive(&hive)
    }

    fn peer_named(petname: &str, grants: &[&str], user: &str, password: &str) -> Peer {
        let mut record = NodeRecord {
            key: some_key().to_string(),
            nickname: petname.to_string(),
            grants: grants
                .iter()
                .map(|g| g.parse::<Grant>().expect("a valid grant"))
                .collect(),
            ..NodeRecord::default()
        };
        record.set_password(user, password);
        Peer {
            petname: petname.to_string(),
            record,
        }
    }

    /// A node with a fixed registry and route table, so the decisions can be exercised without
    /// a config on disk or an endpoint bound.
    #[derive(Debug)]
    struct StubNode {
        peers: HashMap<EndpointId, Peer>,
        routes: Routes,
    }

    impl StubNode {
        fn new(routes: Routes) -> Self {
            Self {
                peers: HashMap::new(),
                routes,
            }
        }

        fn with_peer(mut self, key: EndpointId, peer: Peer) -> Self {
            self.peers.insert(key, peer);
            self
        }
    }

    impl NodeSide for StubNode {
        fn peer(&self, key: &EndpointId) -> Option<Peer> {
            self.peers.get(key).cloned()
        }

        fn resolve(&self, service: &str, target: &str) -> Option<SocketAddr> {
            self.routes.resolve(service, target)
        }

        fn realm(&self) -> String {
            "laptop-b".to_string()
        }
    }

    fn basic(user: &str, password: &str) -> String {
        let token = base64::engine::general_purpose::STANDARD.encode(format!("{user}:{password}"));
        format!("Authorization: Basic {token}\r\n")
    }

    fn get_head(host: &str, target: &str, extra: &str) -> String {
        format!("GET {target} HTTP/1.1\r\nHost: {host}\r\n{extra}\r\n")
    }

    /// A listening socket nothing will ever answer on — enough for `connect` to succeed.
    async fn idle_upstream() -> (TcpListener, u16) {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .await
            .expect("bind an ephemeral upstream");
        let port = listener.local_addr().expect("local addr").port();
        (listener, port)
    }

    /// A port with nothing behind it: bound to learn a free number, then released.
    async fn dead_port() -> u16 {
        let (listener, port) = idle_upstream().await;
        drop(listener);
        port
    }

    /// Drive [`negotiate`] over an in-memory pipe, returning what the node wrote back and the
    /// upstream it handed over (if any).
    async fn negotiate_over(
        node: &StubNode,
        peer: EndpointId,
        service: &str,
        head: &str,
    ) -> (Vec<u8>, Option<TcpStream>) {
        let (mut client, server) = tokio::io::duplex(64 * 1024);
        let (mut recv, mut send) = tokio::io::split(server);
        protocol::write_http_request(&mut client, service)
            .await
            .expect("write the frame");
        client
            .write_all(head.as_bytes())
            .await
            .expect("write the head");
        let upstream = negotiate(peer, node, &mut send, &mut recv)
            .await
            .expect("negotiate");
        (slurp(&mut client, 1).await, upstream)
    }

    /// Read what is available, waiting until at least `at_least` bytes have arrived.
    async fn slurp(stream: &mut DuplexStream, at_least: usize) -> Vec<u8> {
        let mut out = Vec::new();
        let mut chunk = [0u8; 4096];
        while out.len() < at_least {
            let read = tokio::time::timeout(Duration::from_secs(2), stream.read(&mut chunk))
                .await
                .expect("a reply within the timeout")
                .expect("read");
            if read == 0 {
                break;
            }
            out.extend_from_slice(&chunk[..read]);
        }
        out
    }

    // -- C6: who may reach what -----------------------------------------------------------

    #[test]
    fn an_unknown_peer_is_not_authorized() {
        let node = StubNode::new(routes(&[("nosh", "nosh.adi", None, 8010)]));
        let verdict = admit(&node, &some_key(), "nosh");
        assert_eq!(verdict.expect_err("a stranger"), HttpStatus::NotAuthorized);
    }

    #[test]
    fn a_peer_without_a_grant_for_the_service_is_not_authorized() {
        let key = some_key();
        let node = StubNode::new(routes(&[("nosh", "nosh.adi", None, 8010)]))
            .with_peer(key, peer_named("laptop-a", &["http:app"], "igor", "pw"));
        assert_eq!(
            admit(&node, &key, "nosh").expect_err("no grant covers `nosh`"),
            HttpStatus::NotAuthorized
        );
        assert!(admit(&node, &key, "app").is_err(), "`app` is not routed here");
    }

    #[test]
    fn a_peer_with_no_grants_at_all_is_denied() {
        // Default-deny: unlike the forward role's peer list, empty never means "everyone".
        let key = some_key();
        let node = StubNode::new(routes(&[("nosh", "nosh.adi", None, 8010)]))
            .with_peer(key, peer_named("laptop-a", &[], "igor", "pw"));
        assert_eq!(
            admit(&node, &key, "nosh").expect_err("no grants"),
            HttpStatus::NotAuthorized
        );
    }

    #[test]
    fn a_wildcard_http_grant_covers_every_label() {
        let key = some_key();
        let node = StubNode::new(routes(&[
            ("nosh", "nosh.adi", None, 8010),
            ("app", "app.adi", None, 8000),
        ]))
        .with_peer(key, peer_named("laptop-a", &["http:*"], "igor", "pw"));
        for service in ["nosh", "app"] {
            assert!(admit(&node, &key, service).is_ok(), "{service} is granted");
        }
    }

    #[test]
    fn the_grant_check_precedes_the_service_lookup() {
        // Otherwise a stranger could enumerate a node's services by watching `ServiceUnknown`
        // and `NotAuthorized` differ.
        let node = StubNode::new(routes(&[("nosh", "nosh.adi", None, 8010)]));
        assert_eq!(
            admit(&node, &some_key(), "there-is-no-such-service")
                .expect_err("a stranger, whatever they ask for"),
            HttpStatus::NotAuthorized
        );
    }

    #[test]
    fn an_authorized_peer_resolves_to_the_services_local_port() {
        let key = some_key();
        let node = StubNode::new(routes(&[("nosh", "nosh.adi", None, 8010)]))
            .with_peer(key, peer_named("laptop-a", &["http:nosh"], "igor", "pw"));
        let (peer, addr) = admit(&node, &key, "nosh").expect("granted and routed");
        assert_eq!(peer.petname, "laptop-a");
        assert_eq!(addr, SocketAddr::from((Ipv4Addr::LOCALHOST, 8010)));
    }

    #[test]
    fn a_label_this_node_does_not_serve_is_service_unknown() {
        let key = some_key();
        let node = StubNode::new(routes(&[("nosh", "nosh.adi", None, 8010)]))
            .with_peer(key, peer_named("laptop-a", &["http:*"], "igor", "pw"));
        assert_eq!(
            admit(&node, &key, "ghost").expect_err("nothing serves `ghost`"),
            HttpStatus::ServiceUnknown
        );
    }

    #[test]
    fn a_label_resolves_through_the_nodes_own_hive_table() {
        // The label names the *local* `<service>.adi` host, which is how the same service is
        // reachable on the node itself (docs/fleet.md §1).
        let table = routes(&[
            ("nosh", "nosh.adi", None, 8010),
            ("api", "nosh.adi", Some("/api"), 8011),
        ]);
        assert_eq!(
            table.resolve("nosh", "/"),
            Some(SocketAddr::from((Ipv4Addr::LOCALHOST, 8010)))
        );
        assert_eq!(
            table.resolve("nosh", "/api/tasks?x=1"),
            Some(SocketAddr::from((Ipv4Addr::LOCALHOST, 8011))),
            "the longest matching prefix wins, exactly as at the front door"
        );
        assert_eq!(table.resolve("nothing", "/"), None);
    }

    // -- C6: the stream, end to end -------------------------------------------------------

    #[tokio::test]
    async fn an_authorized_request_reaches_the_service_with_its_head_verbatim() {
        let key = some_key();
        let (listener, port) = idle_upstream().await;
        let node = StubNode::new(routes(&[("nosh", "nosh.adi", None, port)]))
            .with_peer(key, peer_named("laptop-a", &["http:nosh"], "igor", "hunter2"));

        let head = get_head("nosh.laptop-b.n.adi", "/dash", &basic("igor", "hunter2"));
        let (reply, upstream) = negotiate_over(&node, key, "nosh", &head).await;

        assert_eq!(reply, vec![HttpStatus::Ok as u8]);
        assert!(upstream.is_some(), "the local service was handed over");

        let (mut served, _) = listener.accept().await.expect("the service was connected");
        let mut got = vec![0u8; head.len()];
        served.read_exact(&mut got).await.expect("the head arrived");
        assert_eq!(
            String::from_utf8_lossy(&got),
            head,
            "the head is forwarded byte for byte — `Host` and target untouched"
        );
    }

    #[tokio::test]
    async fn a_request_without_credentials_gets_a_401_on_an_ok_stream() {
        let key = some_key();
        let (listener, port) = idle_upstream().await;
        let node = StubNode::new(routes(&[("nosh", "nosh.adi", None, port)]))
            .with_peer(key, peer_named("laptop-a", &["http:nosh"], "igor", "hunter2"));

        let head = get_head("nosh.laptop-b.n.adi", "/", "");
        let (reply, upstream) = negotiate_over(&node, key, "nosh", &head).await;

        assert_eq!(reply[0], HttpStatus::Ok as u8, "the transport worked");
        let text = String::from_utf8_lossy(&reply[1..]).to_string();
        assert!(text.starts_with("HTTP/1.1 401 Unauthorized"), "{text}");
        assert!(
            text.contains(r#"WWW-Authenticate: Basic realm="laptop-b""#),
            "the challenge names the node: {text}"
        );
        assert!(upstream.is_none(), "nothing is spliced to the service");

        // The service saw the connection open and close without a byte of the request.
        let (mut served, _) = listener.accept().await.expect("connected before the gate");
        let mut sink = [0u8; 64];
        let read = tokio::time::timeout(Duration::from_millis(200), served.read(&mut sink))
            .await
            .expect("the upstream was dropped, so it sees EOF promptly")
            .expect("read");
        assert_eq!(read, 0, "the head never reached the service");
    }

    #[tokio::test]
    async fn a_wrong_password_is_challenged_too() {
        let key = some_key();
        let (_listener, port) = idle_upstream().await;
        let node = StubNode::new(routes(&[("nosh", "nosh.adi", None, port)]))
            .with_peer(key, peer_named("laptop-a", &["http:nosh"], "igor", "hunter2"));

        let head = get_head("nosh.laptop-b.n.adi", "/", &basic("igor", "wrong"));
        let (reply, upstream) = negotiate_over(&node, key, "nosh", &head).await;
        assert!(String::from_utf8_lossy(&reply[1..]).starts_with("HTTP/1.1 401"));
        assert!(upstream.is_none());
    }

    #[tokio::test]
    async fn an_upgrade_request_is_gated_like_any_other() {
        // A WebSocket handshake is an ordinary request until the `101`, so it is gated by *not*
        // making an exception for it (docs/fleet.md §5).
        let key = some_key();
        let (_listener, port) = idle_upstream().await;
        let node = StubNode::new(routes(&[("nosh", "nosh.adi", None, port)]))
            .with_peer(key, peer_named("laptop-a", &["http:nosh"], "igor", "hunter2"));

        let head = get_head(
            "nosh.laptop-b.n.adi",
            "/ws",
            "Upgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Version: 13\r\n",
        );
        let (reply, upstream) = negotiate_over(&node, key, "nosh", &head).await;
        assert!(String::from_utf8_lossy(&reply[1..]).starts_with("HTTP/1.1 401"));
        assert!(upstream.is_none());
    }

    #[tokio::test]
    async fn a_refused_peer_never_reaches_a_local_socket() {
        let (listener, port) = idle_upstream().await;
        let node = StubNode::new(routes(&[("nosh", "nosh.adi", None, port)]));

        let head = get_head("nosh.laptop-b.n.adi", "/", &basic("igor", "hunter2"));
        let (reply, upstream) = negotiate_over(&node, some_key(), "nosh", &head).await;

        assert_eq!(reply, vec![HttpStatus::NotAuthorized as u8]);
        assert!(upstream.is_none());
        let accepted = tokio::time::timeout(Duration::from_millis(100), listener.accept()).await;
        assert!(accepted.is_err(), "the service was never connected to");
    }

    #[tokio::test]
    async fn a_service_whose_port_is_dead_is_upstream_unavailable() {
        let key = some_key();
        let port = dead_port().await;
        let node = StubNode::new(routes(&[("nosh", "nosh.adi", None, port)]))
            .with_peer(key, peer_named("laptop-a", &["http:nosh"], "igor", "hunter2"));

        let head = get_head("nosh.laptop-b.n.adi", "/", &basic("igor", "hunter2"));
        let (reply, upstream) = negotiate_over(&node, key, "nosh", &head).await;
        assert_eq!(reply, vec![HttpStatus::UpstreamUnavailable as u8]);
        assert!(upstream.is_none());
    }

    #[tokio::test]
    async fn an_unknown_service_is_refused_before_anything_is_connected() {
        let key = some_key();
        let node = StubNode::new(routes(&[("nosh", "nosh.adi", None, 8010)]))
            .with_peer(key, peer_named("laptop-a", &["http:*"], "igor", "hunter2"));

        let head = get_head("ghost.laptop-b.n.adi", "/", &basic("igor", "hunter2"));
        let (reply, upstream) = negotiate_over(&node, key, "ghost", &head).await;
        assert_eq!(reply, vec![HttpStatus::ServiceUnknown as u8]);
        assert!(upstream.is_none());
    }

    #[tokio::test]
    async fn a_path_prefixed_request_reaches_the_backend_not_the_frontend() {
        // One origin per dashboard (docs/fleet.md §4) has to hold over the mesh too, or a page
        // that works under `nosh.adi` would 404 its own `/api` under `nosh.laptop-b.n.adi`.
        let key = some_key();
        let (frontend, frontend_port) = idle_upstream().await;
        let (backend, backend_port) = idle_upstream().await;
        let node = StubNode::new(routes(&[
            ("frontend", "nosh.adi", None, frontend_port),
            ("backend", "nosh.adi", Some("/api"), backend_port),
        ]))
        .with_peer(key, peer_named("laptop-a", &["http:nosh"], "igor", "hunter2"));

        let head = get_head("nosh.laptop-b.n.adi", "/api/tasks", &basic("igor", "hunter2"));
        let (reply, upstream) = negotiate_over(&node, key, "nosh", &head).await;
        assert_eq!(reply, vec![HttpStatus::Ok as u8]);
        assert!(upstream.is_some());

        let (mut served, _) = backend.accept().await.expect("the backend was connected");
        let mut got = vec![0u8; head.len()];
        served.read_exact(&mut got).await.expect("the head arrived");
        assert_eq!(String::from_utf8_lossy(&got), head);
        // The frontend was only probed (the fallback route) and dropped, never written to.
        let (mut probed, _) = frontend.accept().await.expect("probed");
        let mut sink = [0u8; 8];
        assert_eq!(probed.read(&mut sink).await.expect("read"), 0);
    }

    // -- C5: the pool ---------------------------------------------------------------------

    #[derive(Debug)]
    struct StubConn {
        alive: AtomicBool,
    }

    #[derive(Debug, Default)]
    struct CountingDialer {
        dials: AtomicUsize,
        fail: bool,
        delay: Duration,
    }

    impl CountingDialer {
        fn count(&self) -> usize {
            self.dials.load(Ordering::SeqCst)
        }
    }

    impl Dialer for CountingDialer {
        type Conn = StubConn;

        async fn dial(&self, _peer: EndpointId) -> anyhow::Result<StubConn> {
            self.dials.fetch_add(1, Ordering::SeqCst);
            if !self.delay.is_zero() {
                tokio::time::sleep(self.delay).await;
            }
            anyhow::ensure!(!self.fail, "stub dial failure");
            Ok(StubConn {
                alive: AtomicBool::new(true),
            })
        }

        fn is_usable(&self, conn: &StubConn) -> bool {
            conn.alive.load(Ordering::SeqCst)
        }
    }

    #[tokio::test]
    async fn concurrent_callers_to_one_peer_cause_exactly_one_dial() {
        let pool = Arc::new(Pool::new(CountingDialer {
            delay: Duration::from_millis(20),
            ..CountingDialer::default()
        }));
        let peer = some_key();

        let calls: Vec<_> = (0..8)
            .map(|_| {
                let pool = Arc::clone(&pool);
                tokio::spawn(async move { pool.get(peer).await.expect("dialled") })
            })
            .collect();
        let mut conns = Vec::new();
        for call in calls {
            conns.push(call.await.expect("task"));
        }

        assert_eq!(pool.dialer.count(), 1, "the queue behind the first dial");
        assert!(
            conns.windows(2).all(|w| Arc::ptr_eq(&w[0], &w[1])),
            "every caller got the same connection, and streams multiplex over it"
        );
    }

    #[tokio::test]
    async fn a_closed_connection_is_redialled() {
        let pool = Pool::new(CountingDialer::default());
        let peer = some_key();

        let first = pool.get(peer).await.expect("dialled");
        assert_eq!(pool.dialer.count(), 1);
        assert!(
            Arc::ptr_eq(&pool.get(peer).await.expect("reused"), &first),
            "a live connection is handed out again, not re-dialled"
        );
        assert_eq!(pool.dialer.count(), 1);

        first.alive.store(false, Ordering::SeqCst);
        let second = pool.get(peer).await.expect("re-dialled");
        assert_eq!(pool.dialer.count(), 2);
        assert!(!Arc::ptr_eq(&second, &first));
    }

    #[tokio::test]
    async fn a_failed_dial_backs_off_instead_of_spinning() {
        let pool = Pool::with_backoff(
            CountingDialer {
                fail: true,
                ..CountingDialer::default()
            },
            Duration::from_millis(80),
            Duration::from_millis(200),
        );
        let peer = some_key();

        assert!(pool.get(peer).await.is_err(), "the dial fails");
        for _ in 0..5 {
            assert!(pool.get(peer).await.is_err(), "still failing");
        }
        assert_eq!(
            pool.dialer.count(),
            1,
            "a hot retry loop never reaches the network"
        );

        tokio::time::sleep(Duration::from_millis(160)).await;
        assert!(pool.get(peer).await.is_err());
        assert_eq!(pool.dialer.count(), 2, "once the backoff expires, it retries");
    }

    #[test]
    fn the_backoff_doubles_and_is_capped() {
        let base = Duration::from_millis(500);
        let cap = Duration::from_secs(30);
        assert_eq!(backoff_delay(0, base, cap), Duration::ZERO);
        assert_eq!(backoff_delay(1, base, cap), base);
        assert_eq!(backoff_delay(2, base, cap), base * 2);
        assert_eq!(backoff_delay(3, base, cap), base * 4);
        assert_eq!(backoff_delay(9, base, cap), cap);
        assert_eq!(backoff_delay(u32::MAX, base, cap), cap, "no overflow wrap");
    }

    // -- head parsing ---------------------------------------------------------------------

    #[tokio::test]
    async fn read_head_stops_at_the_blank_line() {
        let request = b"GET / HTTP/1.1\r\nHost: nosh.laptop-b.n.adi\r\n\r\nbody-bytes";
        let mut cursor = std::io::Cursor::new(request.to_vec());
        let head = read_head(&mut cursor).await.expect("read");
        assert!(
            head.starts_with(b"GET / HTTP/1.1\r\n"),
            "the head is returned as it arrived"
        );
        assert_eq!(
            header_value(&head, "host").as_deref(),
            Some("nosh.laptop-b.n.adi")
        );
    }

    #[test]
    fn the_host_header_is_matched_case_insensitively_and_the_request_line_skipped() {
        let head = b"GET /x HTTP/1.1\r\nhOsT:  app.laptop-b.n.adi \r\nAccept: */*\r\n\r\n";
        assert_eq!(
            header_value(head, "host").as_deref(),
            Some("app.laptop-b.n.adi")
        );
        assert_eq!(header_value(head, "x-missing"), None);
        assert_eq!(request_target(head).as_deref(), Some("/x"));
    }

    #[test]
    fn a_truncated_request_line_yields_no_target() {
        assert_eq!(request_target(b"GET /x\r\nHost: a.b.n.adi\r\n\r\n"), None);
    }

    // -- pages and wiring -----------------------------------------------------------------

    #[test]
    fn the_not_paired_page_names_the_node_and_the_key_to_pair() {
        let key = some_key().to_string();
        let page = not_paired_page("nosh.laptop-b.n.adi", "laptop-b", &key);
        assert!(page.contains("nosh.laptop-b.n.adi"), "the host as spelled");
        assert!(page.contains("laptop-b"), "the node as spelled");
        assert!(page.contains(&key), "the key an operator has to authorize");
        assert!(page.contains("not paired"));
    }

    #[test]
    fn a_refusal_page_carries_the_nodes_own_reason() {
        let key = some_key().to_string();
        let page = refused_page(
            "nosh.laptop-b.n.adi",
            "laptop-b",
            &key,
            HttpStatus::ServiceUnknown.reason(),
        );
        assert!(page.contains(HttpStatus::ServiceUnknown.reason()));
        assert!(page.contains(&key));
    }

    #[test]
    fn the_pages_escape_what_came_off_the_wire() {
        // A `Host` header is whatever the client wrote, so it reaches the markup escaped.
        let hostile = "<script>alert('x')</script>";
        for page in [
            not_paired_page(hostile, hostile, "key"),
            unreachable_page(hostile, hostile, "key", hostile),
            refused_page(hostile, hostile, "key", "why"),
            bad_request_page(hostile),
        ] {
            assert!(!page.contains("<script>"), "escaped: {page}");
            assert!(page.contains("&lt;script&gt;"));
        }
    }

    #[test]
    fn the_default_port_is_clear_of_the_managed_and_supervisor_bands() {
        // The ports manager allocates 8000..=9999 and reserves 15000..=15999 around ADI DNS.
        const { assert!(DEFAULT_PORT > 1023, "never a privileged port") }
        assert!(!(8000..=9999).contains(&DEFAULT_PORT));
        assert!(!(15000..=15999).contains(&DEFAULT_PORT));
        assert_eq!(default_addr().ip(), Ipv4Addr::LOCALHOST, "loopback only");
    }

    /// A freshly installed node has only the config `adi-core` generates — no hand-managed
    /// `hive/hive.yaml`. Resolving service labels against the canonical path alone left the route
    /// table empty there, so the node answered every request with "no such service" while looking
    /// perfectly healthy: paired, authorized, reachable. Found by running a real node in a
    /// container, invisible on a developer machine where both files exist.
    #[test]
    fn the_route_table_follows_the_front_door_a_node_actually_runs() {
        let root = std::env::temp_dir().join(format!(
            "adi-mesh-frontdoor-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        let store = adi_config::Config::with_root(&root);
        let hand_managed = root.join("hive").join("hive.yaml");

        // Neither present: no front door yet. Not an error — the machine simply serves nothing.
        assert_eq!(front_door_config_in(&store, &hand_managed), None);

        // Only the generated one — the node case that was broken.
        let generated = store
            .module(adi_config::FRONTDOOR_MODULE)
            .raw_path(adi_config::FRONTDOOR_CONFIG_FILE);
        std::fs::create_dir_all(generated.parent().expect("module dir")).expect("mkdir");
        std::fs::write(&generated, "version: \"1\"\n").expect("write generated");
        assert_eq!(
            front_door_config_in(&store, &hand_managed),
            Some(generated.clone())
        );

        // Both present: the hand-managed one wins — it is the one that imports projects and
        // dashboards, and it is what the front door is actually started with.
        std::fs::create_dir_all(hand_managed.parent().expect("hive dir")).expect("mkdir");
        std::fs::write(&hand_managed, "version: \"1\"\n").expect("write hand-managed");
        assert_eq!(
            front_door_config_in(&store, &hand_managed),
            Some(hand_managed.clone())
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    /// A dashboard declares no port — adi-hive leases one for it — so its route only exists once
    /// the leases are filled in. The gateway resolved the config *without* that step, and
    /// `resolve` silently dropped both of the dashboard's routes as "no HTTP port". The node then
    /// served its control panel (a port is declared for that one) and refused every dashboard
    /// with "no such service": the exact shape of a feature that looks finished and delivers
    /// nothing. Found by opening a real dashboard on a container node.
    #[test]
    fn a_service_whose_port_is_only_a_lease_still_gets_a_route() {
        let root = std::env::temp_dir().join(format!(
            "adi-mesh-lease-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("mkdir");
        let ports = adi_ports_manager::Ports::with_config(adi_ports_manager::Config {
            registry_path: root.join("registry.json"),
            ..adi_ports_manager::Config::default()
        });

        // Exactly a dashboard's shape: a proxied service with no `rollout.recreate.ports`.
        let path = root.join("hive.yaml");
        std::fs::write(
            &path,
            "services:\n  frontend:\n    proxy: { host: probe.adi }\n",
        )
        .expect("write");
        let mut hive = Hive::load(&path).expect("load");

        assert!(
            hive.resolve().routes.is_empty(),
            "without the lease there is no route — this is the bug, pinned"
        );

        let allocated = hive.allocate_missing_ports(&ports);
        assert_eq!(allocated.len(), 1, "the manager hands it a port: {allocated:?}");

        let routes = Routes::from_hive(&hive);
        assert!(
            routes.resolve("probe", "/").is_some(),
            "once the lease is filled in the label resolves, as it does at the front door"
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn the_listen_address_falls_back_when_the_override_is_unusable() {
        assert_eq!(addr_from_env(None), default_addr());
        assert_eq!(addr_from_env(Some("   ")), default_addr());
        assert_eq!(addr_from_env(Some("not-an-address")), default_addr());
        assert_eq!(
            addr_from_env(Some("127.0.0.1:14081")),
            SocketAddr::from((Ipv4Addr::LOCALHOST, 14081))
        );
    }

    #[test]
    fn the_realm_reaches_the_challenge_verbatim() {
        // Where the realm *comes from* is [`crate::node`]'s business (and is tested there, against
        // a temp store). What belongs here is that whatever the node side reports is what the `401`
        // actually challenges with — the two used to be separate strings, and a realm the browser
        // never showed would be a name nobody could act on.
        let node = StubNode::new(routes(&[]));
        let realm = node.realm();
        let challenge = auth::unauthorized_response(&realm);
        assert!(
            challenge.contains(&format!("realm=\"{realm}\"")),
            "{challenge}"
        );
    }
}
