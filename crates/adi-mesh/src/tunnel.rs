//! Splice a local TCP connection to an iroh bi-stream, copying bytes both ways until
//! either side hangs up. This is the payload phase, after the [protocol](crate::protocol)
//! handshake has agreed the tunnel is allowed.

use iroh::endpoint::{RecvStream, SendStream};
use tokio::io::{AsyncWriteExt as _, copy};
use tokio::net::TcpStream;

/// Pump `tcp` ⇄ (`send`, `recv`) until EOF in each direction, closing the far half so the
/// peer observes the shutdown. Errors are swallowed: a tunnel ending is normal, not fatal.
pub async fn splice(tcp: TcpStream, mut send: SendStream, mut recv: RecvStream) {
    let (mut tcp_read, mut tcp_write) = tcp.into_split();

    // Local -> peer: forward TCP bytes onto the QUIC stream, then FIN it so the host's
    // copy sees EOF and can shut its upstream write side.
    let to_peer = async {
        let _ = copy(&mut tcp_read, &mut send).await;
        let _ = send.finish();
    };
    // Peer -> local: forward QUIC bytes onto the TCP socket, then shut the TCP write half.
    let to_local = async {
        let _ = copy(&mut recv, &mut tcp_write).await;
        let _ = tcp_write.shutdown().await;
    };

    tokio::join!(to_peer, to_local);
}
