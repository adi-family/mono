//! The tiny wire protocol spoken inside one iroh bi-stream before the raw bytes flow.
//!
//! A forward is one QUIC bi-stream. The client (the accessing side) opens it and sends a
//! fixed 3-byte [request](write_request) naming the port it wants on the host. The host
//! (the serving side) replies with a 1-byte [status](Status); on [`Status::Ok`] both ends
//! then splice the underlying TCP traffic verbatim. Everything is length-fixed, so a read
//! is a single `read_exact` — no framing ambiguity.

use tokio::io::{AsyncRead, AsyncReadExt as _, AsyncWrite, AsyncWriteExt as _};

/// The ALPN identifying this protocol during the iroh/QUIC handshake. The trailing `/0`
/// is the wire version: bump the ALPN (not just [`VERSION`]) on an incompatible change.
pub const ALPN: &[u8] = b"adi/mesh/forward/0";

/// The request header version. Guards against a peer speaking a future header shape.
const VERSION: u8 = 1;

/// Bytes in the fixed request header: `[version, port_hi, port_lo]`.
const REQUEST_LEN: usize = 3;

/// How the host answered a forward request. The discriminant is the on-wire byte.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Status {
    /// The port is allowed and the local upstream is up; raw bytes follow.
    Ok = 0,
    /// The requested port is not on the host's allow-list.
    PortNotAllowed = 1,
    /// The connecting peer is not on the host's authorized-peers list.
    PeerNotAuthorized = 2,
    /// The port is allowed but nothing is listening on it locally.
    UpstreamUnavailable = 3,
}

impl Status {
    /// The human-readable reason, used in the client's error when a tunnel is refused.
    #[must_use]
    pub fn reason(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::PortNotAllowed => "port not allow-listed by the peer",
            Self::PeerNotAuthorized => "this machine is not an authorized peer",
            Self::UpstreamUnavailable => "the peer's local service is not listening",
        }
    }

    fn from_byte(byte: u8) -> Option<Self> {
        match byte {
            0 => Some(Self::Ok),
            1 => Some(Self::PortNotAllowed),
            2 => Some(Self::PeerNotAuthorized),
            3 => Some(Self::UpstreamUnavailable),
            _ => None,
        }
    }
}

/// Write the fixed request header naming the port to reach on the host.
///
/// # Errors
/// Propagates any write error on the stream.
pub async fn write_request<W: AsyncWrite + Unpin>(w: &mut W, port: u16) -> std::io::Result<()> {
    let [hi, lo] = port.to_be_bytes();
    w.write_all(&[VERSION, hi, lo]).await?;
    // The header is tiny; flush so the host sees it without waiting for later body bytes.
    w.flush().await
}

/// Read and validate the request header, returning the requested port.
///
/// # Errors
/// [`std::io::ErrorKind::InvalidData`] if the version byte is unknown; otherwise any read error.
pub async fn read_request<R: AsyncRead + Unpin>(r: &mut R) -> std::io::Result<u16> {
    let mut buf = [0u8; REQUEST_LEN];
    r.read_exact(&mut buf).await?;
    if buf[0] != VERSION {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("unsupported mesh protocol version {}", buf[0]),
        ));
    }
    Ok(u16::from_be_bytes([buf[1], buf[2]]))
}

/// Write the 1-byte status reply, then flush so the client is unblocked promptly.
///
/// # Errors
/// Propagates any write error on the stream.
pub async fn write_status<W: AsyncWrite + Unpin>(w: &mut W, status: Status) -> std::io::Result<()> {
    w.write_all(&[status as u8]).await?;
    w.flush().await
}

/// Read the 1-byte status reply.
///
/// # Errors
/// [`std::io::ErrorKind::InvalidData`] if the byte is not a known status; otherwise any read error.
pub async fn read_status<R: AsyncRead + Unpin>(r: &mut R) -> std::io::Result<Status> {
    let mut buf = [0u8; 1];
    r.read_exact(&mut buf).await?;
    Status::from_byte(buf[0]).ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("unknown mesh status byte {}", buf[0]),
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn request_header_round_trips() {
        let mut buf = Vec::new();
        write_request(&mut buf, 8080).await.expect("write");
        assert_eq!(buf, vec![VERSION, 0x1f, 0x90]);

        let mut cursor = std::io::Cursor::new(buf);
        assert_eq!(read_request(&mut cursor).await.expect("read"), 8080);
    }

    #[tokio::test]
    async fn request_rejects_a_future_version() {
        let mut cursor = std::io::Cursor::new(vec![VERSION + 1, 0, 80]);
        let err = read_request(&mut cursor).await.expect_err("bad version");
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
    }

    #[tokio::test]
    async fn status_round_trips_and_rejects_unknown() {
        for status in [
            Status::Ok,
            Status::PortNotAllowed,
            Status::PeerNotAuthorized,
            Status::UpstreamUnavailable,
        ] {
            let mut buf = Vec::new();
            write_status(&mut buf, status).await.expect("write");
            let mut cursor = std::io::Cursor::new(buf);
            assert_eq!(read_status(&mut cursor).await.expect("read"), status);
        }

        let mut cursor = std::io::Cursor::new(vec![9u8]);
        let err = read_status(&mut cursor).await.expect_err("unknown status");
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
    }
}
