//! The serving side: accept iroh connections and, per bi-stream, tunnel an allow-listed
//! local port to the peer. Every request is checked against the [`HostConfig`] — an
//! unauthorized peer or a non-allow-listed port is refused with a [status](crate::protocol::Status),
//! never connected. One bi-stream is one TCP tunnel.

use std::net::{Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use iroh::endpoint::{Connection, SendStream};
use iroh::{Endpoint, EndpointId};
use tokio::net::TcpStream;
use tokio::sync::watch;
use tracing::{debug, info, warn};

use crate::config::HostConfig;
use crate::protocol::{self, Status};
use crate::tunnel;

/// After sending a refusal, hold the connection open at most this long so the client can
/// read the status byte before the connection closes (an immediate drop truncates it).
const REFUSAL_LINGER: Duration = Duration::from_secs(3);

/// Accept loop for the host role: spawn a task per inbound connection until shutdown.
pub async fn serve(endpoint: Endpoint, host: Arc<HostConfig>, mut shutdown: watch::Receiver<bool>) {
    if host.allow.is_empty() {
        info!("host: no ports allow-listed — every inbound request will be refused");
    } else {
        info!(allow = ?host.allow, peers = host.authorized_peers.len(), "host: serving allow-listed ports");
    }

    loop {
        tokio::select! {
            _ = shutdown.changed() => {
                info!("host: stopping accept loop");
                return;
            }
            incoming = endpoint.accept() => {
                // `accept()` yields `None` once the endpoint is closed.
                let Some(incoming) = incoming else {
                    debug!("host: endpoint closed; accept loop ending");
                    return;
                };
                let host = Arc::clone(&host);
                tokio::spawn(async move {
                    match incoming.await {
                        Ok(conn) => {
                            if let Err(e) = handle_connection(conn, &host).await {
                                debug!(error = %e, "host: connection ended with error");
                            }
                        }
                        Err(e) => debug!(error = %e, "host: inbound connection failed to establish"),
                    }
                });
            }
        }
    }
}

/// Handle one accepted connection: read the request, authorize it, and either tunnel or refuse.
async fn handle_connection(conn: Connection, host: &HostConfig) -> anyhow::Result<()> {
    let peer = conn.remote_id();
    let (mut send, mut recv) = conn.accept_bi().await?;
    let port = protocol::read_request(&mut recv).await?;

    if !peer_authorized(host, &peer) {
        warn!(%peer, port, "host: refusing — peer not authorized");
        return refuse(&conn, &mut send, Status::PeerNotAuthorized).await;
    }
    if !host.port_allowed(port) {
        warn!(%peer, port, "host: refusing — port not allow-listed");
        return refuse(&conn, &mut send, Status::PortNotAllowed).await;
    }

    let upstream = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
    let tcp = match TcpStream::connect(upstream).await {
        Ok(tcp) => tcp,
        Err(e) => {
            warn!(%peer, port, error = %e, "host: local upstream unavailable");
            return refuse(&conn, &mut send, Status::UpstreamUnavailable).await;
        }
    };

    protocol::write_status(&mut send, Status::Ok).await?;
    info!(%peer, port, "host: tunnel open");
    tunnel::splice(tcp, send, recv).await;
    debug!(%peer, port, "host: tunnel closed");
    Ok(())
}

/// Send a refusal status, finish the reply stream, and wait (bounded) for the client to
/// close — so it reads the reason instead of seeing a truncated "connection lost".
async fn refuse(conn: &Connection, send: &mut SendStream, status: Status) -> anyhow::Result<()> {
    protocol::write_status(send, status).await?;
    let _ = send.finish();
    let _ = tokio::time::timeout(REFUSAL_LINGER, conn.closed()).await;
    Ok(())
}

/// A peer is authorized when the list is empty (open to any peer for allowed ports) or it
/// contains the peer's id. Entries that don't parse as an id are ignored (and warned once
/// per hit) rather than silently authorizing everyone.
fn peer_authorized(host: &HostConfig, peer: &EndpointId) -> bool {
    if host.authorized_peers.is_empty() {
        return true;
    }
    host.authorized_peers.iter().any(|entry| match entry.parse::<EndpointId>() {
        Ok(id) => &id == peer,
        Err(e) => {
            warn!(entry = %entry, error = %e, "host: ignoring unparseable authorized_peers entry");
            false
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn some_id() -> EndpointId {
        iroh::SecretKey::generate().public()
    }

    #[test]
    fn empty_authorized_list_admits_any_peer() {
        let host = HostConfig::default();
        assert!(peer_authorized(&host, &some_id()));
    }

    #[test]
    fn non_empty_list_admits_only_listed_ids() {
        let allowed = some_id();
        let other = some_id();
        let host = HostConfig {
            allow: vec![3000],
            authorized_peers: vec![allowed.to_string()],
        };
        assert!(peer_authorized(&host, &allowed));
        assert!(!peer_authorized(&host, &other));
    }

    #[test]
    fn unparseable_entry_does_not_admit() {
        let host = HostConfig {
            allow: vec![],
            authorized_peers: vec!["not-an-id".to_string()],
        };
        assert!(!peer_authorized(&host, &some_id()));
    }
}
