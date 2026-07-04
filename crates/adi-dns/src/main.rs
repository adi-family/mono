//! adi-dns — the adi-family local DNS resolver.
//!
//! A single **foreground** process that:
//!   * answers *split-DNS* override zones locally (e.g. `*.adi` -> 127.0.0.1), and
//!   * forwards every other query to upstream resolvers, transparently.
//!
//! It never daemonizes/forks, so a process supervisor owns its lifecycle:
//! supervisord on Linux/macOS, WinSW/NSSM on Windows. It logs to stdout/stderr
//! (which the supervisor captures) and shuts down cleanly on SIGTERM/SIGINT.
//!
//! Built on hickory-dns (pure Rust): `hickory-server` for the listener and
//! `hickory-resolver` for the upstream forwarder.

mod config;
mod landing;
mod os_routing;
mod status;

use std::fmt;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::time::Duration;

use anyhow::Context as _;
use config::Config;
use hickory_server::net::runtime::{Time, TokioRuntimeProvider};
use hickory_server::proto::op::{Header, HeaderCounts, MessageType, Metadata, OpCode, ResponseCode};
use hickory_server::proto::rr::rdata::{A, AAAA};
use hickory_server::proto::rr::{LowerName, Name, RData, Record, RecordType};
use hickory_server::resolver::config::{NameServerConfig, ResolverConfig};
use hickory_server::resolver::{Resolver, TokioResolver};
use hickory_server::server::{Request, RequestHandler, ResponseHandler, ResponseInfo, Server};
use hickory_server::zone_handler::MessageResponseBuilder;
use tokio::net::{TcpListener, UdpSocket};
use tracing::{error, info, warn};

/// TTL (seconds) applied to locally-synthesized override answers.
const OVERRIDE_TTL: u32 = 60;
/// Idle timeout for a TCP DNS connection.
const TCP_TIMEOUT: Duration = Duration::from_secs(5);
/// Per-connection outgoing response buffer size (bytes); 64 KiB covers a max TCP DNS message.
const RESPONSE_BUFFER: usize = 65_535;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .init();

    // Config path is the first CLI arg; fall back to built-in defaults.
    let config = if let Some(path) = std::env::args().nth(1).map(PathBuf::from) {
        Config::load(&path)?
    } else {
        warn!("no config file given; using built-in defaults");
        Config::default()
    };
    // The DNS side is optional: a landing-only instance (`serve_dns = false`) skips
    // it so a privileged process can own `:80` without fighting the unprivileged
    // resolver for the DNS port. Returns the pieces the shutdown path needs.
    let dns = if config.serve_dns {
        let ports = config.effective_ports();
        info!(
            domain = %config.domain,
            bind_addr = %config.bind_addr,
            ?ports,
            upstreams = ?config.upstreams,
            manage_os_routing = config.manage_os_routing,
            "starting adi-dns"
        );

        // Bind UDP + TCP on the first free candidate port, so a busy port on any
        // given machine never blocks startup.
        let (udp, tcp, bound) = bind_ports(config.bind_addr, &ports)
            .await
            .context("binding resolver listener")?;
        info!(%bound, "listening");

        let handler = AdiHandler::new(&config)?;
        let mut server = Server::new(handler);
        server.register_socket(udp);
        server.register_listener(tcp, TCP_TIMEOUT, RESPONSE_BUFFER);

        // Self-register the OS route for `.domain` at the port we actually bound.
        let routing_installed = install_os_routing(&config, bound);

        // Publish a status file so the controlling GUI can read the live state
        // (running, bound port, route installed). Best-effort — never blocks serving.
        let status_path = status::resolve_path(config.status_file.as_deref());
        let status = status::Status::new(&config.domain, bound, routing_installed);
        match status::write(&status_path, &status) {
            Ok(()) => info!(path = %status_path.display(), "wrote status file"),
            Err(e) => warn!(error = %e, path = %status_path.display(), "could not write status file"),
        }

        Some((server, routing_installed, status_path))
    } else {
        info!(domain = %config.domain, "serve_dns is off; running landing-only");
        None
    };

    // Optionally serve the built-in HTTP "not found" page for `.domain`. Runs on
    // its own task; a bind failure (needs root for :80 / a loopback alias) is
    // logged there and never stops the process.
    let landing_task = if config.landing.enabled {
        let bind = config.landing.bind;
        ensure_landing_address(bind.ip());
        let domain = config.domain.clone();
        Some(tokio::spawn(async move {
            if let Err(e) = landing::serve(bind, domain).await {
                warn!(error = %e, %bind, "landing HTTP server did not start");
            }
        }))
    } else {
        None
    };

    if dns.is_none() && landing_task.is_none() {
        anyhow::bail!("nothing to do: serve_dns is false and landing is disabled");
    }

    info!("adi-dns ready");

    // Run until the DNS server exits (if it's running) or a stop signal arrives.
    match dns {
        Some((mut server, routing_installed, status_path)) => {
            tokio::select! {
                res = server.block_until_done() => res.context("DNS server terminated with error")?,
                () = shutdown_signal() => info!("shutdown signal received; stopping"),
            }
            stop_landing(landing_task);
            status::remove(&status_path);
            if routing_installed {
                match os_routing::uninstall(&config.domain) {
                    Ok(()) => info!(domain = %config.domain, "removed OS route"),
                    Err(e) => warn!(error = %e, "failed to remove OS route; remove it manually"),
                }
            }
            let _ = server.shutdown_gracefully().await;
        }
        None => {
            shutdown_signal().await;
            info!("shutdown signal received; stopping");
            stop_landing(landing_task);
        }
    }
    Ok(())
}

