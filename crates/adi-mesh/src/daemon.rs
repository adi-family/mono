//! The mesh runtime as a controllable handle. [`Daemon::start`] binds the endpoint and
//! spawns the host + client + gateway tasks; [`Daemon::stop`] tears them down. It runs the same
//! way whether driven by the `adi-mesh run` binary or started in-process by the control panel —
//! either way the tasks live only as long as the handle, so nothing survives the owner.
//!
//! The endpoint carries **three ALPNs**: the raw TCP forward that ssh and databases use, the
//! fleet's HTTP gateway (`docs/fleet.md` §7), and the [join](crate::join) handshake a node dials
//! out with to be paired (E3). They are dispatched by `conn.alpn()` inside the
//! one accept loop [`host::serve`] already owns, rather than by iroh's
//! [`protocol::Router`](iroh::protocol::Router) — see [`crate::gateway`] for why that choice went
//! the way it did.

use std::sync::Arc;
use std::time::Duration;

use iroh::endpoint::presets;
use iroh::{Endpoint, EndpointAddr, EndpointId};
use tokio::sync::watch;
use tokio::task::JoinHandle;
use tracing::{info, warn};

use crate::config::MeshConfig;
use crate::gateway::{self, Gateway, NodeCredentials};
use crate::{client, host, identity, join, protocol, relay, ticket};

/// How long to wait for a home relay before publishing a (possibly direct-only) ticket.
const TICKET_RELAY_WAIT: Duration = Duration::from_secs(8);

/// A running mesh: the bound endpoint, a shutdown switch, and the supervised tasks. Dropping
/// it aborts the tasks (and the endpoint); prefer [`stop`](Self::stop) for a clean teardown.
#[derive(Debug)]
pub struct Daemon {
    endpoint: Endpoint,
    shutdown: watch::Sender<bool>,
    tasks: Vec<JoinHandle<()>>,
    id: EndpointId,
}

impl Daemon {
    /// Load config + identity, bind the endpoint, and start the host + client roles, with no node
    /// passwords to spend — every node this machine calls will challenge for one.
    ///
    /// # Errors
    /// Fails if the config/identity can't be read or the endpoint can't bind.
    pub async fn start() -> anyhow::Result<Self> {
        Self::start_with(None).await
    }

