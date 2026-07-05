//! adi-dns — the adi-family local DNS resolver: answers split-DNS override zones
//! locally and forwards everything else upstream. Foreground process; a supervisor
//! owns its lifecycle. Built on hickory-dns.

mod config;
mod os_routing;
mod status;

use std::fmt;
use std::net::{IpAddr, SocketAddr};
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

const OVERRIDE_TTL: u32 = 60;
const TCP_TIMEOUT: Duration = Duration::from_secs(5);
/// 64 KiB — a max-size TCP DNS message.
const RESPONSE_BUFFER: usize = 65_535;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .init();

    let config = if let Some(path) = std::env::args().nth(1).map(PathBuf::from) {
        Config::load(&path)?
    } else {
        warn!("no config file given; using built-in defaults");
        Config::default()
    };
    let ports = config.effective_ports();
    info!(
        domain = %config.domain,
        bind_addr = %config.bind_addr,
        ?ports,
        upstreams = ?config.upstreams,
        manage_os_routing = config.manage_os_routing,
        "starting adi-dns"
    );

    let (udp, tcp, bound) = bind_ports(config.bind_addr, &ports)
        .await
        .context("binding resolver listener")?;
    info!(%bound, "listening");

    let handler = AdiHandler::new(&config)?;
    let mut server = Server::new(handler);
    server.register_socket(udp);
    server.register_listener(tcp, TCP_TIMEOUT, RESPONSE_BUFFER);

    let routing_installed = install_os_routing(&config, bound);

    let status_path = status::resolve_path(config.status_file.as_deref());
    let status = status::Status::new(&config.domain, bound, routing_installed);
    match status::write(&status_path, &status) {
        Ok(()) => info!(path = %status_path.display(), "wrote status file"),
        Err(e) => warn!(error = %e, path = %status_path.display(), "could not write status file"),
    }

    info!("adi-dns ready");

    tokio::select! {
        res = server.block_until_done() => res.context("DNS server terminated with error")?,
        () = shutdown_signal() => info!("shutdown signal received; stopping"),
    }

    status::remove(&status_path);
    if routing_installed {
        match os_routing::uninstall(&config.domain) {
            Ok(()) => info!(domain = %config.domain, "removed OS route"),
            Err(e) => warn!(error = %e, "failed to remove OS route; remove it manually"),
        }
    }
    let _ = server.shutdown_gracefully().await;
    Ok(())
}

/// Bind UDP + TCP on the first free candidate port, tried in order.
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
                // UDP bound but TCP didn't: drop udp and try the next port.
                Err(e) => attempts.push(format!("{sa} tcp: {e}")),
            },
            Err(e) => attempts.push(format!("{sa} udp: {e}")),
        }
    }
    anyhow::bail!("no candidate port could be bound ({})", attempts.join("; "))
}

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

/// Split-DNS overrides first, then upstream forwarding.
struct AdiHandler {
    overrides: Vec<(LowerName, IpAddr)>,
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
        let zones = config.overrides_or_default();
        let mut overrides = Vec::with_capacity(zones.len());
        for zone in &zones {
            let name = Name::from_utf8(&zone.suffix)
                .with_context(|| format!("invalid override suffix {:?}", zone.suffix))?;
            overrides.push((LowerName::from(&name), zone.address));
        }
        // Longest suffix first, so `v6.adi` wins over `adi` (DNS longest-match).
        overrides.sort_by_key(|(name, _)| std::cmp::Reverse(name.num_labels()));

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

    fn match_override(&self, qname: &LowerName) -> Option<IpAddr> {
        self.overrides
            .iter()
            .find(|(suffix, _)| suffix.zone_of(qname))
            .map(|&(_, ip)| ip)
    }

    /// Empty vec when the query type doesn't match the address family (NODATA).
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

        if let Some(ip) = self.match_override(query.name()) {
            info!(%name, %rtype, %ip, "override");
            metadata.authoritative = true;
            let records = Self::override_records(&name, rtype, ip);
            return self.send(request, response_handle, metadata, &records).await;
        }

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

fn serve_failed(request: &Request) -> ResponseInfo {
    let id = request.request_info().map_or(0, |i| i.metadata.id);
    let mut metadata = Metadata::new(id, MessageType::Response, OpCode::Query);
    metadata.response_code = ResponseCode::ServFail;
    ResponseInfo::from(Header {
        metadata,
        counts: HeaderCounts::default(),
    })
}