/// Abort the landing server task, if one is running.
fn stop_landing(task: Option<tokio::task::JoinHandle<()>>) {
    if let Some(task) = task {
        task.abort();
    }
}

/// Bind UDP + TCP on the first candidate port that is free, tried in order.
/// Returns both sockets and the actual bound address.
async fn bind_ports(
    addr: IpAddr,
    ports: &[u16],
) -> anyhow::Result<(UdpSocket, TcpListener, SocketAddr)> {
    let mut attempts = Vec::new();
    for &port in ports {
        let sa = SocketAddr::new(addr, port);
        match UdpSocket::bind(sa).await {
            Ok(udp) => match TcpListener::bind(sa).await {
                Ok(tcp) => return Ok((udp, tcp, sa)),
                // UDP bound but TCP didn't: drop udp (end of scope) and try next.
                Err(e) => attempts.push(format!("{sa} tcp: {e}")),
            },
            Err(e) => attempts.push(format!("{sa} udp: {e}")),
        }
    }
    anyhow::bail!("no candidate port could be bound ({})", attempts.join("; "))
}

/// Install the OS route when requested, returning whether it was installed. Never
/// fatal: a failure (usually missing admin rights) degrades to a warning plus the
/// manual command, and the resolver keeps serving.
fn install_os_routing(config: &Config, bound: SocketAddr) -> bool {
    if !config.manage_os_routing {
        info!("manage_os_routing is off; not modifying OS DNS configuration");
        return false;
    }
    match os_routing::install(&config.domain, bound) {
        Ok(()) => {
            info!(domain = %config.domain, %bound, "installed OS route for .{}", config.domain);
            true
        }
        Err(e) => {
            warn!(error = %e, "could not auto-install OS route (need admin/root?); resolver still serving");
            warn!(
                "route .{} manually: {}",
                config.domain,
                os_routing::describe_manual(&config.domain, bound)
            );
            false
        }
    }
}

/// Make the landing server's bind address usable. On macOS a non-`127.0.0.1`
/// loopback address must be aliased onto `lo0` before it can be bound; elsewhere
/// the whole `127.0.0.0/8` block already routes to loopback. Best-effort (needs
/// root): a failure degrades to a warning, and the bind below simply fails if the
/// address really isn't available.
fn ensure_landing_address(ip: IpAddr) {
    if ip == IpAddr::V4(Ipv4Addr::LOCALHOST) {
        return; // always present
    }
    #[cfg(target_os = "macos")]
    {
        match std::process::Command::new("ifconfig")
            .args(["lo0", "alias", &ip.to_string(), "up"])
            .status()
        {
            Ok(s) if s.success() => info!(%ip, "aliased loopback address for landing server"),
            Ok(s) => warn!(%ip, code = ?s.code(), "ifconfig lo0 alias failed (need root?)"),
            Err(e) => warn!(%ip, error = %e, "could not run ifconfig to alias loopback"),
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = ip; // 127.0.0.0/8 is already loopback on Linux/Windows
    }
}

/// Resolve `SIGTERM`/`SIGINT` (Unix) or Ctrl-C (Windows) into a single future.
async fn shutdown_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};
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

/// The request handler: split-DNS overrides first, then upstream forwarding.
struct AdiHandler {
    /// Override zones, each an FQDN suffix mapped to the address it resolves to.
    overrides: Vec<(LowerName, IpAddr)>,
    /// Forwarder to the configured upstream resolvers.
    resolver: TokioResolver,
}

impl fmt::Debug for AdiHandler {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AdiHandler")
            .field("overrides", &self.overrides)
            .finish_non_exhaustive()
    }
}

impl AdiHandler {
    fn new(config: &Config) -> anyhow::Result<Self> {
        // Falls back to `domain -> 127.0.0.1` when no explicit overrides are set.
        let zones = config.overrides_or_default();
        let mut overrides = Vec::with_capacity(zones.len());
        for zone in &zones {
            let name = Name::from_utf8(&zone.suffix)
                .with_context(|| format!("invalid override suffix {:?}", zone.suffix))?;
            overrides.push((LowerName::from(&name), zone.address));
        }
        // Sort most-specific first so a longer suffix (e.g. `v6.adi`) wins over a
        // shorter one (`adi`) when both match — DNS "longest match" semantics.
        overrides.sort_by_key(|(name, _)| std::cmp::Reverse(name.num_labels()));

        // Build the upstream forwarder. Each upstream is queried over UDP+TCP;
        // the configured port overrides the default 53.
        let name_servers = config
            .upstreams
            .iter()
            .map(|addr| {
                let mut ns = NameServerConfig::udp_and_tcp(addr.ip());
                for conn in &mut ns.connections {
                    conn.port = addr.port();
                }
                ns
            })
            .collect::<Vec<_>>();
        let resolver_config = ResolverConfig::from_parts(None, Vec::new(), name_servers);
        let resolver = Resolver::builder_with_config(resolver_config, TokioRuntimeProvider::default())
            .build()
            .context("building upstream resolver")?;

        Ok(Self { overrides, resolver })
    }

