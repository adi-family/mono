//! A tiny hand-rolled HTTP/1.1 request reader and response writer for the JSON API and
//! SPA. Every response sets `Connection: close`, so each request is its own connection.

use std::collections::HashMap;
use std::time::Duration;

use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::TcpStream;

/// Cap the request head so a client that never sends the blank line can't grow memory.
const MAX_HEAD: usize = 32 * 1024;

/// Cap the request body we'll buffer.
///
/// Nearly every API payload is a few hundred bytes. Two are not: a dashboard arriving from another
/// machine (`POST /api/dashboards/import`), which carries the directory's files as base64 and is
/// packed no larger than 4 MiB before that third is added; and a file somebody attached to a message
/// (`POST /api/agents/attachment`), which is the raw bytes of whatever they dragged in. The
/// attachment store caps those at 25 MiB, so this sits above it — an oversized upload should be
/// refused by the handler, which knows what the file was called, rather than by this reader, which
/// does not.
const MAX_BODY: usize = 32 << 20; // 32 MiB

/// So a silent client can't tie up a connection forever.
const READ_TIMEOUT: Duration = Duration::from_secs(15);

/// A parsed request: method, full path (query included), the headers (names lowercased), the
/// buffered body, and whatever arrived after it.
#[derive(Debug)]
pub struct Request {
    pub method: String,
    pub path: String,
    pub headers: HashMap<String, String>,
    pub body: Vec<u8>,
    /// Bytes read past the body — nothing for an ordinary request, which is one request per
    /// connection, but a client that pipelines straight into a protocol switch (the WebSocket
    /// upgrade at `/api/ws`) may have sent its first frames already. Dropping them would lose
    /// that client's opening message.
    pub rest: Vec<u8>,
}

impl Request {
    /// The path with any `?query` stripped — what routing matches on.
    #[must_use]
    pub fn route_path(&self) -> &str {
        self.path.split('?').next().unwrap_or(&self.path)
    }

    /// One query parameter's raw value, by exact name.
    ///
    /// Raw: nothing is percent-decoded, so this suits the short identifiers routing asks about
    /// (`?engine=openai`) and not free text. A parameter repeated in the query yields its first
    /// occurrence, and one given without `=` reads as empty rather than absent.
    #[must_use]
    pub fn query_param(&self, name: &str) -> Option<&str> {
        let query = self.path.split_once('?').map(|(_, q)| q)?;
        query.split('&').find_map(|pair| {
            let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
            (key == name).then_some(value)
        })
    }

    /// One header's value, by lowercase name.
    #[must_use]
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.get(name).map(String::as_str)
    }

    /// Whether this request asks to leave HTTP behind for a WebSocket (RFC 6455 §4.2.1: an
    /// `Upgrade: websocket` token and `websocket` in `Connection`, both case-insensitive).
    #[must_use]
    pub fn is_websocket_upgrade(&self) -> bool {
        let upgrade = self
            .header("upgrade")
            .is_some_and(|v| v.eq_ignore_ascii_case("websocket"));
        let connection = self.header("connection").is_some_and(|v| {
            v.split(',')
                .any(|token| token.trim().eq_ignore_ascii_case("upgrade"))
        });
        upgrade && connection
    }
}

/// Read one request from `stream`; `Ok(None)` if the peer closed while idle.
///
/// # Errors
/// Fails on a read/timeout error, an oversized head, or a connection closed mid-head.
pub async fn read_request(stream: &mut TcpStream) -> anyhow::Result<Option<Request>> {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 2048];
    let head_end = loop {
        let n = tokio::time::timeout(READ_TIMEOUT, stream.read(&mut chunk)).await??;
        if n == 0 {
            if buf.is_empty() {
                return Ok(None);
            }
            anyhow::bail!("connection closed mid-head");
        }
        buf.extend_from_slice(&chunk[..n]);
        if let Some(pos) = find_head_end(&buf) {
            break pos;
        }
        anyhow::ensure!(buf.len() <= MAX_HEAD, "request head too large");
    };

    let (method, path, headers) = parse_head(&buf[..head_end]);

    let content_length = headers
        .get("content-length")
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(0);
    // Refused, not truncated. Clamping used to leave the excess unread in the socket, so an
    // oversized body surfaced as a JSON parse error about a document that was simply cut in half.
    anyhow::ensure!(
        content_length <= MAX_BODY,
        "request body of {content_length} bytes is over the {MAX_BODY}-byte limit"
    );
    let body_start = head_end + 4; // past the "\r\n\r\n"
    let mut body = buf.get(body_start..).unwrap_or(&[]).to_vec();
    while body.len() < content_length {
        let n = tokio::time::timeout(READ_TIMEOUT, stream.read(&mut chunk)).await??;
        if n == 0 {
            break;
        }
        body.extend_from_slice(&chunk[..n]);
    }
    // Keep, rather than discard, anything past the declared body — see [`Request::rest`].
    let rest = if body.len() > content_length {
        body.split_off(content_length)
    } else {
        Vec::new()
    };

    Ok(Some(Request {
        method,
        path,
        headers,
        body,
        rest,
    }))
}

fn find_head_end(buf: &[u8]) -> Option<usize> {
    buf.windows(4).position(|w| w == b"\r\n\r\n")
}

/// Parse the request line and headers out of the raw head; header names are lowercased.
fn parse_head(head: &[u8]) -> (String, String, HashMap<String, String>) {
    let text = String::from_utf8_lossy(head);
    let mut lines = text.split("\r\n");
    let mut request_line = lines.next().unwrap_or_default().split_whitespace();
    let method = request_line.next().unwrap_or_default().to_string();
    let path = request_line.next().unwrap_or("/").to_string();

    let mut headers = HashMap::new();
    for line in lines {
        if let Some((name, value)) = line.split_once(':') {
            headers.insert(name.trim().to_ascii_lowercase(), value.trim().to_string());
        }
    }
    (method, path, headers)
}

