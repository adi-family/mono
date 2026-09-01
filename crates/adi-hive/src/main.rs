//! adi-hive — the adi-family reverse proxy: routes inbound HTTP by `Host` header to a
//! local upstream (nginx-style), and launches + supervises each service's local `runner`
//! so those upstreams are alive. Foreground process owned by a supervisor.
//!
//! A thin shell over the [`adi_hive`] library: this file owns sockets, processes and the reload
//! tick; every routing decision it makes comes from the library, so the mesh gateway resolves
//! hostnames through exactly the same table.

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use adi_hive::config::{self, Hive};
use adi_hive::proxy::{self, Router};
use adi_hive::{runner, status, tls};
use tokio::net::TcpListener;
use tokio::sync::watch;
use tokio::task::JoinHandle;
use tracing::{info, warn};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    adi_hive::logging::init_tracing();

    // A missing config is not fatal: fall back to built-in defaults so the daemon still runs.
    let path = std::env::args()
        .nth(1)
        .map_or_else(config::default_config_path, PathBuf::from);

    // Settle "is someone already serving this config?" *before* loading it. The load walks every
    // imported project, which is far too expensive to spend on a start that cannot succeed: under
    // a supervisor that keeps the job alive (launchd `KeepAlive`, systemd `Restart`) a duplicate
    // instance re-walks the whole tree, fails to bind, exits, and is restarted forever — burning a
    // core in `stat` and starving every other build on the machine of filesystem metadata. Two
    // supervisors for one config is not hypothetical: two launchd labels pointing at the same
    // hive.yaml is exactly how it happens, and neither one can see the other.
    if let Some(taken) = addresses_already_served(&path) {
        anyhow::bail!(
            "every address {} declares is already in use ({}); another adi-hive is serving this \
             config. Stop the duplicate instead of running two — on macOS `launchctl list | grep \
             adi` names the jobs.",
            path.display(),
            taken
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(", ")
        );
    }

    warn_if_config_is_user_writable(&path);

    let mut hive = if path.exists() {
        info!(path = %path.display(), "loading hive config");
        Hive::load(&path)?
    } else {
        warn!(path = %path.display(), "no hive config; using built-in defaults (bind 127.0.0.1:8080, no routes)");
        Hive::default()
    };

    // Take ports from the ports manager (stable, registry-backed leases); explicit config still wins.
    let ports_manager = adi_ports_manager::Ports::new();
    for (service, port) in hive.allocate_missing_ports(&ports_manager) {
        info!(%service, port, "allocated service port from ports manager");
    }
    if let Some(port) = hive.allocate_bind_port(&ports_manager) {
        info!(port, "allocated front-door bind port from ports manager");
    }

    let resolved = hive.resolve();
    // Each entry already carries its own reason (no HTTP port, or a claim on the reserved mesh
    // zone), so the line only has to surface it.
    for skipped in &resolved.skipped {
        warn!(service = %skipped, "not routed");
    }
    if let Some(gateway) = resolved.mesh_gateway {
        info!(%gateway, "routing *.n.adi to the local mesh gateway");
    }
    info!(binds = ?resolved.binds, routes = resolved.routes.len(), "starting adi-hive");
    // Serve the routing table through a watch channel so the reloader can hot-swap it — a service
    // added on disk with a `proxy.host` starts routing without a front-door restart (which would
    // drop `app.adi` and every other proxied host).
    let mut current = Arc::new(Router::new(&resolved.routes, resolved.mesh_gateway));
    let (route_tx, route_rx) = watch::channel(Arc::clone(&current));

    let mut bound = Vec::with_capacity(resolved.binds.len());
    let mut tasks: Vec<JoinHandle<()>> = Vec::new();
    bind_plain(&resolved, &route_rx, &mut bound, &mut tasks).await;
    bind_tls(&path, &resolved, &route_rx, &mut bound, &mut tasks).await;

    if tasks.is_empty() {
        // Name them: this is the last line before the process exits, and under a supervisor that
        // restarts it the same failure is about to repeat, so the address is the whole diagnosis.
        anyhow::bail!(
            "no proxy address could be bound (tried {}); a privileged port needs root, and an \
             address already in use means another instance has it",
            resolved
                .binds
                .iter()
                .chain(&resolved.tls_binds)
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(", ")
        );
    }

    // Status file sits beside the config, overridable via ADI_HIVE_STATUS_FILE. `bound` is cloned so
    // it can be re-written with the fresh route count when the table hot-swaps below.
    let status_path = status::resolve_path(path.with_file_name("status.json"));
    let status = status::Status::new(bound.clone(), resolved.routes.len());
    match status::write(&status_path, &status) {
        Ok(()) => info!(path = %status_path.display(), "wrote status file"),
        Err(e) => warn!(error = %e, path = %status_path.display(), "could not write status file"),
    }

    // Launch and supervise the services' local runners so the proxied upstreams are alive.
    let base_dir = path
        .parent()
        .map_or_else(|| PathBuf::from("."), Path::to_path_buf);
    let runners = hive.runners(&base_dir);
    if runners.is_empty() {
        info!("no service runners declared");
    } else {
        info!(count = runners.len(), "supervising service runners");
    }
    let mut supervisor = runner::Supervisor::start(runners);

    info!("adi-hive ready");

    // Watch the config (and everything it imports) so a service added on disk applies without a
    // restart: the supervisor reconciles runners, and the routing table is hot-swapped when a
    // `proxy.host` is added/changed/removed — both touch only what actually changed. Only the *bind*
    // addresses are still fixed at startup, so adding a new front-door bind (rare) needs a restart.
    let mut reload = tokio::time::interval(RELOAD_INTERVAL);
    reload.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    // Pinned outside the loop so a signal arriving between ticks is never missed.
    let shutdown = adi_osext::shutdown_signal();
    let replaced = binary_replaced();
    tokio::pin!(shutdown, replaced);
    loop {
        tokio::select! {
            () = &mut shutdown => {
                info!("shutdown signal received; stopping");
                break;
            }
            () = &mut replaced => {
                info!("binary replaced on disk; exiting so launchd respawns the new build");
                break;
            }
            _ = reload.tick() => {
                if let Some((specs, table)) = reload_config(&path, &ports_manager, &base_dir) {
                    let (started, stopped) = supervisor.reconcile(specs);
                    if started > 0 || stopped > 0 {
                        info!(started, stopped, total = supervisor.len(), "reloaded runners");
                    }
                    // Hot-swap the routing table when the host→upstream set changed, so a service
                    // added with a domain starts routing on the next connection — no restart, so
                    // `app.adi` and every other proxied host stay up.
                    if table != *current {
                        info!(routes = table.len(), "routes changed; hot-swapping the proxy table");
                        let status = status::Status::new(bound.clone(), table.len());
                        current = Arc::new(table);
                        route_tx.send_replace(Arc::clone(&current));
                        if let Err(e) = status::write(&status_path, &status) {
                            warn!(error = %e, "could not update status file after route change");
                        }
                    }
                }
            }
        }
    }
    // Stop the runners first (bounded, so a stuck child can't hang shutdown), then the listeners.
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

