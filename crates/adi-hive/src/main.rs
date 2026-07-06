//! adi-hive — the adi-family reverse proxy: routes inbound HTTP by `Host` header to a
//! local upstream (nginx-style), and launches + supervises each service's local
//! `runner` so those upstreams are actually alive. Foreground process; a supervisor
//! owns its lifecycle. Reads the `proxy:` and `runner:` slices of a hive spec.

mod config;
mod notfound;
mod proxy;
mod runner;
mod status;

use std::net::{IpAddr, Ipv4Addr};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use config::Hive;
use proxy::Router;
use tokio::net::TcpListener;
use tokio::task::JoinHandle;
use tracing::{info, warn};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .init();

    // The single hive config: an explicit path arg, else the canonical
    // ~/.adi/mono/hive/hive.yaml. A missing file is not fatal — fall back to
    // built-in defaults (bind 127.0.0.1:8080, no routes) so the daemon still runs.
    let path = std::env::args()
        .nth(1)
        .map_or_else(config::default_config_path, PathBuf::from);
    let mut hive = if path.exists() {
        info!(path = %path.display(), "loading hive config");
        Hive::load(&path)?
    } else {
        warn!(path = %path.display(), "no hive config; using built-in defaults (bind 127.0.0.1:8080, no routes)");
        Hive::default()
    };

    // Take ports from the ports manager (stable, registry-backed leases) rather than
    // hand-picking them in hive.yaml: the proxy's own front-door bind port, and any
    // service that doesn't declare one. Explicitly-configured ports/binds still win.
    let ports_manager = adi_ports_manager::Ports::new();
    for (service, port) in hive.allocate_missing_ports(&ports_manager) {
        info!(%service, port, "allocated service port from ports manager");
    }
    if let Some(port) = hive.allocate_bind_port(&ports_manager) {
        info!(port, "allocated front-door bind port from ports manager");
    }

    let resolved = hive.resolve();
    for skipped in &resolved.skipped {
        warn!(service = %skipped, "not routed: no HTTP port");
    }
    info!(binds = ?resolved.binds, routes = resolved.routes.len(), "starting adi-hive");
    let router = Arc::new(Router::new(&resolved.routes));

    // Bind each address independently: a failure (e.g. :80 needs root, or the port is
    // taken) is logged and skipped, not fatal, so the proxy still serves on the
    // addresses it can bind. Only bail if nothing bound at all.
    let mut bound = Vec::with_capacity(resolved.binds.len());
    let mut tasks: Vec<JoinHandle<()>> = Vec::new();
    for addr in &resolved.binds {
        ensure_loopback_alias(addr.ip());
        match TcpListener::bind(addr).await {
            Ok(listener) => {
                let local = listener.local_addr().unwrap_or(*addr);
                info!(%local, "listening");
                bound.push(local.to_string());
                let router = Arc::clone(&router);
                tasks.push(tokio::spawn(proxy::serve(listener, router)));
            }
            Err(e) => {
                warn!(%addr, error = %e, "could not bind (privileged port needs root, or in use?); skipping");
            }
        }
    }
    if tasks.is_empty() {
        anyhow::bail!("no proxy address could be bound");
    }

    // Status file sits beside the config in the writable mono namespace
    // (e.g. ~/.adi/mono/hive/status.json), overridable via ADI_HIVE_STATUS_FILE.
    let status_path = status::resolve_path(path.with_file_name("status.json"));
    let status = status::Status::new(bound, resolved.routes.len());
    match status::write(&status_path, &status) {
        Ok(()) => info!(path = %status_path.display(), "wrote status file"),
        Err(e) => warn!(error = %e, path = %status_path.display(), "could not write status file"),
    }

    // Launch and supervise the services' local runners so the proxied upstreams are
    // actually alive. Relative working dirs are anchored at the hive.yaml's directory.
    let base_dir = path
        .parent()
        .map_or_else(|| PathBuf::from("."), Path::to_path_buf);
    let runners = hive.runners(&base_dir);
    if runners.is_empty() {
        info!("no service runners declared");
    } else {
        info!(count = runners.len(), "supervising service runners");
    }
    let supervisor = runner::Supervisor::start(runners);

    info!("adi-hive ready");

    shutdown_signal().await;
    info!("shutdown signal received; stopping");
    // Stop the runners first (SIGTERM their process groups), bounded so a stuck child
    // can't hang shutdown; then tear down the proxy listeners.
    if tokio::time::timeout(TERM_TIMEOUT, supervisor.shutdown())
        .await
        .is_err()
    {
        warn!("timed out stopping runners");
    }
    for task in tasks {
        task.abort();
    }
    status::remove(&status_path);
    Ok(())
}

/// Upper bound on how long shutdown waits for all runners to stop.
const TERM_TIMEOUT: Duration = Duration::from_secs(20);

/// On macOS a non-`127.0.0.1` loopback address (e.g. the `127.0.0.53` front door) must
/// be aliased onto `lo0` before it can be bound; elsewhere the whole `127.0.0.0/8`
/// already routes to loopback. Best-effort — a failure here just makes the subsequent
/// bind fail, which is already handled non-fatally.
fn ensure_loopback_alias(ip: IpAddr) {
    if ip == IpAddr::V4(Ipv4Addr::LOCALHOST) {
        return; // always present
    }
    #[cfg(target_os = "macos")]
    {
        match std::process::Command::new("ifconfig")
            .args(["lo0", "alias", &ip.to_string(), "up"])
            .status()
        {
            Ok(s) if s.success() => info!(%ip, "aliased loopback address for proxy bind"),
            Ok(s) => warn!(%ip, code = ?s.code(), "ifconfig lo0 alias failed (need root?)"),
            Err(e) => warn!(%ip, error = %e, "could not run ifconfig to alias loopback"),
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = ip; // 127.0.0.0/8 is already loopback on Linux/Windows
    }
}

async fn shutdown_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        let mut term = signal(SignalKind::terminate()).expect("install SIGTERM handler");
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
