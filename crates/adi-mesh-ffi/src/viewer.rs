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
use std::net::{Ipv4Addr, SocketAddr};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use adi_mesh::fleet::{FleetRegistry, Grant, Scope};
use adi_mesh::gateway::{IrohDialer, Pool};
use adi_mesh::protocol::{self, HttpStatus};
use adi_mesh::config::MeshConfig;
use adi_mesh::{identity, join, relay, ticket, tunnel};
use anyhow::Context as _;
use iroh::endpoint::presets;
use iroh::{Endpoint, EndpointId};
use serde::Serialize;
use tokio::io::AsyncWriteExt as _;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::watch;
use tokio::time::timeout;
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

/// Everything shared with the spawned tasks.
#[derive(Debug)]
struct Shared {
    /// Held so the pool can be rebuilt over the same endpoint. See [`Self::reset_pool`].
    endpoint: Endpoint,
    /// This run's ticket, once the relay is known. `None` until then, which is also the honest
    /// answer to "can a node reach me yet?".
    ticket: Mutex<Option<String>>,
    /// Pairings not yet collected by the UI.
    pairings: Mutex<Vec<Paired>>,
    /// The connection pool, swappable so a connection the OS froze can be retired without
    /// disturbing the listeners that use it. See [`Self::reset_pool`].
    pool: Mutex<Arc<Pool<IrohDialer>>>,
    /// `(node, service)` → the loopback port serving it, for listeners already bound this run.
    open: Mutex<HashMap<(String, String), u16>>,
}

impl Shared {
    fn pool(&self) -> Arc<Pool<IrohDialer>> {
        Arc::clone(&self.pool.lock().unwrap_or_else(PoisonError::into_inner))
    }

    /// Throw the pool away, so the next request dials fresh.
    ///
    /// A whole new [`Pool`] rather than an eviction, because what has to go is not one entry but
    /// the pool's memory of every peer: after a suspension *all* of its connections are dead, and
    /// each still reports itself usable ([`iroh`] only learns otherwise once its idle timeout
    /// fires). A listener already holding the old pool finishes its request against it and picks
    /// this one up on the next.
    fn reset_pool(&self) {
        *self.pool.lock().unwrap_or_else(PoisonError::into_inner) =
            Arc::new(Pool::new(IrohDialer::new(self.endpoint.clone())));
    }
}

/// A running viewer: the endpoint, the tasks it spawned, and the runtime they live on.
#[derive(Debug)]
pub struct Viewer {
    /// Owned so the tasks below outlive the call that started them, and die with this value.
    rt: tokio::runtime::Runtime,
    endpoint: Endpoint,
    shared: Arc<Shared>,
    shutdown: watch::Sender<bool>,
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

        let endpoint = rt.block_on(async {
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
            let relays = MeshConfig::load().map(|cfg| cfg.relays).unwrap_or_default();
            if let Some(mode) = relay::relay_mode(&relays) {
                info!(?relays, "adi-mesh viewer using configured relays");
                builder = builder.relay_mode(mode);
            }
            builder.bind().await.context("binding the mesh endpoint")
        })?;
        let key = endpoint.id();
        info!(%key, "adi-mesh viewer bound");

        let shared = Arc::new(Shared {
            endpoint: endpoint.clone(),
            ticket: Mutex::new(None),
            pairings: Mutex::new(Vec::new()),
            pool: Mutex::new(Arc::new(Pool::new(IrohDialer::new(endpoint.clone())))),
            open: Mutex::new(HashMap::new()),
        });
        let (shutdown, rx) = watch::channel(false);

        rt.spawn(publish_ticket(endpoint.clone(), Arc::clone(&shared)));
        rt.spawn(accept_pairings(
            endpoint.clone(),
            Arc::clone(&shared),
            rx.clone(),
        ));

        Ok(Self {
            rt,
            endpoint,
            shared,
            shutdown,
            key,
        })
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
            Arc::clone(&self.shared),
            key,
            service.to_string(),
            self.shutdown.subscribe(),
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
        self.rt
            .block_on(async move { catalog::fetch(&shared, key, &me, (username, password), &grants).await })
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
        let petname = self
            .rt
            .block_on(async move { catalog::allow(&shared, key, &me, service, (username, password)).await })?;
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

    /// Retire every pooled connection, so the next request dials fresh.
    ///
    /// Called when the app returns to the foreground. iOS freezes the process, and a QUIC
    /// connection that was alive when it went away is usually dead when it comes back — but it can
    /// still *look* open until an idle timeout fires, and a request handed that connection stalls
    /// for the length of the timeout instead of reconnecting.
    pub fn resume(&self) {
        self.shared.reset_pool();
        debug!("connection pool reset after foregrounding");
    }