/// Every address this config declares, when **all** of them are already in use — the signature of
/// a second instance running the same hive.yaml.
///
/// Costs one file read and one bind attempt per address, which is what makes it usable before the
/// config load rather than after.
///
/// `None` whenever the answer is not certain, because refusing to start is the expensive mistake
/// here: a config that declares no address (the ports manager leases one), a file that will not
/// parse (the real load reports that properly), or *any* address that is free or failed for some
/// other reason — a missing loopback alias, say, which [`bind_plain`] repairs itself. Only a
/// definite "every one of them is taken" is worth acting on.
fn addresses_already_served(path: &Path) -> Option<Vec<SocketAddr>> {
    if !path.exists() {
        return None;
    }
    let hive = Hive::parse_no_imports(path).ok()?;
    let declared: Vec<SocketAddr> = hive
        .proxy
        .bind
        .iter()
        .chain(&hive.proxy.tls_bind)
        .copied()
        .collect();
    if declared.is_empty() {
        return None;
    }
    for addr in &declared {
        // std sets `SO_REUSEADDR`, which still refuses a second *listener* on a live address, so
        // `AddrInUse` here means someone is really serving it. The probe listener is dropped
        // immediately; the window that opens is harmless, since the real bind reports its own
        // failure either way.
        match std::net::TcpListener::bind(addr) {
            Err(e) if e.kind() == std::io::ErrorKind::AddrInUse => {}
            _ => return None,
        }
    }
    Some(declared)
}

