//! The local channel client. Hive owns this process, its port, and its internal hostname.

mod live;

use std::collections::BTreeSet;
use std::net::{Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

use adi_agents::Agents;
use adi_channels::{Connections, RouterClient};
use adi_config::Config;
use adi_events::Events;
use adi_secrets::Secrets;
use adi_webapp_api::handlers::Response;
use adi_webapp_api::{handlers, http, origin};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{Mutex, watch};
use tracing::{info, warn};

struct Service {
    connections: Connections,
    secrets: Secrets,
    agents: Agents,
    events: Events,
    router_url: String,
    admin_secret: Option<String>,
    live: live::Live,
    mutations: Mutex<()>,
}

impl Service {
    fn new(config: Config, router_url: String, admin_secret: Option<String>) -> Self {
        Self {
            connections: Connections::with_config(config.clone()),
            secrets: Secrets::with_config(config.clone()),
            agents: Agents::with_config(config.clone()),
            events: Events::with_config(config),
            router_url,
            admin_secret,
            live: live::Live::default(),
            mutations: Mutex::new(()),
        }
    }

    /// Restore saved clients at startup and reconcile after connect/disconnect. Tokens and
    /// connection manifests are read on the blocking pool, like the API's other store access.
    async fn reconcile(self: &Arc<Self>) -> anyhow::Result<()> {
        let service = Arc::clone(self);
        let providers = tokio::task::spawn_blocking(move || -> adi_channels::Result<_> {
            let providers: BTreeSet<_> = service
                .connections
                .list()?
                .into_iter()
                .map(|c| c.manifest.provider)
                .collect();
            Ok(providers
                .into_iter()
                .map(|provider| {
                    let token = match adi_channels::token::load(&service.secrets, &provider) {
                        Ok(token) => token,
                        Err(error) => {
                            warn!(%provider, %error, "couldn't restore provider credentials");
                            None
                        }
                    };
                    (provider, token)
                })
                .collect::<Vec<_>>())
        })
        .await??;
        let active: Vec<_> = providers
            .iter()
            .filter(|(_, token)| token.is_some())
            .map(|(provider, _)| provider.clone())
            .collect();
        self.live.retain(&active).await;
        for (provider, token) in providers {
            let Some(node_token) = token else { continue };
            self.live
                .ensure(RouterClient {
                    router_url: self.router_url.clone(),
                    provider,
                    node_token,
                    connections: self.connections.clone(),
                    agents: self.agents.clone(),
                    events: self.events.clone(),
                    connected: Arc::new(AtomicBool::new(false)),
                })
                .await;
        }
        Ok(())
    }

    async fn answer(self: &Arc<Self>, req: http::Request) -> Response {
        if let Err(refusal) = origin::check(&req) {
            return handlers::error(refusal.status, &refusal.message);
        }
        if req.method == "GET" && matches!(req.route_path(), "/health" | "/api/health") {
            return handlers::ok_json(&serde_json::json!({
                "ok": true, "service": "adi-channelsd", "url": adi_channels::service::url(),
                "pid": std::process::id(),
            }));
        }
        if req.method == "GET" && req.route_path() == "/api/channels/status" {
            // Include configured providers even when their credentials are unavailable.
            let service = Arc::clone(self);
            let listed = tokio::task::spawn_blocking(move || service.connections.list()).await;
            let mut status = self.live.status().await;
            if let Ok(Ok(connections)) = listed {
                for connection in connections {
                    status.entry(connection.manifest.provider).or_insert(false);
                }
            }
            return handlers::channel_status(status);
        }
        // Control requests have one owner. Reconciliation happens before another mutation
        // can remove a provider or add a connection while its old socket is stopping.
        let _mutation = if req.method == "POST" {
            Some(self.mutations.lock().await)
        } else {
            None
        };
        let reconcile = req.method == "POST"
            && matches!(
                req.route_path(),
                "/api/channels/connect" | "/api/channels/disconnect"
            );
        let service = Arc::clone(self);
        let response = blocking(move || service.dispatch(&req)).await;
        if reconcile
            && response.status == 200
            && let Err(error) = self.reconcile().await
        {
            warn!(%error, "couldn't reconcile channel sockets after changing connections");
        }
        response
    }

    fn dispatch(&self, req: &http::Request) -> Response {
        let store = &self.connections;
        match (req.method.as_str(), req.route_path()) {
            ("GET", "/api/channels") => handlers::channels(store),
            ("POST", "/api/channels/connect") => handlers::connect_channel(
                store,
                &self.secrets,
                store.config(),
                &self.router_url,
                self.admin_secret.as_deref(),
                &req.body,
            ),
            ("POST", "/api/channels/disconnect") => {
                handlers::disconnect_channel(store, &self.secrets, &self.router_url, &req.body)
            }
            ("POST", "/api/channels/route") => handlers::route_channel(store, &req.body),
            ("POST", "/api/channels/pause") => handlers::pause_channel(store, &req.body),
            ("POST", "/api/channels/allow") => handlers::allow_channel(store, &req.body),
            ("POST", "/api/channels/reply") => {
                handlers::reply_channel(store, &self.secrets, &self.router_url, &req.body)
            }
            ("GET", path) if path.starts_with("/api/channels/") => {
                handlers::channel(store, &path["/api/channels/".len()..])
            }
            _ => handlers::error(404, "no such channel service endpoint"),
        }
    }
}

async fn blocking(work: impl FnOnce() -> Response + Send + 'static) -> Response {
    tokio::task::spawn_blocking(work)
        .await
        .unwrap_or_else(|e| handlers::error(500, &format!("channel request failed: {e}")))
}

/// No fallback or independent allocation: a managed service must use the port Hive chose.
fn listen_addr(port: Option<&str>) -> anyhow::Result<SocketAddr> {
    let port: u16 = port
        .ok_or_else(|| anyhow::anyhow!("PORT is required; start adi-channelsd through Hive"))?
        .parse()
        .map_err(|_| anyhow::anyhow!("PORT must be a TCP port assigned by Hive"))?;
    anyhow::ensure!(
        port != 0,
        "PORT must be a nonzero TCP port assigned by Hive"
    );
    Ok(SocketAddr::from((Ipv4Addr::LOCALHOST, port)))
}

async fn serve(mut stream: TcpStream, service: Arc<Service>) -> anyhow::Result<()> {
    let Some(req) = http::read_request(&mut stream).await? else {
        return Ok(());
    };
    let response = service.answer(req).await;
    http::write_json(&mut stream, response.status, &response.body).await
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .with_target(false)
        .init();
    let addr = listen_addr(std::env::var("PORT").ok().as_deref())?;
    // Bind before starting sockets: a second accidental process cannot steal the provider
    // subscriptions from the Hive-owned instance already listening on its allocated port.
    let listener = TcpListener::bind(addr).await?;
    let config = Config::open();
    let db = adi_db::Db::with_config(config.clone());
    db.bootstrap()?;
    let tools = adi_tools::Tools::with_config(config.clone());
    if let Err(error) = adi_channels::tool::ensure(&tools) {
        warn!(%error, "couldn't install channel-reply tool");
    }
    let service = Arc::new(Service::new(
        config,
        adi_channels::config::router_url(),
        adi_channels::config::router_admin_secret(),
    ));
    if let Err(error) = service.reconcile().await {
        warn!(%error, "couldn't restore channel clients; will retry");
    }
    let forwarder = adi_channels::finished::QuestionForwarder::new(
        service.connections.clone(),
        service.agents.clone(),
        service.secrets.clone(),
        service.router_url.clone(),
    );
    let (stop_questions, mut questions_stopped) = watch::channel(false);
    let maintenance_service = Arc::clone(&service);
    let questions = tokio::spawn(async move {
        let mut forwarder = forwarder;
        let mut ticks = 0u8;
        loop {
            tokio::select! {
                _ = questions_stopped.changed() => break,
                () = tokio::time::sleep(Duration::from_secs(1)) => {},
            }
            ticks = (ticks + 1) % 5;
            if ticks == 0 {
                let _mutation = maintenance_service.mutations.lock().await;
                if let Err(error) = maintenance_service.reconcile().await {
                    warn!(%error, "couldn't reconcile channel clients; will retry");
                }
            }
            let result = tokio::task::spawn_blocking(move || {
                let result = forwarder.tick();
                (forwarder, result)
            })
            .await;
            match result {
                Ok((next, result)) => {
                    forwarder = next;
                    if let Err(error) = result {
                        warn!(%error, "couldn't forward pending channel questions");
                    }
                }
                Err(error) => {
                    warn!(%error, "question forwarding task failed");
                    break;
                }
            }
        }
    });
    info!(%addr, url = %adi_channels::service::url(), "channel service listening");
    loop {
        tokio::select! {
            accepted = listener.accept() => {
                let (stream, _) = accepted?;
                let service = Arc::clone(&service);
                tokio::spawn(async move {
                    if let Err(error) = serve(stream, service).await {
                        warn!(%error, "channel API connection failed");
                    }
                });
            },
            () = adi_osext::shutdown_signal() => break,
        }
    }
    let _ = stop_questions.send(true);
    service.live.stop_all().await;
    // A provider HTTP timeout should not hold up Hive's service shutdown indefinitely.
    let _ = tokio::time::timeout(Duration::from_secs(2), questions).await;
    Ok(())
}

#[cfg(test)]
mod tests;
