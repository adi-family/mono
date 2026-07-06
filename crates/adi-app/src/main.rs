//! adi-app — the adi application server behind `app.adi`: one process serving the
//! control-panel webapp at `GET /` and a JSON `/api/*` backend over [`adi_ports_manager`].
//! Listens on `$PORT` or an explicit `addr` argument, on loopback.
//!
//! The UI is the Leptos app [`adi-webapp`](../adi-webapp), compiled to wasm by Trunk. Its
//! `dist/` output is embedded here at build time; set `ADI_WEBAPP_DIST=/path/to/dist` to
//! serve those files from disk instead (a dev mode — rebuild the UI with `trunk build` and
//! refresh, no re-embed). The API handlers live in [`adi_webapp_api::handlers`] and share
//! their DTO types with that frontend.

mod http;
mod scan;

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use adi_ports_manager::Ports;
use adi_webapp_api::handlers;
use include_dir::{Dir, include_dir};
use tokio::net::{TcpListener, TcpStream};
use tracing::{debug, info, warn};

/// The webapp's Trunk build output, embedded so the binary is self-contained. Empty until
/// `trunk build` runs in `crates/adi-webapp`; [`serve_asset`] serves a placeholder when
/// `index.html` is absent.
static WEBAPP: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/../adi-webapp/dist");

/// Service identity reported at `/api/health`.
const SERVICE: &str = "adi-app";
const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Fallback listen port when `$PORT` is unset and no `addr` argument is given.
const DEFAULT_PORT: u16 = 8090;