    /// Stop the tasks, clear the published ticket, and close the endpoint.
    pub fn stop(self) {
        let _ = self.shutdown.send(true);
        ticket::clear_published();
        self.rt.block_on(self.endpoint.close());
    }
}

// ---------------------------------------------------------------------------------------
// The tasks
// ---------------------------------------------------------------------------------------

/// Wait for a relay, then record and publish this run's ticket.
async fn publish_ticket(endpoint: Endpoint, shared: Arc<Shared>) {
    let deadline = tokio::time::Instant::now() + RELAY_WAIT;
    let addr = loop {
        let addr = endpoint.addr();
        if ticket::has_relay(&addr) || tokio::time::Instant::now() >= deadline {
            break addr;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    };
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
async fn serve_service(
    listener: TcpListener,
    shared: Arc<Shared>,
    key: EndpointId,
    service: String,
    mut shutdown: watch::Receiver<bool>,
) {
    loop {
        tokio::select! {
            _ = shutdown.changed() => return,
            accepted = listener.accept() => match accepted {
                Ok((tcp, _)) => {
                    let shared = Arc::clone(&shared);
                    let service = service.clone();
                    tokio::spawn(async move {
                        if let Err(e) = proxy_one(tcp, &shared, key, &service).await {
                            debug!(%service, error = %e, "request ended with an error");
                        }
                    });
                }
                Err(e) => {
                    warn!(%service, error = %e, "accept failed");
                    tokio::time::sleep(ACCEPT_ERROR_BACKOFF).await;
                }
            }
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
    shared: &Shared,
    key: EndpointId,
    service: &str,
) -> anyhow::Result<()> {
    // Everything up to the first byte of HTTP is bounded (see [`STEP_TIMEOUT`]); the splice that
    // follows is not, because by then the two ends are talking and a long-lived stream — a
    // WebSocket, a log tail — is the point rather than a stall.
    let opened = timeout(STEP_TIMEOUT, async {
        let conn = shared.pool().get(key).await?;
        let (mut send, mut recv) = conn.open_bi().await?;
        protocol::write_http_request(&mut send, service).await?;
        let status = protocol::read_http_status(&mut recv).await?;
        anyhow::Ok((send, recv, status))
    })
    .await;

    let (send, recv, status) = match opened {
        Ok(Ok(opened)) => opened,
        // A failure *or* a timeout means the pooled connection cannot be trusted: after the app is
        // suspended every one of them is dead while still reporting itself usable, so leaving them
        // in place would make the next request hang exactly as this one did.
        Ok(Err(e)) => {
            shared.reset_pool();
            warn!(%service, error = %e, "cannot reach the node");
            return gateway_error(
                &mut tcp,
                "This node is not reachable right now.",
                &e.to_string(),
            )
            .await;
        }
        Err(_) => {
            shared.reset_pool();
            warn!(%service, "timed out reaching the node");
            return gateway_error(
                &mut tcp,
                "This node did not answer.",
                "The connection timed out. If the app has just come back from the background, \
                 reload — the next attempt dials fresh.",
            )
            .await;
        }
    };

    match status {
        HttpStatus::Ok => {
            tunnel::splice(tcp, send, recv).await;
            Ok(())
        }
        refused => {
            info!(%service, reason = refused.reason(), "node refused");
            gateway_error(&mut tcp, "The node refused this service.", refused.reason()).await
        }
    }
}

/// Answer the browser with a `502` when the mesh could not carry the request.
///
/// Plain and small on purpose: this page is read on a phone, by whoever just tapped a service, and
/// its whole job is to distinguish "the node is off" from "you may not have this".
async fn gateway_error(tcp: &mut TcpStream, headline: &str, detail: &str) -> anyhow::Result<()> {
    let body = format!(
        "<!doctype html><meta charset=utf-8>\
         <meta name=viewport content=\"width=device-width,initial-scale=1\">\
         <title>Unreachable</title>\
         <style>body{{font:-apple-system-body,system-ui,sans-serif;margin:0;padding:2rem;\
         color:#111;background:#fff}}h1{{font-size:1.25rem;margin:0 0 .5rem}}\
         p{{color:#666;margin:0;line-height:1.5}}\
         @media(prefers-color-scheme:dark){{body{{color:#eee;background:#000}}p{{color:#999}}}}</style>\
         <h1>{headline}</h1><p>{detail}</p>"
    );
    let head = format!(
        "HTTP/1.1 502 Bad Gateway\r\nContent-Type: text/html; charset=utf-8\r\n\
         Content-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    tcp.write_all(head.as_bytes()).await?;
    tcp.write_all(body.as_bytes()).await?;
    tcp.shutdown().await?;
    Ok(())
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
/// every request on its own copy (`docs/fleet.md` §5). So a failure to write it is logged and
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
    use super::*;

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
        assert!(any, "http:* means the list is a starting point, not the whole");
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