/// Bind each plain-HTTP address independently: a failure (a privileged port without root, or one
/// already in use) is logged and skipped, not fatal — the caller bails only if *nothing* bound, so
/// one taken port never costs the front door the addresses it could still serve.
async fn bind_plain(
    resolved: &config::Resolved,
    route_rx: &watch::Receiver<Arc<Router>>,
    bound: &mut Vec<String>,
    tasks: &mut Vec<JoinHandle<()>>,
) {
    for addr in &resolved.binds {
        ensure_loopback_alias(addr.ip());
        match TcpListener::bind(addr).await {
            Ok(listener) => {
                let local = listener.local_addr().unwrap_or(*addr);
                info!(%local, "listening");
                bound.push(local.to_string());
                tasks.push(tokio::spawn(proxy::serve(listener, route_rx.clone())));
            }
            Err(e) => {
                warn!(%addr, error = %e, "could not bind (privileged port needs root, or in use?); skipping");
            }
        }
    }
}

/// Add the HTTPS front door, when the config asks for one, appending whatever bound to `bound` and
/// `tasks`. Everything about it is best-effort: a certificate we can't mint or a port we can't take
/// is a warning, never a reason to drop the plain-HTTP front door `app.adi` already depends on.
async fn bind_tls(
    path: &Path,
    resolved: &config::Resolved,
    route_rx: &watch::Receiver<Arc<Router>>,
    bound: &mut Vec<String>,
    tasks: &mut Vec<JoinHandle<()>>,
) {
    if resolved.tls_binds.is_empty() {
        return;
    }
    let hosts: Vec<String> = resolved.routes.iter().map(|r| r.host.clone()).collect();
    // The mesh SANs are minted once, at start: a node paired later is covered from the next start,
    // which is also when its petname reaches `proxy.mesh_nodes`.
    let mesh = if resolved.mesh_gateway.is_some() {
        Some(resolved.mesh_nodes.as_slice())
    } else {
        None
    };
    let ready = match tls::prepare(&path.with_file_name("tls"), &hosts, mesh) {
        Ok(ready) => ready,
        Err(e) => {
            warn!(error = %e, "TLS unavailable; serving plain HTTP only");
            return;
        }
    };
    if ready.ca_is_new {
        warn!(
            ca = %ready.ca_path.display(),
            "generated a new local CA — nothing trusts it yet, so HTTPS will warn until you {}",
            tls::trust_hint(&ready.ca_path),
        );
    } else {
        info!(ca = %ready.ca_path.display(), "using the existing local CA");
    }
    let acceptor = tokio_rustls::TlsAcceptor::from(ready.config);
    for addr in &resolved.tls_binds {
        ensure_loopback_alias(addr.ip());
        match TcpListener::bind(addr).await {
            Ok(listener) => {
                let local = listener.local_addr().unwrap_or(*addr);
                info!(%local, "listening (TLS)");
                bound.push(format!("{local} (tls)"));
                tasks.push(tokio::spawn(proxy::serve_tls(
                    listener,
                    acceptor.clone(),
                    route_rx.clone(),
                )));
            }
            Err(e) => warn!(
                %addr, error = %e,
                "could not bind for TLS (privileged port needs root, or in use?); skipping"
            ),
        }
    }
}

/// Say so when a **root** hive is reading a config an ordinary user can write.
///
/// A warning and not a refusal, deliberately. sshd, sudo and cron all refuse in this situation,
/// and refusing is the stronger fix — but the config a root front door is pointed at *is*
/// `~/.adi/mono/hive/hive.yaml`, which is the user's own store file by design, so refusing would
/// take `:80` and `:443` down on every machine already installed until somebody moved or chowned
/// it. The thing that made this urgent is fixed where it belongs instead: a root hive no longer
/// launches anything the file asks for ([`Hive::runners`]). What is left is that whoever can write
/// the file can still re-point a *route* — worth knowing about, not worth going dark over.
#[cfg(unix)]
fn warn_if_config_is_user_writable(path: &Path) {
    use std::os::unix::fs::MetadataExt as _;

    // SAFETY: POSIX `geteuid` takes no arguments, cannot fail, and reads no caller memory.
    #[allow(unsafe_code)]
    let root = unsafe {
        unsafe extern "C" {
            fn geteuid() -> u32;
        }
        geteuid() == 0
    };
    if !root {
        return;
    }
    let Ok(meta) = std::fs::metadata(path) else {
        return;
    };
    // Ownership *or* a group/other write bit: the owner can grant themselves the mode, so
    // `0644 alice` is as writable by alice as `0666` is by everyone.
    let (uid, mode) = (meta.uid(), meta.mode() & 0o7777);
    if uid == 0 && mode & 0o022 == 0 {
        return;
    }
    warn!(
        path = %path.display(), uid, mode = format!("{mode:04o}"),
        "running as root from a config a non-root user can write — they cannot make this daemon \
         launch anything (runners are dropped when root), but they can re-point any route it serves"
    );
}