    /// Return the override address if `qname` falls within a configured zone.
    fn match_override(&self, qname: &LowerName) -> Option<IpAddr> {
        self.overrides
            .iter()
            .find(|(suffix, _)| suffix.zone_of(qname))
            .map(|&(_, ip)| ip)
    }

    /// Synthesize the answer records for an override hit. Empty when the query
    /// type doesn't match the override's address family (a NOERROR/no-data case).
    fn override_records(name: &Name, rtype: RecordType, ip: IpAddr) -> Vec<Record> {
        let rdata = match (ip, rtype) {
            (IpAddr::V4(v4), RecordType::A) => Some(RData::A(A(v4))),
            (IpAddr::V6(v6), RecordType::AAAA) => Some(RData::AAAA(AAAA(v6))),
            _ => None,
        };
        rdata
            .map(|rd| vec![Record::from_rdata(name.clone(), OVERRIDE_TTL, rd)])
            .unwrap_or_default()
    }

    /// Core request logic. Returns the `ResponseInfo` for whatever response we sent.
    async fn respond<R: ResponseHandler>(
        &self,
        request: &Request,
        response_handle: &mut R,
    ) -> anyhow::Result<ResponseInfo> {
        let info = request
            .request_info()
            .map_err(|e| anyhow::anyhow!("malformed request: {e}"))?;
        let query = info.query;
        let rtype = query.query_type();
        let name = query.original().name().clone();

        let mut metadata = Metadata::response_from_request(info.metadata);
        metadata.recursion_available = true;

        // 1) Split-DNS override.
        if let Some(ip) = self.match_override(query.name()) {
            info!(%name, %rtype, %ip, "override");
            metadata.authoritative = true;
            let records = Self::override_records(&name, rtype, ip);
            return self.send(request, response_handle, metadata, &records).await;
        }

        // 2) Forward to upstream.
        match self.resolver.lookup(name.clone(), rtype).await {
            Ok(lookup) => {
                let records = lookup.answers().to_vec();
                self.send(request, response_handle, metadata, &records).await
            }
            Err(e) if e.is_nx_domain() => {
                self.send_code(request, response_handle, metadata, ResponseCode::NXDomain)
                    .await
            }
            Err(e) if e.is_no_records_found() => {
                self.send_code(request, response_handle, metadata, ResponseCode::NoError)
                    .await
            }
            Err(e) => {
                warn!(%name, %rtype, error = %e, "upstream lookup failed");
                self.send_code(request, response_handle, metadata, ResponseCode::ServFail)
                    .await
            }
        }
    }

    /// Send a successful (possibly empty) answer set.
    async fn send<R: ResponseHandler>(
        &self,
        request: &Request,
        response_handle: &mut R,
        metadata: Metadata,
        records: &[Record],
    ) -> anyhow::Result<ResponseInfo> {
        let builder = MessageResponseBuilder::from_message_request(request);
        let message = builder.build(
            metadata,
            records.iter(),
            std::iter::empty(),
            std::iter::empty(),
            std::iter::empty(),
        );
        response_handle
            .send_response(message)
            .await
            .context("sending response")
    }

    /// Send a response carrying only a status code (NXDOMAIN, SERVFAIL, empty NOERROR).
    async fn send_code<R: ResponseHandler>(
        &self,
        request: &Request,
        response_handle: &mut R,
        mut metadata: Metadata,
        code: ResponseCode,
    ) -> anyhow::Result<ResponseInfo> {
        metadata.response_code = code;
        self.send(request, response_handle, metadata, &[]).await
    }
}

#[async_trait::async_trait]
impl RequestHandler for AdiHandler {
    async fn handle_request<R: ResponseHandler, T: Time>(
        &self,
        request: &Request,
        mut response_handle: R,
    ) -> ResponseInfo {
        match self.respond(request, &mut response_handle).await {
            Ok(info) => info,
            Err(err) => {
                error!(%err, "failed to handle request");
                serve_failed(request)
            }
        }
    }
}

/// Last-resort SERVFAIL response info, used when even sending a response failed.
fn serve_failed(request: &Request) -> ResponseInfo {
    let id = request.request_info().map_or(0, |i| i.metadata.id);
    let mut metadata = Metadata::new(id, MessageType::Response, OpCode::Query);
    metadata.response_code = ResponseCode::ServFail;
    ResponseInfo::from(Header {
        metadata,
        counts: HeaderCounts::default(),
    })
}
