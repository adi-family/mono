//! The mesh runtime as a controllable handle. [`Daemon::start`] binds the endpoint and
//! spawns the host + client tasks; [`Daemon::stop`] tears them down. It runs the same way
//! whether driven by the `adi-mesh run` binary or started in-process by the control panel —
//! either way the tasks live only as long as the handle, so nothing survives the owner.

use std::sync::Arc;
use std::time::Duration;

use iroh::endpoint::presets;
use iroh::{Endpoint, EndpointAddr, EndpointId};
use tokio::sync::watch;
use tokio::task::JoinHandle;
use tracing::{info, warn};

use crate::config::MeshConfig;
use crate::{client, host, identity, protocol, ticket};

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
    /// Load config + identity, bind the endpoint, and start the host + client roles.
    ///
    /// # Errors
    /// Fails if the config/identity can't be read or the endpoint can't bind.
    pub async fn start() -> anyhow::Result<Self> {
        let cfg = MeshConfig::load()?;
        let secret = identity::load_or_create()?;
        let endpoint = Endpoint::builder(presets::N0)
            .secret_key(secret)
            .alpns(vec![protocol::ALPN.to_vec()])
            .bind()
            .await?;
        let id = endpoint.id();
        info!(%id, "adi-mesh endpoint bound");

        let (shutdown, rx) = watch::channel(false);
        let mut tasks = Vec::new();

        // Publish this run's ticket once its relay is up, so tools can share it.
        tasks.push(tokio::spawn(publish_ticket(endpoint.clone())));

        let host_cfg = Arc::new(cfg.host.clone());
        tasks.push(tokio::spawn(host::serve(
            endpoint.clone(),
            host_cfg,
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
