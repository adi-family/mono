//! HTTP/1.1 as a client speaks it, over a mesh [`Stream`](crate::mesh::Stream).
//!
//! A browser normally never needs this: `fetch()` is the HTTP client. Here the bytes cross a QUIC
//! stream the tab opened itself, so nothing in the platform is on the path and the framing is
//! ours to get right — the request head, the response head, and the three ways HTTP/1.1 says how
//! long a body is.
//!
//! The body is delivered as **chunks as they arrive**, never as a `Vec` read to end. That is the
//! whole point of the module: a control panel's `text/event-stream` and a `101` upgrade are
//! ordinary responses whose body does not end, and a client that returned `Vec<u8>` could not
//! carry either one.

use std::fmt::Write as _;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as B64;

use crate::mesh::{Reader, Result};

/// A request to send over an open mesh stream.
#[derive(Debug, Clone)]
pub struct Request {
    /// `GET`, `POST`, …
    pub method: String,
    /// The request target — a path with its query, never an absolute URL.
    pub target: String,
    /// Header name/value pairs, in the order they go on the wire.
    pub headers: Vec<(String, String)>,
    /// The body, already encoded. Sent with a `Content-Length` when non-empty.
    pub body: Vec<u8>,
}

impl Request {
    /// A `GET` with no headers of its own.
    #[must_use]
    pub fn get(target: impl Into<String>) -> Self {
        Self {
            method: "GET".into(),
            target: target.into(),
            headers: Vec::new(),
            body: Vec::new(),
        }
    }

    /// Add a header, replacing any existing one of the same name.
    #[must_use]
    pub fn with(mut self, name: &str, value: &str) -> Self {
        self.headers.retain(|(n, _)| !n.eq_ignore_ascii_case(name));
        self.headers.push((name.to_string(), value.to_string()));
        self
    }

    /// Attach the node's Basic credential (`docs/fleet.md` §5, the HTTP layer).
    #[must_use]
    pub fn with_basic_auth(self, username: &str, password: &str) -> Self {
        let credential = B64.encode(format!("{username}:{password}"));
        self.with("Authorization", &format!("Basic {credential}"))
    }

    /// The request as it goes on the wire.
    ///
    /// `Host: 127.0.0.1` unless the caller set one, for the reason the iOS viewer uses it
    /// (`adi-mesh-ffi/src/viewer/catalog.rs`): the node routes on the service name in the gateway
    /// frame and never on `Host`, and `docs/fleet.md` §3 forbids rewriting it — so what goes here
    /// has to be something that cannot accidentally mean anything to the far side.
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let mut head = format!("{} {} HTTP/1.1\r\n", self.method, self.target);
        if !self.has("host") {
            head.push_str("Host: 127.0.0.1\r\n");
        }
        for (name, value) in &self.headers {
            // `write!` into a `String` is infallible; the `Result` is the trait's, not this call's.
            let _ = write!(head, "{name}: {value}\r\n");
        }
        // Only when there is one: a `Content-Length: 0` on a GET is legal but makes some upstreams
        // wait for a body, and this head is spliced to an arbitrary local service.
        if !self.body.is_empty() && !self.has("content-length") {
            let _ = write!(head, "Content-Length: {}\r\n", self.body.len());
        }
        head.push_str("\r\n");
        let mut out = head.into_bytes();
        out.extend_from_slice(&self.body);
        out
    }

    fn has(&self, name: &str) -> bool {
        self.headers
            .iter()
            .any(|(n, _)| n.eq_ignore_ascii_case(name))
    }
}

/// A response head, parsed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Head {
    /// The status code.
    pub status: u16,
    /// The reason phrase, as the node's service wrote it.
    pub reason: String,
    /// Header name/value pairs. Names are lowercased; values keep their spelling.
    pub headers: Vec<(String, String)>,
}

impl Head {
    /// Parse a head — everything up to and including the blank line.
    ///
    /// # Errors
    /// If there is no status line, or its code is not a number.
    pub fn parse(head: &[u8]) -> Result<Self> {
        // Lossy rather than strict: a header value is bytes, and one service emitting a Latin-1
        // filename must not cost the reader the whole response.
        let text = String::from_utf8_lossy(head);
        let mut lines = text.split("\r\n");
        let status_line = lines.next().unwrap_or_default();
        let mut parts = status_line.splitn(3, ' ');
        let _version = parts.next();
        let status: u16 = parts
            .next()
            .and_then(|code| code.parse().ok())
            .ok_or_else(|| format!("the response has no status code: {status_line:?}"))?;
        let reason = parts.next().unwrap_or_default().to_string();

        let headers = lines
            .take_while(|line| !line.is_empty())
            .filter_map(|line| line.split_once(':'))
            .map(|(name, value)| (name.trim().to_ascii_lowercase(), value.trim().to_string()))
            .collect();
        Ok(Self {
            status,
            reason,
            headers,
        })
    }

