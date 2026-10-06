//! This node's channel-router sockets, with at most one client task per provider.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use adi_agents::Agents;
use adi_channels::{Connections, RouterClient};
use adi_events::Events;
use tokio::sync::{Mutex, watch};
use tracing::info;

const ROUTER_URL_ENV: &str = "ADI_CHANNEL_ROUTER_URL";
const DEFAULT_ROUTER_URL: &str = "http://127.0.0.1:8787";

/// Optional operator secret; ordinary nodes use open registration with rate limits and caps.
const ROUTER_ADMIN_SECRET_ENV: &str = "ADI_CHANNEL_ROUTER_ADMIN_SECRET";

#[must_use]
pub fn router_url() -> String {
    std::env::var(ROUTER_URL_ENV).unwrap_or_else(|_| DEFAULT_ROUTER_URL.to_string())
}

/// Empty values are treated as an absent secret.
#[must_use]
pub fn router_admin_secret() -> Option<String> {
    std::env::var(ROUTER_ADMIN_SECRET_ENV)
        .ok()
        .filter(|s| !s.is_empty())
}

#[derive(Debug, Default)]
pub struct Live {
    shutdowns: Mutex<HashMap<String, watch::Sender<bool>>>,
    // A synchronous mutex lets the blocking status handler read connection state.
    connected: std::sync::Mutex<HashMap<String, Arc<AtomicBool>>>,
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
        let connected = Arc::new(AtomicBool::new(false));
        self.connected
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(provider.to_string(), connected.clone());
        let client = RouterClient {
            router_url: router_url(),
            provider: provider.to_string(),
            node_token,
            connections,
            agents,
            events,
            connected,
        };
        tokio::spawn(client.run(rx));
        info!(provider, "channel router client started");
        shutdowns.insert(provider.to_string(), tx);
    }

    /// Stop a provider's socket once it has no remaining connections on this node.
    pub async fn stop(&self, provider: &str) {
        if let Some(tx) = self.shutdowns.lock().await.remove(provider) {
            let _ = tx.send(true);
            self.connected
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .remove(provider);
            info!(provider, "channel router client stopped");
        }
    }

    /// Signal all sockets to close without waiting for them.
    pub async fn stop_all(&self) {
        for tx in self.shutdowns.lock().await.values() {
            let _ = tx.send(true);
        }
    }

    /// False for absent clients and clients that have not completed their handshake.
    #[must_use]
    pub fn is_connected(&self, provider: &str) -> bool {
        self.connected
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(provider)
            .is_some_and(|c| c.load(Ordering::Relaxed))
    }
}
