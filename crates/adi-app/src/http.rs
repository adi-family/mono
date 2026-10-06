//! HTTP/1.1 request parsing and responses for the API and SPA.
//! Responses use `Connection: close`; each connection serves one request.

use std::collections::HashMap;
use std::time::Duration;

use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::TcpStream;

const MAX_HEAD: usize = 32 * 1024;

/// Above the attachment store's 25 MiB limit so its handler can report oversized uploads.
const MAX_BODY: usize = 32 << 20; // 32 MiB

const READ_TIMEOUT: Duration = Duration::from_secs(15);

/// Parsed HTTP request with lowercase header names and the query included in `path`.
#[derive(Debug)]
pub struct Request {
    pub method: String,
    pub path: String,
    pub headers: HashMap<String, String>,
    pub body: Vec<u8>,
    /// Bytes past the body, preserved for clients that pipeline WebSocket frames after upgrade.
    pub rest: Vec<u8>,
}

impl Request {
    #[must_use]
    pub fn route_path(&self) -> &str {
        self.path.split('?').next().unwrap_or(&self.path)
    }

    /// First raw value for an exact query name; no percent-decoding.
    /// A parameter without `=` has an empty value.
    #[must_use]
    pub fn query_param(&self, name: &str) -> Option<&str> {
        let query = self.path.split_once('?').map(|(_, q)| q)?;
        query.split('&').find_map(|pair| {
            let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
            (key == name).then_some(value)
        })
    }

    /// Look up a header by lowercase name.
    #[must_use]
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.get(name).map(String::as_str)
    }

    /// Check case-insensitive WebSocket upgrade tokens (RFC 6455 §4.2.1).
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

/// Read one request; `Ok(None)` means the peer closed while idle.
///
/// # Errors
/// Fails on I/O, timeout, oversized input, or a connection closed mid-head.
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

/// Write a response with `no-store` caching and close the connection.
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
    write_with_cache(stream, status, reason, content_type, "no-store", body).await
}

/// Cache a build asset for a year. Its filename must contain a content hash.
/// Keep the referencing `index.html` uncached so new builds use new asset URLs.
///
/// # Errors
/// Fails if the socket write fails.
pub async fn write_immutable(
    stream: &mut TcpStream,
    content_type: &str,
    body: &[u8],
) -> anyhow::Result<()> {
    let cache = "public, max-age=31536000, immutable";
    write_with_cache(stream, 200, "OK", content_type, cache, body).await
}

async fn write_with_cache(
    stream: &mut TcpStream,
    status: u16,
    reason: &str,
    content_type: &str,
    cache_control: &str,
    body: &[u8],
) -> anyhow::Result<()> {
    let head = format!(
        "HTTP/1.1 {status} {reason}\r\n\
         Content-Type: {content_type}\r\n\
         Content-Length: {len}\r\n\
         Cache-Control: {cache_control}\r\n\
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

/// Cache content privately for a year; its address must never serve different bytes.
/// `disposition` selects inline rendering or download.
///
/// # Errors
/// Fails if the socket write fails.
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

/// Download without caching: report URLs can be reused for newly generated content.
///
/// # Errors
/// Fails if the socket write fails.
pub async fn write_download(
    stream: &mut TcpStream,
    content_type: &str,
    filename: &str,
    body: &[u8],
) -> anyhow::Result<()> {
    let head = format!(
        "HTTP/1.1 200 OK\r\n\
         Content-Type: {content_type}\r\n\
         Content-Length: {len}\r\n\
         Content-Disposition: attachment; filename=\"{filename}\"\r\n\
         X-Content-Type-Options: nosniff\r\n\
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

/// Write a JSON response.
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

/// Write an HTML response.
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
        assert_eq!(req.query_param("nope"), None);
        assert_eq!(bare("GET", "/a?flag").query_param("flag"), Some(""));
        assert_eq!(bare("GET", "/a").query_param("engine"), None);
        assert_eq!(bare("GET", "/a?engineer=1").query_param("engine"), None);
    }

    #[test]
    fn recognizes_a_websocket_upgrade() {
        let mut req = bare("GET", "/api/ws");
        assert!(!req.is_websocket_upgrade());
        req.headers.insert("upgrade".into(), "WebSocket".into());
        // Firefox sends the upgrade token in a list.
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
