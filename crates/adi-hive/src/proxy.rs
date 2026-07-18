//! The reverse-proxy core: accept an HTTP/1.x connection, read its request head, pick an
//! upstream by `Host` header, then splice bytes both ways. Hand-rolled L7 proxy — the head
//! is parsed only far enough to route; original bytes are forwarded unchanged.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::{TcpListener, TcpStream};
use tracing::{debug, info, warn};

use crate::config::ResolvedRoute;

/// Caps per-connection memory against a client that never sends the blank line.
const MAX_HEAD: usize = 16 * 1024;

/// So a silent client can't tie up a task forever.
const READ_TIMEOUT: Duration = Duration::from_secs(10);

/// The host → upstream routing table, built once at startup and shared across tasks.
#[derive(Debug)]
pub struct Router {
    routes: Vec<(String, SocketAddr)>,
}

impl Router {
    #[must_use]
    pub fn new(routes: &[ResolvedRoute]) -> Self {
        Self {
            routes: routes
                .iter()
                .map(|r| (r.host.trim().to_ascii_lowercase(), r.upstream))
                .collect(),
        }
    }

    /// Match a raw `Host` header value (with an optional `:port`) to an upstream.
    fn resolve(&self, host: &str) -> Option<SocketAddr> {
        let host = host.trim().to_ascii_lowercase();
        let host = host.split(':').next().unwrap_or(&host);
        self.routes
            .iter()
            .find(|(h, _)| h == host)
            .map(|(_, addr)| *addr)
    }
}

/// Accept loop for one listener; per-connection errors are logged, not returned, until the task is aborted.
pub async fn serve(listener: TcpListener, router: Arc<Router>) {
    loop {
        match listener.accept().await {
            Ok((stream, peer)) => {
                let router = Arc::clone(&router);
                tokio::spawn(async move {
                    if let Err(e) = handle(stream, &router).await {
                        debug!(%peer, error = %e, "proxy connection error");
                    }
                });
            }
            Err(e) => {
                // Don't spin the loop hot on a transient accept error.
                warn!(error = %e, "accept failed");
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        }
    }
}

async fn handle(mut client: TcpStream, router: &Router) -> anyhow::Result<()> {
    let head = read_head(&mut client).await?;

    let Some(host) = extract_host(&head) else {
        return respond_error(&mut client, 400, "Bad Request", "Missing Host header.").await;
    };
    let Some(upstream) = router.resolve(&host) else {
        // Reached the front door but no app answers this host: 404 fallback page (distinct
        // from the 502 below, which means the app exists but its upstream is down).
        info!(%host, "no route");
        return respond_not_found(&mut client).await;
    };

    let mut server = match TcpStream::connect(upstream).await {
        Ok(s) => s,
        Err(e) => {
            warn!(%host, %upstream, error = %e, "upstream connect failed");
            return respond_error(&mut client, 502, "Bad Gateway", "Upstream is unavailable.")
                .await;
        }
    };
    debug!(%host, %upstream, "proxying");

    // Forward the head bytes we already consumed, then splice the rest both ways.
    server.write_all(&head).await?;

    let (mut cread, mut cwrite) = client.split();
    let (mut sread, mut swrite) = server.split();
    let client_to_server = async {
        let _ = tokio::io::copy(&mut cread, &mut swrite).await;
        let _ = swrite.shutdown().await;
    };
    let server_to_client = async {
        let _ = tokio::io::copy(&mut sread, &mut cwrite).await;
        let _ = cwrite.shutdown().await;
    };
    tokio::join!(client_to_server, server_to_client);
    Ok(())
}

/// Read until the blank line ending the head, a size cap, or a timeout; the returned buffer is forwarded verbatim (may include first body bytes).
async fn read_head(stream: &mut TcpStream) -> anyhow::Result<Vec<u8>> {
    use anyhow::Context as _;
    let mut buf = Vec::new();
    let mut chunk = [0u8; 1024];
    loop {
        let n = tokio::time::timeout(READ_TIMEOUT, stream.read(&mut chunk))
            .await
            .context("timed out reading request head")?
            .context("reading request head")?;
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&chunk[..n]);
        if head_complete(&buf) || buf.len() >= MAX_HEAD {
            break;
        }
    }
    Ok(buf)
}

fn head_complete(buf: &[u8]) -> bool {
    buf.windows(4).any(|w| w == b"\r\n\r\n")
}

/// Pull the `Host` header value out of a raw request head (case-insensitive field name).
fn extract_host(head: &[u8]) -> Option<String> {
    let text = String::from_utf8_lossy(head);
    for line in text.split("\r\n") {
        if line.is_empty() {
            break;
        }
        if let Some((name, value)) = line.split_once(':')
            && name.trim().eq_ignore_ascii_case("host")
        {
            let value = value.trim();
            if !value.is_empty() {
                return Some(value.to_string());
            }
        }
    }
    None
}

/// Serve the animated `4XX` fallback page with a `404` — `Host` matched no configured route.
async fn respond_not_found(stream: &mut TcpStream) -> anyhow::Result<()> {
    let body = crate::notfound::PAGE;
    let response = format!(
        "HTTP/1.1 404 Not Found\r\n\
         Content-Type: text/html; charset=utf-8\r\n\
         Content-Length: {len}\r\n\
         Cache-Control: no-store\r\n\
         Connection: close\r\n\
         \r\n\
         {body}",
        len = body.len(),
    );
    stream.write_all(response.as_bytes()).await?;
    stream.flush().await?;
    let _ = stream.shutdown().await;
    Ok(())
}

