//! adi-app — the adi application server.
//!
//! One process, two roles behind `app.adi` (adi-hive proxies the host here):
//! - `GET /` (and any non-`/api` path) serves the control-panel **SPA** (a single
//!   embedded HTML file).
//! - `/api/*` is the **Rust backend**: a JSON API over the live [`adi_ports_manager`]
//!   port registry.
//!
//! It listens on `$PORT` (injected by the adi-hive runner) or an explicit `addr`
//! argument, on loopback. Hand-rolled HTTP/1.1 (see [`http`]); no web framework.

mod api;
mod http;

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Instant;

use adi_ports_manager::Ports;
use tokio::net::{TcpListener, TcpStream};
use tracing::{debug, info, warn};

/// The SPA, embedded so the binary is self-contained (no runtime asset paths).
const INDEX_HTML: &str = include_str!("../web/index.html");

/// Fallback listen port when `$PORT` is unset and no `addr` argument is given.
const DEFAULT_PORT: u16 = 8090;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .init();

    let addr = listen_addr();
    let listener = TcpListener::bind(addr).await?;
    let local = listener.local_addr().unwrap_or(addr);
    let ports = Arc::new(Ports::new());
    let start = Instant::now();
    info!(%local, registry = %ports.config().registry_path.display(), "adi-app listening");

    loop {
        tokio::select! {
            accepted = listener.accept() => match accepted {
                Ok((stream, peer)) => {
                    let ports = Arc::clone(&ports);
                    tokio::spawn(async move {
                        if let Err(e) = handle(stream, &ports, start).await {
                            debug!(%peer, error = %e, "connection error");
                        }
                    });
                }
                Err(e) => warn!(error = %e, "accept failed"),
            },
            () = shutdown_signal() => {
                info!("shutdown signal received; stopping");
                break;
            }
        }
    }
    Ok(())
}

/// Resolve where to listen: an explicit `SocketAddr`/`host:port` argument wins, else
/// `127.0.0.1:$PORT`, else `127.0.0.1:DEFAULT_PORT`.
fn listen_addr() -> SocketAddr {
    if let Some(arg) = std::env::args().nth(1) {
        if let Ok(addr) = arg.parse::<SocketAddr>() {
            return addr;
        }
        if let Ok(port) = arg.parse::<u16>() {
            return SocketAddr::from(([127, 0, 0, 1], port));
        }
        warn!(arg = %arg, "ignoring unparseable listen argument");
    }
    let port = std::env::var("PORT")
        .ok()
        .and_then(|v| v.trim().parse::<u16>().ok())
        .unwrap_or(DEFAULT_PORT);
    SocketAddr::from(([127, 0, 0, 1], port))
}

/// Read one request, route it, and write the response.
async fn handle(mut stream: TcpStream, ports: &Ports, start: Instant) -> anyhow::Result<()> {
    let Some(req) = http::read_request(&mut stream).await? else {
        return Ok(()); // idle connection, peer closed
    };
    debug!(method = %req.method, path = %req.path, "request");

    let path = req.route_path();
    let (status, body) = match (req.method.as_str(), path) {
        ("GET", "/api/health") => api::health(start),
        ("GET", "/api/ports") => api::ports(ports),
        ("POST", "/api/ports/reserve") => api::reserve(ports, &req.body),
        ("POST", "/api/ports/release") => api::release(ports, &req.body),
        (_, p) if p.starts_with("/api") => api::error(404, "no such API endpoint"),
        // SPA fallback: any other GET serves the app shell (client-side routing).
        ("GET", _) => return http::write_html(&mut stream, 200, INDEX_HTML).await,
        _ => api::error(405, "method not allowed"),
    };
    http::write_json(&mut stream, status, &body).await
}

async fn shutdown_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        let Ok(mut term) = signal(SignalKind::terminate()) else {
            return futures_pending().await;
        };
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {},
            _ = term.recv() => {},
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}

/// Never resolves — used if the SIGTERM handler can't be installed, so the accept loop
/// keeps running rather than busy-looping on an immediately-ready shutdown branch.
#[cfg(unix)]
async fn futures_pending() {
    std::future::pending::<()>().await;
}
