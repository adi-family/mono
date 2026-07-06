//! A tiny HTTP/1.1 request reader and response writer — enough for a JSON API and a
//! single-page app, with no web framework (the same hand-rolled approach as adi-hive's
//! proxy). Every response sets `Connection: close`, so each request is its own
//! connection: no keep-alive body-framing to track, which keeps this small and correct.

use std::collections::HashMap;
use std::time::Duration;

use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::TcpStream;

/// Cap the request head so a client that never sends the blank line can't grow memory.
const MAX_HEAD: usize = 32 * 1024;

/// Cap the request body we'll buffer (API payloads are tiny).
const MAX_BODY: usize = 1 << 20; // 1 MiB

/// So a silent client can't tie up a connection forever.
const READ_TIMEOUT: Duration = Duration::from_secs(15);

/// A parsed request: method, full path (query included), and the buffered body. Headers
/// are consumed during parsing (only `Content-Length` matters here) and not retained.
#[derive(Debug)]
pub struct Request {
    pub method: String,
    pub path: String,
    pub body: Vec<u8>,
}

impl Request {
    /// The path with any `?query` stripped — what routing matches on.
    #[must_use]
    pub fn route_path(&self) -> &str {
        self.path.split('?').next().unwrap_or(&self.path)
    }
}

/// Read one request from `stream`. Returns `Ok(None)` if the peer closed before sending
/// anything (an idle connection), so the caller can just drop it.
///
/// # Errors
///
/// Fails on a read/timeout error, a head larger than [`MAX_HEAD`], or a connection that
/// closes mid-head.
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
        .unwrap_or(0)
        .min(MAX_BODY);
    let body_start = head_end + 4; // past the "\r\n\r\n"
    let mut body = buf.get(body_start..).unwrap_or(&[]).to_vec();
    while body.len() < content_length {
        let n = tokio::time::timeout(READ_TIMEOUT, stream.read(&mut chunk)).await??;
        if n == 0 {
            break;
        }
        body.extend_from_slice(&chunk[..n]);
    }
    body.truncate(content_length);

    Ok(Some(Request { method, path, body }))
}

fn find_head_end(buf: &[u8]) -> Option<usize> {
    buf.windows(4).position(|w| w == b"\r\n\r\n")
}

/// Parse the request line and headers out of the raw head (everything before the blank
/// line). Header names are lowercased for case-insensitive lookup.
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
///
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

/// Write a JSON response with the given status.
///
/// # Errors
///
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
///
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
        404 => "Not Found",
        405 => "Method Not Allowed",
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

    #[test]
    fn route_path_strips_query() {
        let req = Request {
            method: "GET".into(),
            path: "/api/ports?live=1".into(),
            body: Vec::new(),
        };
        assert_eq!(req.route_path(), "/api/ports");
    }

    #[test]
    fn finds_the_head_terminator() {
        assert_eq!(find_head_end(b"GET / HTTP/1.1\r\n\r\nBODY"), Some(14));
        assert_eq!(find_head_end(b"GET / HTTP/1.1\r\n"), None);
    }
}
