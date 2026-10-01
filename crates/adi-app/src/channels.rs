//! This node's live channel-router sockets: at most one [`adi_channels::RouterClient`] task per
//! provider that has a connection on it, keyed so `connect`/`disconnect` can start or stop one
//! without caring whether another route already did.
//!
//! Where the router lives and how this node authenticates to it are read from the environment —
//! there is no router running anywhere but a developer's own `wrangler dev` for as long as
//! `apps/channel-router` (ADI-MONO-120) is unreleased, so a compiled-in default would just be a
//! guess at a port nobody promised.

use std::collections::HashMap;

use adi_agents::Agents;
use adi_channels::{Connections, RouterClient};
use adi_events::Events;
use tokio::sync::{Mutex, watch};
use tracing::info;

/// `apps/channel-router`'s base address — `http://127.0.0.1:8787` is `wrangler dev`'s own
/// default, assumed here only because nothing else has ever been deployed to assume instead.
const ROUTER_URL_ENV: &str = "ADI_CHANNEL_ROUTER_URL";
const DEFAULT_ROUTER_URL: &str = "http://127.0.0.1:8787";

/// This node's copy of the router's `ROUTER_ADMIN_SECRET` (`docs/channels.md` §1/§8) — gates
/// minting a node token, never ordinary traffic. Empty by default, which will fail `/register`
/// against any router that actually checks it; there is nothing to default it to safely.
const ROUTER_ADMIN_SECRET_ENV: &str = "ADI_CHANNEL_ROUTER_ADMIN_SECRET";

#[must_use]
pub fn router_url() -> String {
    std::env::var(ROUTER_URL_ENV).unwrap_or_else(|_| DEFAULT_ROUTER_URL.to_string())
}

#[must_use]
pub fn router_admin_secret() -> String {
    std::env::var(ROUTER_ADMIN_SECRET_ENV).unwrap_or_default()
}

/// The running sockets, by provider. Cheap to hold in `App`; everything here is behind the async
/// mutex, the same shape [`crate::MeshCtl`] already uses for its own single long-lived task.
#[derive(Debug, Default)]
pub struct Live {
    shutdowns: Mutex<HashMap<String, watch::Sender<bool>>>,
}

impl Live {
    /// Start a socket for `provider` if one isn't already running.
    pub async fn ensure(
        &self,
        provider: &str,
        node_token: String,
        connections: Connections,
        agents: Agents,
        events: Events,
    ) {
        let mut shutdowns = self.shutdowns.lock().await;
        if shutdowns.contains_key(provider) {
            return;
        }
        let (tx, rx) = watch::channel(false);
        let client = RouterClient {
            router_url: router_url(),
            provider: provider.to_string(),
            node_token,
            connections,
            agents,
            events,
        };
        tokio::spawn(client.run(rx));
        info!(provider, "channel router client started");
        shutdowns.insert(provider.to_string(), tx);
    }

    /// Stop the socket for `provider`, if one is running — called once nothing on this node still
    /// has a connection on that provider.
    pub async fn stop(&self, provider: &str) {
        if let Some(tx) = self.shutdowns.lock().await.remove(provider) {
            let _ = tx.send(true);
            info!(provider, "channel router client stopped");
        }
    }

    /// Signal every running socket to close, without waiting — best-effort, the same shutdown
    /// shape the trigger supervisor and event dispatcher get, for the same reason: a process
    /// exiting should not leave a socket open past it on purpose.
    pub async fn stop_all(&self) {
        for tx in self.shutdowns.lock().await.values() {
            let _ = tx.send(true);
        }
    }
}
