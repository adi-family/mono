//! The tiny wire protocols spoken inside one iroh bi-stream before the raw bytes flow.
//!
//! Two of them live here, one per ALPN — iroh's router accepts several ALPNs on one endpoint,
//! so a node speaks both at once and the handshake, not a discriminator byte, picks the shape.
//!
//! **[`ALPN`] — `adi/mesh/forward/0`, the raw TCP forward.** A forward is one QUIC bi-stream.
//! The client (the accessing side) opens it and sends a fixed 3-byte [request](write_request)
//! naming the port it wants on the host. The host (the serving side) replies with a 1-byte
//! [status](Status); on [`Status::Ok`] both ends then splice the underlying TCP traffic
//! verbatim. Everything is length-fixed, so a read is a single `read_exact` — no framing
//! ambiguity. This is the path ssh, databases and anything not HTTP keep using.
//!
//! **[`HTTP_ALPN`] — `adi/mesh/http/1`, the fleet HTTP gateway** (`docs/fleet.md` §7). One
//! bi-stream is one HTTP connection. The caller sends a
//! [variable-length header](write_http_request) naming a **service** — one or more DNS labels,
//! see [`is_service_name`] — the node answers with one [`HttpStatus`] byte, and on
//! [`HttpStatus::Ok`] the raw HTTP bytes flow in both directions untouched — the `Host` header
//! included, because the node cannot know what the viewer calls it. A name rather than a port,
//! because the mapping from a service name to a local port belongs to the node's own route
//! table, not to the caller.
//!
//! The two status bytes are deliberately **sibling types, not one shared enum**. The
//! discriminants collide but the meanings do not: byte 1 is "port not allow-listed" for a
//! forward and "no such service" for the gateway. A single enum would carry variants that are
//! unreachable in half of its uses, and would bind two ALPNs that version independently to one
//! vocabulary — bumping `adi/mesh/http/1` would then churn the forward path for no reason.
//! [`Status`] stays the forward protocol's; [`HttpStatus`] is the gateway's.
//!
//! [`parse_fleet_host`] lives here for the same reason: which hostnames name a remote node,
//! and how one splits into `(service, node)`, is part of this wire contract — the `service`
//! it yields is exactly the name that goes into the `adi/mesh/http/1` header — and it shares
//! [`is_service_name`] with the header validator, so the two can never drift apart.

use tokio::io::{AsyncRead, AsyncReadExt as _, AsyncWrite, AsyncWriteExt as _};

/// The ALPN identifying this protocol during the iroh/QUIC handshake. The trailing `/0`
/// is the wire version: bump the ALPN (not just [`VERSION`]) on an incompatible change.
pub const ALPN: &[u8] = b"adi/mesh/forward/0";

/// The request header version. Guards against a peer speaking a future header shape.
const VERSION: u8 = 1;

/// Bytes in the fixed request header: `[version, port_hi, port_lo]`.
const REQUEST_LEN: usize = 3;

