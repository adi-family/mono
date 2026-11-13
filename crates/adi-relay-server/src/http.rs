//! Minimal HTTP/1.1 request parsing and response writing, trimmed to exactly what the relay
//! needs: no SPA, no static assets, no WebSocket upgrade, no `Accept-Encoding` negotiation.
//! Mirrors `adi-webapp-api/src/http.rs`'s shape (every other service binary in this workspace
//! already does its own HTTP this way) without depending on that crate — §8's "deliberately not
//! built on adi-db or adi-config" extends to not pulling in the app's own HTTP layer either,
//! which is tied to that crate's SPA-serving and 32 MiB upload limit.

use std::collections::HashMap;
use std::time::Duration;

use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::TcpStream;

const MAX_HEAD: usize = 16 * 1024;

const READ_TIMEOUT: Duration = Duration::from_secs(15);

/// A parsed request, headers lowercase, query string left inside `path`.
#[derive(Debug)]
pub struct Request {
    pub method: String,
    pub path: String,
    pub headers: HashMap<String, String>,
    pub body: Vec<u8>,
}

impl Request {
    #[must_use]
    pub fn route_path(&self) -> &str {
        self.path.split('?').next().unwrap_or(&self.path)
    }

    #[must_use]
    pub fn query_param(&self, name: &str) -> Option<&str> {
        let query = self.path.split_once('?').map(|(_, q)| q)?;
        query.split('&').find_map(|pair| {
            let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
            (key == name).then_some(value)
        })
    }

    #[must_use]
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.get(name).map(String::as_str)
    }
}

/// Read one request, enforcing `max_body_bytes` on the body (the mode's own limit, §6/§9 — not
/// a fixed constant, since public and enterprise instances disagree on it). `Ok(None)` means the
/// peer closed while idle.
///
/// # Errors
/// I/O, timeout, an oversized head, a body over `max_body_bytes`, or a connection closed mid-head.
pub async fn read_request(stream: &mut TcpStream, max_body_bytes: u64) -> anyhow::Result<Option<Request>> {
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
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(0);
    anyhow::ensure!(
        content_length <= max_body_bytes,
        "request body of {content_length} bytes is over the {max_body_bytes}-byte limit"
    );
    #[allow(clippy::cast_possible_truncation)]
    let content_length = content_length as usize;
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

    Ok(Some(Request {
        method,
        path,
        headers,
        body,
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

/// Write a JSON response and close the connection — every response this service ever sends.
///
/// # Errors
/// Fails if the socket write fails.
pub async fn write_json(stream: &mut TcpStream, status: u16, json: &str) -> anyhow::Result<()> {
    let reason = reason_phrase(status);
    let head = format!(
        "HTTP/1.1 {status} {reason}\r\n\
         Content-Type: application/json; charset=utf-8\r\n\
         Content-Length: {len}\r\n\
         Cache-Control: no-store\r\n\
         Connection: close\r\n\
         \r\n",
        len = json.len(),
    );
    stream.write_all(head.as_bytes()).await?;
    stream.write_all(json.as_bytes()).await?;
    stream.flush().await?;
    let _ = stream.shutdown().await;
    Ok(())
}

/// Write a plain-text response — only `GET /healthz` uses this.
///
/// # Errors
/// Fails if the socket write fails.
pub async fn write_text(stream: &mut TcpStream, status: u16, text: &str) -> anyhow::Result<()> {
    let reason = reason_phrase(status);
    let head = format!(
        "HTTP/1.1 {status} {reason}\r\n\
         Content-Type: text/plain; charset=utf-8\r\n\
         Content-Length: {len}\r\n\
         Cache-Control: no-store\r\n\
         Connection: close\r\n\
         \r\n",
        len = text.len(),
    );
    stream.write_all(head.as_bytes()).await?;
    stream.write_all(text.as_bytes()).await?;
    stream.flush().await?;
    let _ = stream.shutdown().await;
    Ok(())
}

fn reason_phrase(status: u16) -> &'static str {
    match status {
        200 => "OK",
        201 => "Created",
        202 => "Accepted",
        204 => "No Content",
        400 => "Bad Request",
        401 => "Unauthorized",
        402 => "Payment Required",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        413 => "Payload Too Large",
        429 => "Too Many Requests",
        500 => "Internal Server Error",
        _ => "Unknown",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_method_path_and_headers() {
        let head = b"POST /v1/endpoints HTTP/1.1\r\nHost: relay\r\nContent-Length: 3\r\n";
        let (method, path, headers) = parse_head(head);
        assert_eq!(method, "POST");
        assert_eq!(path, "/v1/endpoints");
        assert_eq!(headers.get("host").map(String::as_str), Some("relay"));
        assert_eq!(headers.get("content-length").map(String::as_str), Some("3"));
    }

    fn bare(method: &str, path: &str) -> Request {
        Request {
            method: method.into(),
            path: path.into(),
            headers: HashMap::new(),
            body: Vec::new(),
        }
    }

    #[test]
    fn route_path_strips_query() {
        assert_eq!(bare("GET", "/v1/pull?batch=10").route_path(), "/v1/pull");
    }

    #[test]
    fn query_param_reads_one_value() {
        let req = bare("GET", "/v1/pull?batch=10&x=1");
        assert_eq!(req.query_param("batch"), Some("10"));
        assert_eq!(req.query_param("nope"), None);
    }

    #[test]
    fn finds_the_head_terminator() {
        assert_eq!(find_head_end(b"GET / HTTP/1.1\r\n\r\nBODY"), Some(14));
        assert_eq!(find_head_end(b"GET / HTTP/1.1\r\n"), None);
    }
}
