//! The forwarding core: one client connection in, one provider exchange out, one journal row.
//!
//! # Streaming is the requirement, not a feature
//!
//! A model answer arrives as server-sent events over minutes, and a proxy that waits for the end
//! before answering turns a live transcript into a long silence. So the response head is written
//! and flushed as soon as the provider sends it, and every chunk is flushed on arrival — the tee
//! into the journal happens on the way past, never in front.
//!
//! # One exchange per connection
//!
//! Each response carries `Connection: close`, as `adi-app` and the front door's own spliced
//! responses do. It costs a TCP handshake to loopback and removes the whole class of bug where a
//! keep-alive socket carries a second request whose framing the first one's headers decided.

use std::fmt::Write as _;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::io::AsyncWriteExt as _;
use tokio::net::TcpStream;
use tracing::{info, warn};

use crate::config::Settings;
use crate::http::{self, Reader};
use crate::journal::{self, Entry, Journal};

/// How long to wait for the provider's TCP + TLS handshake.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);

/// How long to wait for the *next* bytes of an answer, not for the whole of it. A long generation
/// keeps arriving; a provider that has stopped talking mid-stream stalls here and nowhere else.
const READ_TIMEOUT: Duration = Duration::from_secs(300);

/// Headers that describe one hop and must not be replayed at the next (RFC 9110 §7.6.1), plus the
/// two the upstream leg owns: `host` (the URL names the host now) and `content-length` (the client
/// builds it from the body it is given).
const HOP_BY_HOP: [&str; 10] = [
    "connection",
    "proxy-connection",
    "keep-alive",
    "transfer-encoding",
    "te",
    "trailer",
    "upgrade",
    "host",
    "content-length",
    "expect",
];

/// The shared state each connection borrows.
#[derive(Debug, Clone)]
pub struct Gateway {
    settings: Arc<Settings>,
    client: reqwest::Client,
    journal: Journal,
}

impl Gateway {
    /// Build the upstream client and take a handle on the journal.
    ///
    /// # Errors
    /// Fails if the HTTP client cannot be built.
    pub fn new(settings: Arc<Settings>, journal: Journal) -> anyhow::Result<Self> {
        // reqwest is built `rustls-no-provider`, so no provider is installed for us; `ring` is
        // what the rest of the workspace selects. Already-installed is the outcome wanted, not an
        // error.
        rustls::crypto::ring::default_provider()
            .install_default()
            .ok();

        let client = reqwest::Client::builder()
            .connect_timeout(CONNECT_TIMEOUT)
            .read_timeout(READ_TIMEOUT)
            // Every byte matters the moment it exists: an SSE chunk held back for a fuller packet
            // is a token that reaches the reader late.
            .tcp_nodelay(true)
            .build()?;

        Ok(Self {
            settings,
            client,
            journal,
        })
    }

    /// Serve one connection: read the request, forward it, stream the answer back, journal it.
    ///
    /// # Errors
    /// Fails only on a client socket error. A provider that refuses or times out is a `502` to the
    /// client and a row with an `error`, not an error here.
    pub async fn handle(&self, mut stream: TcpStream, peer: SocketAddr) -> anyhow::Result<()> {
        let Some(request) = Reader::new(&mut stream).request().await? else {
            return Ok(());
        };

        let started_at = journal::now_ms();
        let start = Instant::now();

        // The index doubles as the liveness answer and is deliberately not journalled: it says
        // nothing about model traffic, and it is what anything polling the port will ask for.
        if request.target == "/" {
            return http::write_json(&mut stream, 200, "OK", &self.index()).await;
        }

        let Some(route) = self.settings.resolve(&request.target) else {
            let body = serde_json::json!({
                "error": "no route",
                "detail": format!(
                    "{} does not start with a provider this gateway serves",
                    request.target
                ),
                "providers": self.settings.prefixes(),
            })
            .to_string();
            self.record_unrouted(&request, peer, started_at, start);
            return http::write_json(&mut stream, 404, "Not Found", &body).await;
        };

        let cap = self.settings.max_logged_body;
        let (model, wants_stream) = journal::model_and_stream(&request.body);
        let mut entry = Entry {
            started_at,
            provider: route.provider.clone(),
            method: request.method.clone(),
            target: request.target.clone(),
            upstream: route.url.clone(),
            model,
            streamed: wants_stream,
            client: Some(peer.to_string()),
            request_headers: journal::headers_json(&request.headers),
            request_body: journal::body_text(&request.body, cap),
            ..Entry::default()
        };

        let result = self
            .exchange(&mut stream, &request, &route.url, &mut entry)
            .await;
        entry.duration_ms = u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX);