    /// The first value of `name`, which is already lowercase here.
    #[must_use]
    pub fn get(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.as_str())
    }

    /// How this response says its body ends.
    #[must_use]
    pub fn framing(&self) -> Framing {
        // Order is RFC 9112 §6.3: an upgrade first — past a `101` the bytes are no longer HTTP at
        // all — then chunked, which wins over any `Content-Length` that came with it.
        if self.status == 101 {
            return Framing::Upgraded;
        }
        if self
            .get("transfer-encoding")
            .is_some_and(|te| te.to_ascii_lowercase().contains("chunked"))
        {
            return Framing::Chunked;
        }
        match self
            .get("content-length")
            .and_then(|v| v.trim().parse().ok())
        {
            Some(len) => Framing::Length(len),
            // No length and not chunked means "until the connection closes" — which is also what a
            // `text/event-stream` looks like, and why this arm must never be read to end eagerly.
            None => Framing::UntilClose,
        }
    }
}

/// How many bytes of body follow, and how the reader will know.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Framing {
    /// Exactly this many bytes.
    Length(usize),
    /// `Transfer-Encoding: chunked`.
    Chunked,
    /// Until end of stream.
    UntilClose,
    /// A `101`: what follows is another protocol's bytes, not a body.
    Upgraded,
}

/// A response body, delivered a chunk at a time.
///
/// Holds the stream it is reading, so dropping it is what closes the connection — which is how a
/// reader that navigates away stops an SSE feed the node would otherwise keep writing to.
#[derive(Debug)]
pub struct Body {
    framing: Framing,
    remaining: usize,
    done: bool,
}

impl Body {
    /// A reader for a body framed as `head` says.
    #[must_use]
    pub fn new(head: &Head) -> Self {
        let framing = head.framing();
        Self {
            framing,
            remaining: match framing {
                Framing::Length(len) => len,
                _ => 0,
            },
            // A `101` has no body, and a `204`/`304` must not be read for one whatever it claims.
            done: matches!(framing, Framing::Upgraded)
                || matches!(head.status, 204 | 304)
                || framing == Framing::Length(0),
        }
    }

    /// Whether the body is complete. A `false` here does **not** promise more bytes soon: an
    /// event stream is unfinished for as long as the reader is watching it.
    #[must_use]
    pub fn is_done(&self) -> bool {
        self.done
    }

    /// The next piece of body, or an empty vector once it is complete.
    ///
    /// # Errors
    /// A malformed chunk header, or any read error.
    pub async fn next(&mut self, stream: &mut Reader) -> Result<Vec<u8>> {
        if self.done {
            return Ok(Vec::new());
        }
        match self.framing {
            Framing::Length(_) => {
                let chunk = stream.read().await?;
                if chunk.is_empty() {
                    self.done = true;
                    return Ok(Vec::new());
                }
                let take = chunk.len().min(self.remaining);
                if take < chunk.len() {
                    // The far side pipelined past the end of this body. Nothing on this client
                    // sends a second request down a stream it is already reading, so this is a
                    // broken upstream — keep the body honest and drop the surplus.
                    stream.unread(&chunk[take..]);
                }
                self.remaining -= take;
                self.done = self.remaining == 0;
                Ok(chunk[..take].to_vec())
            }
            Framing::UntilClose => {
                let chunk = stream.read().await?;
                self.done = chunk.is_empty();
                Ok(chunk)
            }
            Framing::Chunked => self.next_chunked(stream).await,
            Framing::Upgraded => {
                self.done = true;
                Ok(Vec::new())
            }
        }
    }

    /// One `chunked` chunk: its size line, its bytes, and the CRLF after them.
    async fn next_chunked(&mut self, stream: &mut Reader) -> Result<Vec<u8>> {
        if self.remaining == 0 {
            let size = read_line(stream).await?;
            // A chunk extension (`1a;name=value`) is legal and carries nothing we act on.
            let size = size.split(';').next().unwrap_or("").trim();
            let size = usize::from_str_radix(size, 16)
                .map_err(|_| format!("a chunk size that is not hex: {size:?}"))?;
            if size == 0 {
                // The trailer section, then the final blank line. Read until one is empty rather
                // than assuming there is none: a trailer is rare but legal, and leaving it on the
                // stream would corrupt nothing here (we close) yet would read as a bug later.
                while !read_line(stream).await?.is_empty() {}
                self.done = true;
                return Ok(Vec::new());
            }
            self.remaining = size;
        }
        let chunk = stream.read().await?;
        if chunk.is_empty() {
            return Err("the stream ended inside a chunk".into());
        }
        let take = chunk.len().min(self.remaining);
        if take < chunk.len() {
            stream.unread(&chunk[take..]);
        }
        self.remaining -= take;
        if self.remaining == 0 {
            // The CRLF that closes the chunk.
            let _ = read_line(stream).await?;
        }
        Ok(chunk[..take].to_vec())
    }
}