    /// [`start`](Self::start), plus the node passwords this machine already holds
    /// ([`NodeCredentials`]). The control panel passes its store, so a browser here is not asked
    /// for a password the machine keeps; nothing else does, and nothing else changes.
    ///
    /// # Errors
    /// As [`start`](Self::start).
    pub async fn start_with(
        credentials: Option<Arc<dyn NodeCredentials>>,
    ) -> anyhow::Result<Self> {
        let cfg = MeshConfig::load()?;
        let secret = identity::load_or_create()?;
        let mut builder = Endpoint::builder(presets::N0)
            .secret_key(secret)
            // Every ALPN on one endpoint, so a machine keeps one identity and one relay session
            // whether a peer wants a raw port, a service (`docs/fleet.md` §7), or to be paired.
            // The join ALPN belongs here and not on a listener of its own: a viewer that had to
            // open something extra to accept a pairing is a viewer that can forget to close it.
            .alpns(vec![
                protocol::ALPN.to_vec(),
                protocol::HTTP_ALPN.to_vec(),
                join::ALPN.to_vec(),
            ]);
        // After the preset, never instead of it: `presets::N0` also brings the pkarr publisher and
        // the DNS lookup, and those stay whichever relays this machine calls home. Only the relay
        // half is being overridden here.
        if let Some(mode) = relay::relay_mode(&cfg.relays) {
            info!(relays = ?cfg.relays, "adi-mesh using configured relays");
            builder = builder.relay_mode(mode);
        }
        let endpoint = builder.bind().await?;
        let id = endpoint.id();
        info!(%id, "adi-mesh endpoint bound");

        let (shutdown, rx) = watch::channel(false);
        let mut tasks = Vec::new();

        // Publish this run's ticket once its relay is up, so tools can share it.
        tasks.push(tokio::spawn(publish_ticket(endpoint.clone())));

        // Both roles served over the endpoint share this: the forward loop dispatches the
        // gateway's ALPN to it, and the local listener below calls out through it.
        let gateway = Gateway::new(endpoint.clone());
        let gateway = Arc::new(match credentials {
            Some(credentials) => gateway.with_credentials(credentials),
            None => gateway,
        });

        let host_cfg = Arc::new(cfg.host.clone());
        tasks.push(tokio::spawn(host::serve(
            endpoint.clone(),
            host_cfg,
            Arc::clone(&gateway),
            rx.clone(),
        )));

        // The calling side. A gateway that cannot bind is a lost fleet, not a lost mesh: the
        // forwards and the host role keep running, and the front door's own "no gateway" page
        // explains the rest.
        let addr = gateway::configured_addr();
        match gateway::bind(addr).await {
            Ok(listener) => {
                info!(%addr, "mesh gateway listening for *.n.adi");
                tasks.push(tokio::spawn(gateway::serve(
                    listener,
                    Arc::clone(&gateway),
                    rx.clone(),
                )));
            }
            Err(e) => warn!(%addr, error = %e, "mesh gateway could not bind; no node is reachable from here"),
        }

        // Independently of the listener: the *node* side reads the same snapshots, so a machine
        // that only serves peers still picks up a new pairing without a restart.
        tasks.push(tokio::spawn(gateway::reload(
            Arc::clone(&gateway),
            rx.clone(),
        )));

        if cfg.forwards.is_empty() {
            info!("no forwards configured");
        }
        for forward in cfg.forwards {
            tasks.push(tokio::spawn(client::run(
                endpoint.clone(),
                forward,
                rx.clone(),
            )));
        }

        info!(%id, "adi-mesh ready");
        Ok(Self {
            endpoint,
            shutdown,
            tasks,
            id,
        })
    }

    /// This machine's [`EndpointId`].
    #[must_use]
    pub fn endpoint_id(&self) -> EndpointId {
        self.id
    }

    /// Signal every task to stop, wait for them, clear the published ticket, and close the
    /// endpoint. After this, nothing from this daemon is left running.
    pub async fn stop(self) {
        let _ = self.shutdown.send(true);
        ticket::clear_published();
        for task in self.tasks {
            let _ = task.await;
        }
        self.endpoint.close().await;
    }
}

/// Bind an endpoint just long enough to learn this machine's current address, and return
/// its shareable ticket. Used by the `adi-mesh ticket` command; does not start the roles.
///
/// # Errors
/// Fails if the identity can't be read, the endpoint can't bind, or encoding fails.
pub async fn current_ticket() -> anyhow::Result<String> {
    let secret = identity::load_or_create()?;
    let endpoint = Endpoint::builder(presets::N0)
        .secret_key(secret)
        .alpns(vec![protocol::ALPN.to_vec()])
        .bind()
        .await?;
    let addr = wait_for_relay_addr(&endpoint).await;
    let token = ticket::encode(&addr)?;
    endpoint.close().await;
    Ok(token)
}

/// Publish + log this run's ticket once its relay is up.
async fn publish_ticket(endpoint: Endpoint) {
    let addr = wait_for_relay_addr(&endpoint).await;
    match ticket::encode(&addr) {
        Ok(token) => {
            if let Err(e) = ticket::publish(&token) {
                warn!(error = %e, "could not persist this machine's ticket");
            }
            info!(ticket = %token, "share this ticket with a peer to reach this machine");
        }
        Err(e) => warn!(error = %e, "could not encode this machine's ticket"),
    }
}

/// Poll the endpoint's address until it has a home relay, bounded by [`TICKET_RELAY_WAIT`],
/// so a shared ticket is reachable off-LAN (falling back to direct-only when offline).
async fn wait_for_relay_addr(endpoint: &Endpoint) -> EndpointAddr {
    let deadline = tokio::time::Instant::now() + TICKET_RELAY_WAIT;
    loop {
        let addr = endpoint.addr();
        if ticket::has_relay(&addr) || tokio::time::Instant::now() >= deadline {
            return addr;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}