/// Windows has no root front door, so there is no privilege boundary to warn about.
#[cfg(not(unix))]
fn warn_if_config_is_user_writable(_path: &Path) {}

/// Upper bound on how long shutdown waits for all runners to stop.
const TERM_TIMEOUT: Duration = Duration::from_secs(20);

/// How often the config (and its imports) is re-read to pick up added/removed services.
/// Polling rather than an fs-watch: the files are tiny, a few are involved, and this keeps
/// adi-hive dependency-free.
const RELOAD_INTERVAL: Duration = Duration::from_secs(3);

/// Re-read the config and resolve it to the runners *and* the routing table it now describes, or
/// `None` if it could not be read (a half-written file mid-edit, say) — in which case the caller
/// keeps what it has rather than tearing every service down (or dropping every route) over a
/// transient parse error.
fn reload_config(
    path: &Path,
    ports_manager: &adi_ports_manager::Ports,
    base_dir: &Path,
) -> Option<(Vec<config::RunnerSpec>, Router)> {
    if !path.exists() {
        return None;
    }
    let mut hive = match Hive::load(path) {
        Ok(hive) => hive,
        Err(e) => {
            warn!(error = %e, "could not reload config; keeping the running services and routes");
            return None;
        }
    };
    // A service added since startup has no port yet; leases are idempotent, so re-running this
    // over unchanged services is a no-op that returns their existing ports. Ports must be allocated
    // before resolving, or a fresh service would resolve to no upstream.
    hive.allocate_missing_ports(ports_manager);
    let resolved = hive.resolve();
    Some((
        hive.runners(base_dir),
        Router::new(&resolved.routes, resolved.mesh_gateway),
    ))
}

/// How often the self-watch re-checks the binary on disk.
const WATCH_SELF_PERIOD: Duration = Duration::from_secs(30);

/// With `ADI_WATCH_SELF=1` (set in the launchd plists adi-core generates), resolve the
/// running binary's inode at startup and complete once the file at that path has been
/// *replaced* — the app updater swaps the whole bundle, and this clean exit lets
/// launchd's `KeepAlive` respawn the new build. Root daemons (the :80 front door)
/// can't be kickstarted by the unprivileged updater, so they restart themselves.
/// Without the env var (or when the exe can't be resolved) this never completes.
async fn binary_replaced() {
    let watching = std::env::var_os("ADI_WATCH_SELF").is_some_and(|v| v == "1");
    let exe = std::env::current_exe().ok();
    let (Some(exe), true) = (exe, watching) else {
        std::future::pending::<()>().await;
        return;
    };
    let Some(original) = inode(&exe) else {
        std::future::pending::<()>().await;
        return;
    };

    let mut ticker = tokio::time::interval(WATCH_SELF_PERIOD);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    // Require the same *new* inode on two consecutive ticks so a copy still in flight
    // (a non-atomic install) isn't caught halfway. The updater's rename is atomic, but
    // dev builds writing target/release aren't.
    let mut pending: Option<u64> = None;
    loop {
        ticker.tick().await;
        match inode(&exe) {
            Some(now) if now != original => {
                if pending == Some(now) {
                    return;
                }
                pending = Some(now);
            }
            _ => pending = None,
        }
    }
}

/// A change-token for the file at `path`, if it exists: a value that shifts once the file is
/// replaced. On Unix that's the inode (a rename swaps it); on Windows, which has no stable inode,
/// a `(modified-time, length)` pair stands in — the updater's atomic replace changes both.
fn inode(path: &Path) -> Option<u64> {
    let meta = std::fs::metadata(path).ok()?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt as _;
        Some(meta.ino())
    }
    #[cfg(not(unix))]
    {
        let mtime = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map_or(0, |d| d.as_nanos() as u64);
        Some(mtime ^ meta.len().rotate_left(1))
    }
}

/// On macOS a non-`127.0.0.1` loopback address must be aliased onto `lo0` before it can be bound; elsewhere `127.0.0.0/8` already routes to loopback. Best-effort.
fn ensure_loopback_alias(ip: IpAddr) {
    if ip == IpAddr::V4(Ipv4Addr::LOCALHOST) {
        return;
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