/// Write a small self-contained HTML error page and close (used for `502` upstream-down or `400` malformed).
async fn respond_error(
    stream: &mut TcpStream,
    code: u16,
    reason: &str,
    message: &str,
) -> anyhow::Result<()> {
    let body = error_page(code, reason, message);
    let response = format!(
        "HTTP/1.1 {code} {reason}\r\n\
         Content-Type: text/html; charset=utf-8\r\n\
         Content-Length: {len}\r\n\
         Cache-Control: no-store\r\n\
         Connection: close\r\n\
         \r\n\
         {body}",
        len = body.len(),
    );
    stream.write_all(response.as_bytes()).await?;
    stream.flush().await?;
    let _ = stream.shutdown().await;
    Ok(())
}

fn error_page(code: u16, reason: &str, message: &str) -> String {
    format!(
        "<!doctype html>\n\
         <html lang=\"en\">\n\
         <head>\n\
         <meta charset=\"utf-8\">\n\
         <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n\
         <title>{code} {reason}</title>\n\
         <style>\n\
           :root {{ --bg:#fafafb; --fg:#0d0f12; --muted:#6b7280; --line:#e5e7eb; }}\n\
           @media (prefers-color-scheme: dark) {{\n\
             :root {{ --bg:#0a0b0d; --fg:#e9ecf1; --muted:#8b919c; --line:#23262b; }}\n\
           }}\n\
           html,body {{ height:100%; }}\n\
           body {{ margin:0; min-height:100vh; display:flex; align-items:center;\n\
             justify-content:center; padding:40px 24px;\n\
             background:var(--bg); color:var(--fg); letter-spacing:-.006em;\n\
             -webkit-font-smoothing:antialiased;\n\
             font:13.5px/1.45 ui-sans-serif, system-ui, -apple-system, \"Segoe UI\", Roboto, Helvetica, Arial, sans-serif; }}\n\
           .wrap {{ display:flex; flex-direction:column; align-items:center; gap:12px; text-align:center; }}\n\
           .line {{ display:flex; align-items:center; }}\n\
           .code {{ font-size:20px; font-weight:600; letter-spacing:-.02em;\n\
             font-variant-numeric:tabular-nums; }}\n\
           .reason {{ margin-left:14px; padding-left:14px; border-left:1px solid var(--line);\n\
             color:var(--muted); }}\n\
           .msg {{ margin:0; color:var(--muted); max-width:34rem; }}\n\
         </style>\n\
         </head>\n\
         <body>\n\
           <div class=\"wrap\">\n\
             <div class=\"line\">\n\
               <span class=\"code\">{code}</span>\n\
               <span class=\"reason\">{reason}</span>\n\
             </div>\n\
             <p class=\"msg\">{message}</p>\n\
           </div>\n\
         </body>\n\
         </html>\n"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn router() -> Router {
        Router::new(&[
            ResolvedRoute {
                host: "App.Test".to_string(),
                upstream: "127.0.0.1:8010".parse().unwrap(),
            },
            ResolvedRoute {
                host: "api.test".to_string(),
                upstream: "127.0.0.1:8009".parse().unwrap(),
            },
        ])
    }

    #[test]
    fn resolves_host_case_insensitively_and_ignores_port() {
        let r = router();
        assert_eq!(r.resolve("app.test"), "127.0.0.1:8010".parse().ok());
        assert_eq!(r.resolve("APP.TEST"), "127.0.0.1:8010".parse().ok());
        assert_eq!(r.resolve("app.test:8080"), "127.0.0.1:8010".parse().ok());
        assert_eq!(r.resolve("api.test"), "127.0.0.1:8009".parse().ok());
        assert_eq!(r.resolve("unknown.test"), None);
    }

    #[test]
    fn extracts_host_from_a_request_head() {
        let head = b"GET /path HTTP/1.1\r\nHost: app.adi\r\nAccept: */*\r\n\r\n";
        assert_eq!(extract_host(head).as_deref(), Some("app.adi"));
    }

    #[test]
    fn extracts_host_ignoring_field_name_case() {
        let head = b"GET / HTTP/1.1\r\nhOsT:   api.adi:8080  \r\n\r\n";
        assert_eq!(extract_host(head).as_deref(), Some("api.adi:8080"));
    }

    #[test]
    fn missing_host_yields_none() {
        let head = b"GET / HTTP/1.1\r\nAccept: */*\r\n\r\n";
        assert_eq!(extract_host(head), None);
    }

    #[test]
    fn detects_end_of_head() {
        assert!(head_complete(b"GET / HTTP/1.1\r\nHost: a.adi\r\n\r\n"));
        assert!(!head_complete(b"GET / HTTP/1.1\r\nHost: a.adi\r\n"));
    }

    #[test]
    fn error_page_is_self_contained() {
        let page = error_page(502, "Bad Gateway", "No upstream.");
        assert!(page.starts_with("<!doctype html>"));
        assert!(page.contains("502"));
        assert!(page.contains("Bad Gateway"));
        assert!(!page.contains("http://"), "no external refs");
    }
}
