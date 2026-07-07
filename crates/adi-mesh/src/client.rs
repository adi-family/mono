//! The accessing side: bind a local TCP port per [`Forward`] and, for each inbound
//! connection, dial the peer over iroh and tunnel to its remote port. To a local process
//! `127.0.0.1:<listen>` looks like the peer's service.
//!
//! Each local connection opens its own iroh connection + bi-stream. That trades a little
//! setup cost for simplicity and independent failure — a future optimization is to reuse
//! one connection per peer and multiplex streams over it.

use std::net::{Ipv4Addr, SocketAddr};
use std::time::Duration;

use iroh::{Endpoint, EndpointAddr};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::watch;
use tracing::{debug, error, info, warn};

use crate::config::Forward;
use crate::protocol::{self, Status};
use crate::{ticket, tunnel};

/// After an accept error, pause briefly so a persistent failure can't spin the loop hot.
const ACCEPT_ERROR_BACKOFF: Duration = Duration::from_millis(100);

/// Run one forward: bind its local port and tunnel every connection to the peer until
/// shutdown. A bad peer id or an unbindable port logs and returns (the other forwards and
/// the host role keep running).
pub async fn run(endpoint: Endpoint, forward: Forward, mut shutdown: watch::Receiver<bool>) {
    // A target is a ticket (id + addresses) or a bare id (relies on discovery).
    let target = match ticket::parse_target(&forward.peer) {
        Ok(target) => target,
        Err(e) => {
            error!(name = %forward.name, error = %e, "forward: invalid peer target; skipping");
            return;
        }
    };
    let peer_id = target.id;

    let bind = SocketAddr::from((Ipv4Addr::LOCALHOST, forward.listen));
    let listener = match TcpListener::bind(bind).await {
        Ok(listener) => listener,
        Err(e) => {
            error!(name = %forward.name, %bind, error = %e, "forward: cannot bind local port; skipping");
            return;
        }
    };
    info!(name = %forward.name, %bind, peer = %peer_id, remote_port = forward.port, "forward: listening");

    loop {
        tokio::select! {
            _ = shutdown.changed() => {
                info!(name = %forward.name, "forward: stopping");
                return;
            }
            accepted = listener.accept() => match accepted {
                Ok((tcp, client)) => {
                    let endpoint = endpoint.clone();
                    let target = target.clone();
                    let name = forward.name.clone();
                    let remote_port = forward.port;
                    tokio::spawn(async move {
                        if let Err(e) = tunnel_one(endpoint, target, remote_port, tcp).await {
                            warn!(name = %name, %client, error = %e, "forward: tunnel failed");
                        }
                    });
                }
                Err(e) => {
                    warn!(name = %forward.name, error = %e, "forward: accept failed");
                    tokio::time::sleep(ACCEPT_ERROR_BACKOFF).await;
                }
            }
        }
    }
}

/// Dial the peer, request `remote_port`, and — if the host allows it — splice the local
/// connection through.
async fn tunnel_one(
    endpoint: Endpoint,
    target: EndpointAddr,
    remote_port: u16,
    tcp: TcpStream,
) -> anyhow::Result<()> {
    let peer = target.id;
    let conn = endpoint.connect(target, protocol::ALPN).await?;
    let (mut send, mut recv) = conn.open_bi().await?;
    protocol::write_request(&mut send, remote_port).await?;

    match protocol::read_status(&mut recv).await? {
        Status::Ok => {
            debug!(%peer, remote_port, "forward: tunnel open");
            tunnel::splice(tcp, send, recv).await;
            debug!(%peer, remote_port, "forward: tunnel closed");
            Ok(())
        }
        refused => anyhow::bail!(
            "peer refused tunnel to port {remote_port}: {}",
            refused.reason()
        ),
    }
}