/// Define a one-byte wire status: the discriminant *is* the byte on the wire, and each variant
/// carries the sentence the other side shows when that is the answer.
///
/// Both protocols here have one, and a variant is only ever right in three places at once — the
/// byte it is sent as, the parser that reads it back, and its reason. Declaring them together is
/// what stops a variant from reaching the enum and the reason but not `from_byte`, where a peer
/// running this build would answer `InvalidData` to a status this build considers valid.
macro_rules! wire_status {
    (
        $(#[$enum_doc:meta])*
        $name:ident {
            $( $(#[$variant_doc:meta])* $variant:ident = $byte:literal => $reason:literal, )+
        }
    ) => {
        $(#[$enum_doc])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        #[repr(u8)]
        pub enum $name {
            $( $(#[$variant_doc])* $variant = $byte, )+
        }

        impl $name {
            /// The human-readable reason, for the error the other side raises when the answer is
            /// a refusal.
            #[must_use]
            pub fn reason(self) -> &'static str {
                match self {
                    $( Self::$variant => $reason, )+
                }
            }

            fn from_byte(byte: u8) -> Option<Self> {
                match byte {
                    $( $byte => Some(Self::$variant), )+
                    _ => None,
                }
            }
        }
    };
}

wire_status! {
    /// How the host answered a forward request. The discriminant is the on-wire byte.
    Status {
        /// The port is allowed and the local upstream is up; raw bytes follow.
        Ok = 0 => "ok",
        /// The requested port is not on the host's allow-list.
        PortNotAllowed = 1 => "port not allow-listed by the peer",
        /// The connecting peer is not on the host's authorized-peers list.
        PeerNotAuthorized = 2 => "this machine is not an authorized peer",
        /// The port is allowed but nothing is listening on it locally.
        UpstreamUnavailable = 3 => "the peer's local service is not listening",
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
    write_byte(w, status as u8).await
}

/// Read the 1-byte status reply.
///
/// # Errors
/// [`std::io::ErrorKind::InvalidData`] if the byte is not a known status; otherwise any read error.
pub async fn read_status<R: AsyncRead + Unpin>(r: &mut R) -> std::io::Result<Status> {
    let byte = read_byte(r).await?;
    Status::from_byte(byte).ok_or_else(|| invalid_data(format!("unknown mesh status byte {byte}")))
}

// ---------------------------------------------------------------------------------------
// adi/mesh/http/1 — the fleet HTTP gateway (docs/fleet.md §7)
// ---------------------------------------------------------------------------------------

/// The ALPN identifying the HTTP gateway protocol during the iroh/QUIC handshake. The
/// trailing `/1` is the wire version: bump the ALPN (not just [`HTTP_VERSION`]) on an
/// incompatible change, so an old peer fails at the handshake instead of mid-stream.
pub const HTTP_ALPN: &[u8] = b"adi/mesh/http/1";

/// The gateway request header version, mirrored in the header's first byte.
const HTTP_VERSION: u8 = 1;

/// The longest a single DNS label may be — the DNS limit, which is what bounds a petname and
/// each label of a service name.
pub const MAX_LABEL_LEN: usize = 63;

/// The longest a **service name** may be. A service name is one or more labels
/// ([`is_service_name`]), so the ceiling is the DNS *name* limit rather than the label one —
/// still inside the `u8` the header's `service_len` byte carries, which is what keeps the frame
/// shape unchanged now that the field holds more than one label.
pub const MAX_SERVICE_LEN: usize = 253;

/// The reserved suffix that marks a hostname as addressing a *remote* node rather than a
/// local service (`docs/fleet.md` §1). Local services keep `<service>.adi`, so the two
/// namespaces can never collide.
pub const FLEET_SUFFIX: &str = "n.adi";

wire_status! {
    /// How a node answered a gateway request. The discriminant is the on-wire byte.
    ///
    /// Only *transport* outcomes live here. HTTP-level failures (a `401` from the node's auth
    /// gate, a `502` from its front door) are ordinary HTTP responses on an
    /// [`Ok`](HttpStatus::Ok) stream — keeping them apart is what lets the caller render a
    /// precise local error page instead of guessing from a status line it never received.
    HttpStatus {
        /// The service resolved and its local upstream is up; HTTP bytes follow.
        Ok = 0 => "ok",
        /// No service by that label exists on this node.
        ServiceUnknown = 1 => "no such service on that node",
        /// The peer holds no grant for this service (`docs/fleet.md` §5, default-deny).
        NotAuthorized = 2 => "this machine holds no grant for that service",
        /// The service is known but nothing is listening on its local port.
        UpstreamUnavailable = 3 => "the node's local service is not listening",
    }
}

/// Is this one DNS label as `docs/fleet.md` §2 defines it —
/// `^[a-z0-9]([a-z0-9-]{0,61}[a-z0-9])?$`?
///
/// Lowercase only: the wire form is already normalised, so a comparison is a byte compare and
/// two spellings of one name can never both exist in a registry.
#[must_use]
pub fn is_dns_label(label: &str) -> bool {
    let bytes = label.as_bytes();
    if bytes.is_empty() || bytes.len() > MAX_LABEL_LEN {
        return false;
    }
    let alnum = |b: u8| b.is_ascii_lowercase() || b.is_ascii_digit();
    // First and last must be alphanumeric; hyphens are legal only in between.
    alnum(bytes[0]) && alnum(bytes[bytes.len() - 1]) && bytes.iter().all(|&b| alnum(b) || b == b'-')
}

/// Is this a **service name** — one or more [DNS labels](is_dns_label) joined by dots, at most
/// [`MAX_SERVICE_LEN`] bytes overall?
///
/// A service name is a node's own hostname with its local zone taken off, and those are not
/// always one label: a project may sit at `app.nosh.adi` beside the `nosh.adi` it belongs to
/// (`docs/fleet.md` §1). The name that reaches such a service over the mesh is therefore
/// `app.nosh.<node>.n.adi`, and the label the node resolves is the whole `app.nosh` — which is
/// why this, not [`is_dns_label`], is what the wire accepts.
///
/// Every label is still validated individually, so the name stays lowercase, hyphen-safe and
/// free of empty labels; nothing that would be an illegal hostname passes here.
#[must_use]
pub fn is_service_name(name: &str) -> bool {
    name.len() <= MAX_SERVICE_LEN && !name.is_empty() && name.split('.').all(is_dns_label)
}

/// Write the gateway request header — `[version][service_len][service]` — then flush, so the
/// node can start resolving the service while the caller is still reading the HTTP head.
///
/// # Errors
/// [`std::io::ErrorKind::InvalidInput`] if `service` is not a valid service name (a local
/// programming error: emitting the frame anyway would only make the peer reject it);
/// otherwise any write error on the stream.
pub async fn write_http_request<W: AsyncWrite + Unpin>(
    w: &mut W,
    service: &str,
) -> std::io::Result<()> {
    if !is_service_name(service) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("{service:?} is not a valid service name"),
        ));
    }
    // Infallible after the check above: a valid name is 1..=253 bytes.
    let len = u8::try_from(service.len()).unwrap_or(u8::MAX);
    w.write_all(&[HTTP_VERSION, len]).await?;
    w.write_all(service.as_bytes()).await?;
    w.flush().await
}

/// Read and validate the gateway request header, returning the requested service name.
///
/// # Errors
/// [`std::io::ErrorKind::InvalidData`] if the version is unknown, the length is zero or over
/// [`MAX_SERVICE_LEN`], the bytes are not UTF-8, or the name is not a valid service name;
/// otherwise any read error.
pub async fn read_http_request<R: AsyncRead + Unpin>(r: &mut R) -> std::io::Result<String> {
    let mut head = [0u8; 2];
    r.read_exact(&mut head).await?;
    if head[0] != HTTP_VERSION {
        return Err(invalid_data(format!(
            "unsupported mesh http protocol version {}",
            head[0]
        )));
    }
    let len = usize::from(head[1]);
    if len == 0 {
        return Err(invalid_data(
            "mesh http request names an empty service".into(),
        ));
    }
    // Refuse before allocating or reading: an over-long length is a broken peer, and the
    // stream is closed either way, so there is nothing to resynchronise with.
    if len > MAX_SERVICE_LEN {
        return Err(invalid_data(format!(
            "mesh http service name is {len} bytes, over the {MAX_SERVICE_LEN}-byte limit"
        )));
    }
    let mut buf = vec![0u8; len];
    r.read_exact(&mut buf).await?;
    let service = String::from_utf8(buf)
        .map_err(|_| invalid_data("mesh http service name is not valid utf-8".into()))?;
    if !is_service_name(&service) {
        return Err(invalid_data(format!(
            "mesh http service name {service:?} is not a dotted run of dns labels"
        )));
    }
    Ok(service)
}

/// Write the 1-byte gateway status reply, then flush so the caller is unblocked promptly.
///
/// # Errors
/// Propagates any write error on the stream.
pub async fn write_http_status<W: AsyncWrite + Unpin>(
    w: &mut W,
    status: HttpStatus,
) -> std::io::Result<()> {
    write_byte(w, status as u8).await
}

/// Read the 1-byte gateway status reply.
///
/// # Errors
/// [`std::io::ErrorKind::InvalidData`] if the byte is not a known status; otherwise any read
/// error.
pub async fn read_http_status<R: AsyncRead + Unpin>(r: &mut R) -> std::io::Result<HttpStatus> {
    let byte = read_byte(r).await?;
    HttpStatus::from_byte(byte)
        .ok_or_else(|| invalid_data(format!("unknown mesh http status byte {byte}")))
}

/// Split a `Host` header value in the reserved namespace into `(service, node)`:
/// `nosh.laptop-b.n.adi` → `("nosh", "laptop-b")`, `app.nosh.laptop-b.n.adi` →
/// `("app.nosh", "laptop-b")`.
///
/// **The node is always the label immediately before [`FLEET_SUFFIX`]; everything to its left
/// is the service**, however many labels that is. A node's own hostnames are not all one label
/// — `app.nosh.adi` is an ordinary local host beside `nosh.adi` — and the service name here is
/// exactly that host minus its local zone, so a deep name has to survive the split intact
/// rather than be refused as "one label too many".
///
/// Returns `None` for everything that is not at least four labels ending in [`FLEET_SUFFIX`] —
/// `n.adi` and `foo.n.adi` name no service, and a front door must fall through to its ordinary
/// local routing for them rather than guess. An optional `:port` is stripped and the name is
/// lowercased first, since a `Host` header carries whatever case the address bar happened to
/// hold; a single trailing root dot is tolerated, because a fully-qualified `Host` is legal.
#[must_use]
pub fn parse_fleet_host(host: &str) -> Option<(String, String)> {
    let host = host.trim();
    // Strip an optional port. A bracketed IPv6 literal fails the digit check or the label
    // check below, which is the right answer: an address is never a fleet host.
    let name = match host.split_once(':') {
        Some((name, port)) => {
            if port.is_empty() || !port.bytes().all(|b| b.is_ascii_digit()) {
                return None;
            }
            name
        }
        None => host,
    };
    let name = name.strip_suffix('.').unwrap_or(name).to_ascii_lowercase();

    // The pattern needs a node label on top of the suffix's own two, so `n.adi` never matches;
    // `foo.n.adi` — a node with nothing on it — falls out on the emptiness check below.
    let labels: Vec<&str> = name.split('.').collect();
    let [service @ .., node, "n", "adi"] = labels.as_slice() else {
        return None;
    };
    if service.is_empty() || !is_dns_label(node) {
        return None;
    }
    let service = service.join(".");
    is_service_name(&service).then(|| (service, node.to_string()))
}

fn invalid_data(message: String) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::InvalidData, message)
}

/// Write one byte and flush, so the peer is unblocked without waiting for later body bytes.
async fn write_byte<W: AsyncWrite + Unpin>(w: &mut W, byte: u8) -> std::io::Result<()> {
    w.write_all(&[byte]).await?;
    w.flush().await
}

/// Read exactly one byte.
async fn read_byte<R: AsyncRead + Unpin>(r: &mut R) -> std::io::Result<u8> {
    let mut buf = [0u8; 1];
    r.read_exact(&mut buf).await?;
    Ok(buf[0])
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

    // --- adi/mesh/http/1 -----------------------------------------------------------------

    /// The bytes a peer would send for `service`, bypassing [`write_http_request`]'s own
    /// validation so the reader can be tested against frames only a hostile peer would emit.
    fn http_frame(version: u8, len: u8, service: &[u8]) -> std::io::Cursor<Vec<u8>> {
        let mut bytes = vec![version, len];
        bytes.extend_from_slice(service);
        std::io::Cursor::new(bytes)
    }

    #[tokio::test]
    async fn http_request_header_round_trips() {
        let mut buf = Vec::new();
        write_http_request(&mut buf, "nosh").await.expect("write");
        assert_eq!(buf, b"\x01\x04nosh".to_vec());

        let mut cursor = std::io::Cursor::new(buf);
        assert_eq!(read_http_request(&mut cursor).await.expect("read"), "nosh");
    }

    #[tokio::test]
    async fn http_request_round_trips_the_longest_legal_name() {
        // 253 bytes as labels of 63: the length ceiling is the *name* limit, not the label one.
        let service = std::iter::repeat_n("a".repeat(MAX_LABEL_LEN), 4)
            .collect::<Vec<_>>()
            .join(".")[..MAX_SERVICE_LEN]
            .trim_end_matches('.')
            .to_string();
        let mut buf = Vec::new();
        write_http_request(&mut buf, &service).await.expect("write");

        let mut cursor = std::io::Cursor::new(buf);
        assert_eq!(read_http_request(&mut cursor).await.expect("read"), service);
    }

    #[tokio::test]
    async fn http_request_round_trips_a_multi_label_service() {
        // `app.nosh.adi` on the node is `app.nosh` on the wire — a service name, not a label.
        let mut buf = Vec::new();
        write_http_request(&mut buf, "app.nosh")
            .await
            .expect("write");
        assert_eq!(buf, b"\x01\x08app.nosh".to_vec());

        let mut cursor = std::io::Cursor::new(buf);
        assert_eq!(
            read_http_request(&mut cursor).await.expect("read"),
            "app.nosh"
        );
    }

    #[tokio::test]
    async fn http_request_rejects_a_future_version() {
        let mut frame = http_frame(HTTP_VERSION + 1, 4, b"nosh");
        let err = read_http_request(&mut frame)
            .await
            .expect_err("bad version");
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
    }

    #[tokio::test]
    async fn http_request_rejects_a_zero_length_service() {
        let mut frame = http_frame(HTTP_VERSION, 0, b"");
        let err = read_http_request(&mut frame).await.expect_err("empty");
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
    }

    #[tokio::test]
    async fn http_request_rejects_an_over_long_service() {
        let service = vec![b'a'; MAX_SERVICE_LEN + 1];
        let len = u8::try_from(service.len()).expect("fits");
        let mut frame = http_frame(HTTP_VERSION, len, &service);
        let err = read_http_request(&mut frame).await.expect_err("too long");
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);

        // The writer refuses the same name locally rather than emitting a doomed frame.
        let mut buf = Vec::new();
        let err = write_http_request(&mut buf, &"a".repeat(MAX_SERVICE_LEN + 1))
            .await
            .expect_err("too long");
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput);
        assert!(buf.is_empty(), "nothing is written for a rejected name");

        // A single label is still capped at the label limit, inside a legal-length name.
        let mut buf = Vec::new();
        assert!(
            write_http_request(&mut buf, &"a".repeat(MAX_LABEL_LEN + 1))
                .await
                .is_err(),
            "one over-long label is not a service name however short the whole is"
        );
    }

    #[tokio::test]
    async fn http_request_rejects_non_utf8() {
        let mut frame = http_frame(HTTP_VERSION, 4, &[0xff, 0xfe, 0xfd, 0xfc]);
        let err = read_http_request(&mut frame).await.expect_err("not utf-8");
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
    }

    #[tokio::test]
    async fn http_request_rejects_a_non_label_service() {
        // `no.sh` is absent on purpose: a dot now separates labels rather than disqualifying
        // the name. What is still refused is a *label* that breaks the rule, wherever it sits.
        for bad in [
            "-nosh",
            "nosh-",
            "no sh",
            "No.sh",
            "NOSH",
            "no_sh",
            ".nosh",
            "nosh.",
            "app..nosh",
        ] {
            let len = u8::try_from(bad.len()).expect("fits");
            let mut frame = http_frame(HTTP_VERSION, len, bad.as_bytes());
            let err = read_http_request(&mut frame)
                .await
                .expect_err("not a label");
            assert_eq!(err.kind(), std::io::ErrorKind::InvalidData, "{bad:?}");

            let mut buf = Vec::new();
            assert!(write_http_request(&mut buf, bad).await.is_err(), "{bad:?}");
        }
    }

    #[tokio::test]
    async fn http_status_round_trips_and_rejects_unknown() {
        for status in [
            HttpStatus::Ok,
            HttpStatus::ServiceUnknown,
            HttpStatus::NotAuthorized,
            HttpStatus::UpstreamUnavailable,
        ] {
            let mut buf = Vec::new();
            write_http_status(&mut buf, status).await.expect("write");
            assert_eq!(buf, vec![status as u8]);

            let mut cursor = std::io::Cursor::new(buf);
            assert_eq!(read_http_status(&mut cursor).await.expect("read"), status);
            assert!(!status.reason().is_empty());
        }

        let mut cursor = std::io::Cursor::new(vec![4u8]);
        let err = read_http_status(&mut cursor).await.expect_err("unknown");
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
    }

    #[test]
    fn http_status_bytes_match_the_documented_table() {
        assert_eq!(HttpStatus::Ok as u8, 0);
        assert_eq!(HttpStatus::ServiceUnknown as u8, 1);
        assert_eq!(HttpStatus::NotAuthorized as u8, 2);
        assert_eq!(HttpStatus::UpstreamUnavailable as u8, 3);
    }

    #[test]
    fn dns_labels_follow_the_documented_rule() {
        for good in ["a", "0", "nosh", "laptop-b", "a-b-c", &"a".repeat(63)] {
            assert!(is_dns_label(good), "{good:?} is a valid label");
        }
        for bad in [
            "",
            "-a",
            "a-",
            "-",
            "A",
            "Nosh",
            "no sh",
            "no.sh",
            "no_sh",
            "nöesh",
            &"a".repeat(64),
        ] {
            assert!(!is_dns_label(bad), "{bad:?} is not a valid label");
        }
    }

    // --- host parsing --------------------------------------------------------------------

    #[test]
    fn fleet_host_splits_into_service_and_node() {
        let parsed = parse_fleet_host("nosh.laptop-b.n.adi");
        assert_eq!(parsed, Some(("nosh".into(), "laptop-b".into())));
    }

    #[test]
    fn the_node_is_the_last_label_before_the_suffix_however_deep_the_service() {
        // `app.nosh.adi` and `ivr-analytics.nosh.adi` are ordinary hosts on the node; the whole
        // name to the left of the node label is the service the node resolves.
        assert_eq!(
            parse_fleet_host("app.nosh.laptop-b.n.adi"),
            Some(("app.nosh".into(), "laptop-b".into()))
        );
        assert_eq!(
            parse_fleet_host("ivr-analytics.nosh.zomro-de1.n.adi"),
            Some(("ivr-analytics.nosh".into(), "zomro-de1".into()))
        );
        assert_eq!(
            parse_fleet_host("a.b.c.d.laptop-b.n.adi"),
            Some(("a.b.c.d".into(), "laptop-b".into()))
        );
    }

    #[test]
    fn service_names_are_one_or_more_labels() {
        for good in ["a", "nosh", "app.nosh", "ivr-analytics.nosh", "a.b.c.d"] {
            assert!(is_service_name(good), "{good:?} is a valid service name");
        }
        for bad in [
            "",
            ".",
            "app.",
            ".nosh",
            "app..nosh",
            "App.nosh",
            "app.no sh",
        ] {
            assert!(!is_service_name(bad), "{bad:?} is not a valid service name");
        }
        assert!(!is_service_name(&"a".repeat(MAX_SERVICE_LEN + 1)));
    }

    #[test]
    fn fleet_host_strips_the_port_and_normalises_case() {
        for host in [
            "nosh.laptop-b.n.adi:8443",
            "NOSH.Laptop-B.N.ADI",
            "  nosh.laptop-b.n.adi  ",
            "nosh.laptop-b.n.adi.",
            "NOSH.LAPTOP-B.N.ADI:80",
        ] {
            assert_eq!(
                parse_fleet_host(host),
                Some(("nosh".into(), "laptop-b".into())),
                "{host:?}"
            );
        }
    }

    #[test]
    fn non_fleet_hosts_are_none() {
        for host in [
            "",
            "n.adi",             // the suffix alone names no node
            "foo.n.adi",         // a node with no service
            "nosh.laptop-b.adi", // the local namespace, not the fleet one
            "nosh.laptop-b.n.test",
            "nosh.adi",
            "app.adi",
            "example.com",
            "127.0.0.1:8000",
            "[::1]:8000",
            ".laptop-b.n.adi",          // empty service label
            "nosh..n.adi",              // empty node label
            "app..nosh.laptop-b.n.adi", // an empty label inside the service name
            "-nosh.laptop-b.n.adi",
            "nosh.laptop-.n.adi",
            "no_sh.laptop-b.n.adi",
            "nosh.laptop-b.n.adi:http", // a non-numeric port is not a port
            "nosh.laptop-b.n.adi:",
        ] {
            assert_eq!(parse_fleet_host(host), None, "{host:?} is not a fleet host");
        }
    }

    #[test]
    fn a_fleet_host_service_is_wire_legal() {
        // The name the host yields is the one that goes on the wire, so the writer must
        // always accept it — this is the invariant that keeps the two validators in step.
        for host in ["app.laptop-b.n.adi", "app.nosh.laptop-b.n.adi"] {
            let (service, node) = parse_fleet_host(host).expect("fleet host");
            assert!(is_service_name(&service) && is_dns_label(&node), "{host}");
            assert_eq!(format!("{service}.{node}.{FLEET_SUFFIX}"), host);
        }
    }
}