/// Env var pointing at a webapp `dist/` to serve from disk instead of the embedded copy.
const DIST_ENV: &str = "ADI_WEBAPP_DIST";

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
    let webapp_dist = Arc::new(webapp_dist_override());
    let start = Instant::now();
    info!(%local, registry = %ports.config().registry_path.display(), "adi-app listening");
    if let Some(dir) = webapp_dist.as_ref() {
        info!(dist = %dir.display(), "serving webapp from disk (dev mode)");
    }

    loop {
        tokio::select! {
            accepted = listener.accept() => match accepted {
                Ok((stream, peer)) => {
                    let ports = Arc::clone(&ports);
                    let webapp_dist = Arc::clone(&webapp_dist);
                    tokio::spawn(async move {
                        if let Err(e) = handle(stream, &ports, start, webapp_dist.as_deref()).await {
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

/// Resolve where to listen: an explicit `addr` argument wins, else `$PORT`, else `DEFAULT_PORT`.
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
async fn handle(
    mut stream: TcpStream,
    ports: &Ports,
    start: Instant,
    dist: Option<&Path>,
) -> anyhow::Result<()> {
    let Some(req) = http::read_request(&mut stream).await? else {
        return Ok(());
    };
    debug!(method = %req.method, path = %req.path, "request");

    let path = req.route_path();
    let (status, body) = match (req.method.as_str(), path) {
        ("GET", "/api/health") => handlers::health(SERVICE, VERSION, start),
        ("GET", "/api/ports") => handlers::ports(ports),
        ("GET", "/api/ports/used") => handlers::used_ports(scan::listening_ports()),
        ("POST", "/api/ports/reserve") => handlers::reserve(ports, &req.body),
        ("POST", "/api/ports/release") => handlers::release(ports, &req.body),
        (_, p) if p.starts_with("/api") => handlers::error(404, "no such API endpoint"),
        // Any other GET serves a webapp asset, or the app shell for client-side routing.
        ("GET", p) => return serve_asset(&mut stream, p, dist).await,
        _ => handlers::error(405, "method not allowed"),
    };
    http::write_json(&mut stream, status, &body).await
}

/// Serve a webapp asset. With a disk override ([`DIST_ENV`]) set, files come from that
/// directory; otherwise from the embedded copy. Either way, an unknown path falls back to
/// the app shell (`index.html`) for client-side routing, and the placeholder if the webapp
/// isn't built yet.
async fn serve_asset(
    stream: &mut TcpStream,
    path: &str,
    dist: Option<&Path>,
) -> anyhow::Result<()> {
    let rel = match path.trim_start_matches('/') {
        "" => "index.html",
        other => other,
    };
    match dist {
        Some(dir) => serve_from_disk(stream, dir, rel).await,
        None => serve_embedded(stream, rel).await,
    }
}

/// Serve `rel` from the embedded `dist/`, falling back to the shell / placeholder.
async fn serve_embedded(stream: &mut TcpStream, rel: &str) -> anyhow::Result<()> {
    if let Some(file) = WEBAPP.get_file(rel) {
        return http::write_response(stream, 200, "OK", content_type(rel), file.contents()).await;
    }
    if let Some(index) = WEBAPP.get_file("index.html") {
        let html = "text/html; charset=utf-8";
        return http::write_response(stream, 200, "OK", html, index.contents()).await;
    }
    http::write_html(stream, 200, &placeholder_html()).await
}

/// Serve `rel` from a `dist/` directory on disk (the [`DIST_ENV`] dev mode), falling back
/// to the shell / placeholder.
async fn serve_from_disk(stream: &mut TcpStream, dir: &Path, rel: &str) -> anyhow::Result<()> {
    if is_safe_rel(rel)
        && let Ok(bytes) = tokio::fs::read(dir.join(rel)).await
    {
        return http::write_response(stream, 200, "OK", content_type(rel), &bytes).await;
    }
    if let Ok(bytes) = tokio::fs::read(dir.join("index.html")).await {
        let html = "text/html; charset=utf-8";
        return http::write_response(stream, 200, "OK", html, &bytes).await;
    }
    http::write_html(stream, 200, &placeholder_html()).await
}

/// Reject path traversal: `rel` has its leading `/` stripped already, so joining it to the
/// dist dir stays inside as long as no component is `..`.
fn is_safe_rel(rel: &str) -> bool {
    !rel.split('/').any(|c| c == "..")
}

/// The `ADI_WEBAPP_DIST` override, if it points at an existing directory.
fn webapp_dist_override() -> Option<PathBuf> {
    let dir = PathBuf::from(std::env::var_os(DIST_ENV)?);
    if dir.is_dir() {
        Some(dir)
    } else {
        warn!(dist = %dir.display(), "{DIST_ENV} is set but not a directory; ignoring");
        None
    }
}

/// The page shown when the webapp isn't built into `dist/` yet, styled with the shared
/// [`adi_css`] design system.
fn placeholder_html() -> String {
    format!(
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\">\
         <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\
         <title>adi-app</title>{style}</head>\
         <body><div class=\"adi-container\">\
         <h1>adi-app is running</h1>\
         <p class=\"adi-muted\">The web UI hasn't been built yet. Build it with:</p>\
         <pre class=\"adi-mono\">scripts/build-app.sh</pre>\
         <p class=\"adi-muted\">or run <code>trunk build</code> in <code>crates/adi-webapp</code>, \
         then <code>cargo build -p adi-app</code>.</p>\
         </div></body></html>",
        style = adi_css::style_tag(),
    )
}

/// Map a file name to a `Content-Type` by its extension; unknown types are served as bytes.
fn content_type(path: &str) -> &'static str {
    match path.rsplit('.').next() {
        Some("html") => "text/html; charset=utf-8",
        Some("js" | "mjs") => "text/javascript; charset=utf-8",
        Some("wasm") => "application/wasm",
        Some("css") => "text/css; charset=utf-8",
        Some("json" | "map") => "application/json; charset=utf-8",
        Some("svg") => "image/svg+xml",
        Some("ico") => "image/x-icon",
        Some("png") => "image/png",
        Some("woff2") => "font/woff2",
        _ => "application/octet-stream",
    }
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

/// Never resolves — keeps the accept loop alive if the SIGTERM handler can't be installed.
#[cfg(unix)]
async fn futures_pending() {
    std::future::pending::<()>().await;
}
