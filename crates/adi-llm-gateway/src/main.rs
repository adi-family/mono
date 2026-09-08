//! adi-llm-gateway — the endpoint every model client on this machine is pointed at.
//!
//! A thin shell over [`adi_llm_gateway`]: this file owns the socket and the port it is bound to;
//! every forwarding decision is the library's. Foreground process, owned by the hive supervisor,
//! which is what publishes it as `llm.adi` and restarts it when it dies.

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use adi_llm_gateway::config::{self, Settings};
use adi_llm_gateway::journal::Journal;
use adi_llm_gateway::proxy::Gateway;
use tokio::net::TcpListener;
use tracing::{info, warn};

/// Loopback only. The front door is what gives this a name and a route; an endpoint that forwards
/// whatever it is handed, with whatever credential it is handed, is not one to bind on a LAN
/// address by accident.
const BIND_IP: IpAddr = IpAddr::V4(Ipv4Addr::LOCALHOST);

/// How long shutdown waits for the journal to catch up. Long enough for a queue of rows, short
/// enough that a supervisor's `SIGTERM` is never the thing that has to escalate to `SIGKILL`.
const FLUSH_TIMEOUT: Duration = Duration::from_secs(2);

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    init_tracing();

    let settings = Arc::new(Settings::load()?);
    // Before the listener: a gateway that cannot write its journal is just a proxy, and starting
    // one silently is how a day of traffic goes unrecorded.
    let journal = Journal::open()?;
    let gateway = Gateway::new(Arc::clone(&settings), journal.clone())?;

    let addr = SocketAddr::new(BIND_IP, port()?);
    let listener = TcpListener::bind(addr).await?;
    info!(
        %addr,
        providers = ?settings.prefixes(),
        "adi-llm-gateway listening"
    );

    let shutdown = adi_osext::shutdown_signal();
    tokio::pin!(shutdown);
    loop {
        tokio::select! {
            () = &mut shutdown => {
                journal.flush(FLUSH_TIMEOUT);
                info!("shutting down");
                return Ok(());
            }
            accepted = listener.accept() => match accepted {
                Ok((stream, peer)) => {
                    let gateway = gateway.clone();
                    tokio::spawn(async move {
                        if let Err(e) = gateway.handle(stream, peer).await {
                            warn!(%peer, error = %e, "connection failed");
                        }
                    });
                }
                // One failed accept is not a reason to stop serving the ones that follow.
                Err(e) => warn!(error = %e, "accept failed"),
            },
        }
    }
}

/// The port to bind: the argument, else `$PORT` as the supervisor sets it, else the lease this
/// service holds in the ports registry.
///
/// The last case is what makes a hand-started copy land on the same port as the supervised one —
/// `reserve` is idempotent per `(service, key)`, so it returns the existing lease rather than a
/// new port.
fn port() -> anyhow::Result<u16> {
    if let Some(arg) = std::env::args().nth(1) {
        return arg
            .parse()
            .map_err(|_| anyhow::anyhow!("{arg:?} is not a port number"));
    }
    if let Ok(from_env) = std::env::var("PORT")
        && let Ok(port) = from_env.parse()
    {
        return Ok(port);
    }
    Ok(adi_ports_manager::Ports::new().reserve(config::SERVICE, config::PORT_KEY)?)
}

/// Logs to stdout for the supervisor to capture, at `info` unless `RUST_LOG` says otherwise —
/// the same defaults every other supervised adi service uses.
fn init_tracing() {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .init();
}