        if let Err(e) = &result {
            entry.error = Some(e.to_string());
        }
        info!(
            provider = %route.provider,
            method = %request.method,
            target = %request.target,
            status = entry.status.unwrap_or(0),
            bytes = entry.response_bytes,
            ms = entry.duration_ms,
            "proxied"
        );
        self.journal.record(entry);
        result
    }

    /// Forward one request and stream the answer back, filling in the response half of `entry`.
    async fn exchange(
        &self,
        stream: &mut TcpStream,
        request: &http::Request,
        url: &str,
        entry: &mut Entry,
    ) -> anyhow::Result<()> {
        let Ok(method) = reqwest::Method::from_bytes(request.method.as_bytes()) else {
            let body = serde_json::json!({"error": "bad method"}).to_string();
            entry.status = Some(400);
            return http::write_json(stream, 400, "Bad Request", &body).await;
        };

        let mut outbound = self.client.request(method, url).body(request.body.clone());
        for (field, value) in &request.headers {
            if forward_to_upstream(field) {
                outbound = outbound.header(field, value);
            }
        }

        let mut response = match outbound.send().await {
            Ok(response) => response,
            Err(e) => {
                // The provider's own failure, reported as this gateway's — with the cause, because
                // a client that sees a bare 502 has no way to tell a DNS failure from a timeout.
                warn!(%url, error = %e, "upstream request failed");
                entry.status = Some(502);
                entry.error = Some(e.to_string());
                let body =
                    serde_json::json!({"error": "upstream request failed", "detail": e.to_string()})
                        .to_string();
                return http::write_json(stream, 502, "Bad Gateway", &body).await;
            }
        };

        let status = response.status();
        entry.status = Some(status.as_u16());
        entry.response_headers = Some(response_headers_json(&response));
        if response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|v| v.contains("event-stream"))
        {
            entry.streamed = true;
        }

        let mut head = format!(
            "HTTP/1.1 {} {}\r\n",
            status.as_u16(),
            status.canonical_reason().unwrap_or("Unknown")
        );
        for (field, value) in response.headers() {
            let name = field.as_str();
            if !return_to_client(name) {
                continue;
            }
            if let Ok(value) = value.to_str() {
                let _ = write!(head, "{name}: {value}\r\n");
            }
        }
        head.push_str("Connection: close\r\n\r\n");
        stream.write_all(head.as_bytes()).await?;
        // Before a single body byte: an SSE client acts on the head, and holding it back until the
        // first token is what makes a working stream look like a hung one.
        stream.flush().await?;

        let cap = self.settings.max_logged_body;
        let mut tee = Vec::new();
        loop {
            let chunk = match response.chunk().await {
                Ok(Some(chunk)) => chunk,
                Ok(None) => break,
                Err(e) => {
                    warn!(%url, error = %e, "upstream stream ended early");
                    entry.error = Some(e.to_string());
                    break;
                }
            };
            entry.response_bytes += chunk.len() as u64;
            if tee.len() < cap {
                let room = cap - tee.len();
                tee.extend_from_slice(&chunk[..chunk.len().min(room)]);
            }
            stream.write_all(&chunk).await?;
            stream.flush().await?;
        }

        entry.response_body = journal::body_text(&tee, cap).map(|text| {
            if entry.response_bytes > tee.len() as u64 {
                format!(
                    "{text}\n<truncated: {} of {} bytes kept>",
                    tee.len(),
                    entry.response_bytes
                )
            } else {
                text
            }
        });

        let _ = stream.shutdown().await;
        Ok(())
    }

    /// A request whose base URL names no provider here — journalled, because a client pointed at
    /// the wrong path is exactly the thing this table should make visible.
    fn record_unrouted(
        &self,
        request: &http::Request,
        peer: SocketAddr,
        started_at: u64,
        start: Instant,
    ) {
        self.journal.record(Entry {
            started_at,
            duration_ms: u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX),
            provider: "unrouted".to_string(),
            method: request.method.clone(),
            target: request.target.clone(),
            upstream: String::new(),
            status: Some(404),
            client: Some(peer.to_string()),
            request_headers: journal::headers_json(&request.headers),
            request_body: journal::body_text(&request.body, self.settings.max_logged_body),
            error: Some("no route".to_string()),
            ..Entry::default()
        });
    }

    /// What `GET /` answers: the routes, so a misconfigured base URL is one curl from explaining
    /// itself.
    fn index(&self) -> String {
        serde_json::json!({
            "service": "adi-llm-gateway",
            "usage": "point a client's base URL at http://llm.adi/<provider>",
            "routes": self.settings.routes,
        })
        .to_string()
    }
}

/// Whether a client header is replayed at the provider.
///
/// `accept-encoding` is dropped on purpose: nothing here decompresses, so asking for gzip would
/// buy loopback bandwidth we do not need and cost the journal every readable body it has.
fn forward_to_upstream(field: &str) -> bool {
    !HOP_BY_HOP
        .iter()
        .any(|name| field.eq_ignore_ascii_case(name))
        && !field.eq_ignore_ascii_case("accept-encoding")
}

/// Whether a provider header is passed back to the client. `content-length` *is* — the body is
/// forwarded byte for byte, so the provider's own length still describes it.
fn return_to_client(field: &str) -> bool {
    ![
        "connection",
        "keep-alive",
        "transfer-encoding",
        "trailer",
        "upgrade",
    ]
    .iter()
    .any(|name| field.eq_ignore_ascii_case(name))
}

/// The provider's response headers as JSON, redacted the same way the request's are.
fn response_headers_json(response: &reqwest::Response) -> String {
    let headers: Vec<(String, String)> = response
        .headers()
        .iter()
        .filter_map(|(field, value)| {
            value
                .to_str()
                .ok()
                .map(|value| (field.as_str().to_string(), value.to_string()))
        })
        .collect();
    journal::headers_json(&headers)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_upstream_leg_drops_hop_by_hop_headers_and_asks_for_no_encoding() {
        assert!(forward_to_upstream("x-api-key"));
        assert!(forward_to_upstream("anthropic-version"));
        assert!(!forward_to_upstream("Host"));
        assert!(!forward_to_upstream("Content-Length"));
        assert!(!forward_to_upstream("Accept-Encoding"));
    }

    #[test]
    fn the_client_leg_keeps_content_length_and_drops_the_framing_headers() {
        assert!(return_to_client("content-length"));
        assert!(return_to_client("content-type"));
        assert!(!return_to_client("Transfer-Encoding"));
        assert!(!return_to_client("Connection"));
    }
}
