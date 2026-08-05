//! Splice a local TCP connection to an iroh bi-stream, copying bytes both ways until
//! either side hangs up. This is the payload phase, after the [protocol](crate::protocol)
//! handshake has agreed the tunnel is allowed.

use adi_hive::proxy::forward_closing_response_head;
use iroh::endpoint::{RecvStream, SendStream};
use tokio::io::{AsyncWriteExt as _, copy};
use tokio::net::TcpStream;
use tracing::debug;

/// Pump `tcp` ⇄ (`send`, `recv`) until EOF in each direction, closing the far half so the
/// peer observes the shutdown. Errors are swallowed: a tunnel ending is normal, not fatal.
pub async fn splice(tcp: TcpStream, send: SendStream, recv: RecvStream) {
    pump(tcp, send, recv, false).await;
}

/// [`splice`], plus `Connection: close` written into the first response head.
///
/// **The node side only**, where `tcp` is the local service and so the `tcp → peer` direction is
/// the response. The caller uses this on a *carved* host — one where a second service claims a
/// path prefix, which is every dashboard (`docs/fleet.md` §4).
///
/// The reason is the same one the front door has: the upstream was resolved from *this* request,
/// and what follows is a byte splice, so every later request on the same connection lands on the
/// upstream this one picked. A browser makes that wrong immediately — it fetches the page and then
/// sends the page's `/api` calls down the very same keep-alive connection, where they reach the
/// frontend that served `/` instead of the backend. Telling the client the connection ends here is
/// what makes its next request arrive on a fresh stream, to be routed on its own request line.
pub async fn splice_closing(tcp: TcpStream, send: SendStream, recv: RecvStream) {
    pump(tcp, send, recv, true).await;
}

/// The shared body of both. `close_response` rewrites the head of the `tcp → peer` direction.
async fn pump(tcp: TcpStream, mut send: SendStream, mut recv: RecvStream, close_response: bool) {
    let (mut tcp_read, mut tcp_write) = tcp.into_split();

    // Local -> peer: forward TCP bytes onto the QUIC stream, then FIN it so the host's
    // copy sees EOF and can shut its upstream write side.
    //
    // The rewrite lives *inside* this direction and not before the join. Awaited first it would
    // deadlock any request whose body did not fit in the bytes already read: the rest of that body
    // would sit unread in the other direction, the upstream would wait out its `Content-Length`,
    // and the response head being waited for here could never arrive.
    let to_peer = async {
        if close_response
            && let Err(e) = forward_closing_response_head(&mut tcp_read, &mut send).await
        {
            debug!(error = %e, "could not rewrite the response head; ending the stream");
            let _ = send.finish();
            return;
        }
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