/// Read one CRLF-terminated line, leaving everything after it buffered.
async fn read_line(stream: &mut Reader) -> Result<String> {
    let mut line = Vec::new();
    loop {
        if let Some(at) = line.windows(2).position(|w| w == b"\r\n") {
            let rest = line.split_off(at + 2);
            stream.unread(&rest);
            line.truncate(at);
            return Ok(String::from_utf8_lossy(&line).to_string());
        }
        let chunk = stream.read().await?;
        if chunk.is_empty() {
            return Err("the stream ended mid-line".into());
        }
        line.extend_from_slice(&chunk);
        if line.len() > MAX_LINE {
            return Err("a chunk header ran past its limit".into());
        }
    }
}

/// The most a chunk-size line or a trailer may be. Real ones are under 20 bytes.
const MAX_LINE: usize = 8 * 1024;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_request_carries_a_host_and_a_length_only_when_it_needs_them() {
        let bare = Request::get("/api/health").encode();
        let bare = String::from_utf8(bare).expect("utf-8");
        assert!(bare.starts_with("GET /api/health HTTP/1.1\r\n"));
        assert!(bare.contains("Host: 127.0.0.1\r\n"));
        assert!(
            !bare.contains("Content-Length"),
            "a bodyless request must not claim a length: the head is spliced to a real service"
        );

        let posted = Request {
            method: "POST".into(),
            target: "/api/x".into(),
            headers: vec![("Content-Type".into(), "application/json".into())],
            body: b"{}".to_vec(),
        }
        .encode();
        let posted = String::from_utf8(posted).expect("utf-8");
        assert!(posted.contains("Content-Length: 2\r\n"));
        assert!(posted.ends_with("\r\n\r\n{}"));
    }

    #[test]
    fn a_header_set_twice_is_replaced_not_repeated() {
        let request = Request::get("/")
            .with("Authorization", "Basic one")
            .with_basic_auth("adi", "two");
        assert_eq!(
            request
                .headers
                .iter()
                .filter(|(n, _)| n == "Authorization")
                .count(),
            1
        );
    }

    #[test]
    fn a_head_parses_into_status_and_lowercased_headers() {
        let head = Head::parse(
            b"HTTP/1.1 401 Unauthorized\r\n\
              WWW-Authenticate: Basic realm=\"laptop-b\"\r\n\
              Content-Length: 0\r\n\r\n",
        )
        .expect("head");
        assert_eq!(head.status, 401);
        assert_eq!(head.reason, "Unauthorized");
        assert_eq!(
            head.get("www-authenticate"),
            Some("Basic realm=\"laptop-b\"")
        );
        assert_eq!(head.framing(), Framing::Length(0));
    }

    #[test]
    fn framing_follows_the_rfcs_order() {
        let upgraded = Head::parse(b"HTTP/1.1 101 Switching Protocols\r\n\r\n").expect("head");
        assert_eq!(upgraded.framing(), Framing::Upgraded);

        // Chunked wins over a Content-Length that came with it.
        let both = Head::parse(
            b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nContent-Length: 9\r\n\r\n",
        )
        .expect("head");
        assert_eq!(both.framing(), Framing::Chunked);

        // An event stream names no length and is not chunked — the case a client that read to
        // end would hang on forever.
        let sse = Head::parse(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\n\r\n")
            .expect("head");
        assert_eq!(sse.framing(), Framing::UntilClose);
    }

    #[test]
    fn a_bodyless_status_is_done_before_it_is_read() {
        for head in [
            &b"HTTP/1.1 204 No Content\r\n\r\n"[..],
            &b"HTTP/1.1 304 Not Modified\r\nContent-Length: 17\r\n\r\n"[..],
            &b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n"[..],
            &b"HTTP/1.1 101 Switching Protocols\r\n\r\n"[..],
        ] {
            let head = Head::parse(head).expect("head");
            assert!(Body::new(&head).is_done(), "{head:?}");
        }
    }
}