/// Write a full response and close the connection.
///
/// # Errors
/// Fails if the socket write fails.
pub async fn write_response(
    stream: &mut TcpStream,
    status: u16,
    reason: &str,
    content_type: &str,
    body: &[u8],
) -> anyhow::Result<()> {
    let head = format!(
        "HTTP/1.1 {status} {reason}\r\n\
         Content-Type: {content_type}\r\n\
         Content-Length: {len}\r\n\
         Cache-Control: no-store\r\n\
         Connection: close\r\n\
         \r\n",
        len = body.len(),
    );
    stream.write_all(head.as_bytes()).await?;
    stream.write_all(body).await?;
    stream.flush().await?;
    let _ = stream.shutdown().await;
    Ok(())
}

/// Write a body that may be cached for a year — for content whose address *is* its version.
///
/// Every other answer this server writes carries `no-store`, which is right for state that is polled
/// and can change under the reader. An attachment cannot: its id is minted from random bytes when
/// the bytes are stored, and nothing ever writes different bytes under the same id. Without this the
/// page redraws a chat every second and re-fetches every screenshot in it every time.
///
/// # Errors
/// Fails if the socket write fails.
/// `disposition` is `inline` for what the browser may render in a tab and `attachment` for what it
/// must download instead — see [`serve_attachment`](crate::serve_attachment), which decides which.
pub async fn write_cached(
    stream: &mut TcpStream,
    content_type: &str,
    disposition: &str,
    body: &[u8],
) -> anyhow::Result<()> {
    let head = format!(
        "HTTP/1.1 200 OK\r\n\
         Content-Type: {content_type}\r\n\
         Content-Length: {len}\r\n\
         Content-Disposition: {disposition}\r\n\
         X-Content-Type-Options: nosniff\r\n\
         Cache-Control: private, max-age=31536000, immutable\r\n\
         Connection: close\r\n\
         \r\n",
        len = body.len(),
    );
    stream.write_all(head.as_bytes()).await?;
    stream.write_all(body).await?;
    stream.flush().await?;
    let _ = stream.shutdown().await;
    Ok(())
}

/// Write a JSON response with the given status.
///
/// # Errors
/// Fails if the socket write fails.
pub async fn write_json(stream: &mut TcpStream, status: u16, json: &str) -> anyhow::Result<()> {
    let reason = reason_phrase(status);
    write_response(
        stream,
        status,
        reason,
        "application/json; charset=utf-8",
        json.as_bytes(),
    )
    .await
}

/// Write an HTML response with the given status.
///
/// # Errors
/// Fails if the socket write fails.
pub async fn write_html(stream: &mut TcpStream, status: u16, html: &str) -> anyhow::Result<()> {
    let reason = reason_phrase(status);
    write_response(
        stream,
        status,
        reason,
        "text/html; charset=utf-8",
        html.as_bytes(),
    )
    .await
}

fn reason_phrase(status: u16) -> &'static str {
    match status {
        200 => "OK",
        400 => "Bad Request",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        415 => "Unsupported Media Type",
        500 => "Internal Server Error",
        _ => "Unknown",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_method_path_and_headers() {
        let head =
            b"POST /api/ports/reserve?x=1 HTTP/1.1\r\nHost: app.adi\r\nContent-Length: 3\r\n";
        let (method, path, headers) = parse_head(head);
        assert_eq!(method, "POST");
        assert_eq!(path, "/api/ports/reserve?x=1");
        assert_eq!(headers.get("host").map(String::as_str), Some("app.adi"));
        assert_eq!(headers.get("content-length").map(String::as_str), Some("3"));
    }

    /// A request with no headers and no body — the shape the path/upgrade tests need.
    fn bare(method: &str, path: &str) -> Request {
        Request {
            method: method.into(),
            path: path.into(),
            headers: HashMap::new(),
            body: Vec::new(),
            rest: Vec::new(),
        }
    }

    #[test]
    fn route_path_strips_query() {
        assert_eq!(bare("GET", "/api/ports?live=1").route_path(), "/api/ports");
    }

    #[test]
    fn query_param_reads_one_value_out_of_the_query() {
        let req = bare("POST", "/api/voice/transcribe?engine=openai&x=1");
        assert_eq!(req.query_param("engine"), Some("openai"));
        assert_eq!(req.query_param("x"), Some("1"));
        // Absent, versus present-but-empty: the caller distinguishes them, so the type must.
        assert_eq!(req.query_param("nope"), None);
        assert_eq!(bare("GET", "/a?flag").query_param("flag"), Some(""));
        // A path carrying no query at all is not a parse failure, just no parameters.
        assert_eq!(bare("GET", "/a").query_param("engine"), None);
        // A name that is only a prefix of a real one must not match it.
        assert_eq!(bare("GET", "/a?engineer=1").query_param("engine"), None);
    }

    #[test]
    fn recognizes_a_websocket_upgrade() {
        let mut req = bare("GET", "/api/ws");
        assert!(!req.is_websocket_upgrade());
        req.headers.insert("upgrade".into(), "WebSocket".into());
        // Firefox sends `keep-alive, Upgrade`, so the token has to be found in a list.
        req.headers
            .insert("connection".into(), "keep-alive, Upgrade".into());
        assert!(req.is_websocket_upgrade());
    }

    #[test]
    fn finds_the_head_terminator() {
        assert_eq!(find_head_end(b"GET / HTTP/1.1\r\n\r\nBODY"), Some(14));
        assert_eq!(find_head_end(b"GET / HTTP/1.1\r\n"), None);
    }
}
