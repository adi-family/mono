//! A tiny built-in HTTP server that answers *every* request with a styled
//! "not found" page for the resolver's domain.
//!
//! Why it exists: `adi-dns` maps the whole `.<domain>` suffix to a single
//! loopback address (split-DNS). A browser that opens `http://anything.<domain>/`
//! then makes an HTTP request to that address — and if nothing is listening it
//! just gets a bare connection error. Pointing the override at an address this
//! server owns turns that dead end into a clear page ("no such `.<domain>` app is
//! registered"), and leaves room to route real apps here later.
//!
//! It is deliberately dependency-free: a hand-rolled HTTP/1.x responder over the
//! tokio runtime the resolver already uses. It reads the request head, pulls out
//! the `Host` and path for the message, and writes one fixed `404` response.
//! Binding a privileged port (`:80`) or a non-`127.0.0.1` loopback alias needs
//! root; a bind failure is non-fatal — the DNS side keeps serving regardless.

use std::net::SocketAddr;
use std::time::Duration;

use anyhow::Context as _;
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::{TcpListener, TcpStream};
use tracing::{debug, info, warn};

/// Max bytes of request head we read before responding (headers only — any body
/// is ignored). Caps memory per connection against a client that never sends the
/// terminating blank line.
const MAX_HEAD: usize = 8 * 1024;

/// Per-connection read timeout, so a silent client can't tie up a task forever.
const READ_TIMEOUT: Duration = Duration::from_secs(5);

/// Bind `addr` and serve the not-found page for `.domain` until the task is
/// dropped. Each accepted connection is handled on its own task.
///
/// # Errors
/// Returns an error only if the listener cannot be bound (e.g. the port needs
/// root, or the loopback alias for a non-`127.0.0.1` address is missing). Once
/// bound, per-connection errors are logged and never bubble up.
pub async fn serve(addr: SocketAddr, domain: String) -> anyhow::Result<()> {
    let listener = TcpListener::bind(addr)
        .await
        .with_context(|| format!("binding landing HTTP listener on {addr}"))?;
    info!(%addr, domain = %domain, "landing HTTP server listening");

    loop {
        match listener.accept().await {
            Ok((stream, peer)) => {
                let domain = domain.clone();
                tokio::spawn(async move {
                    if let Err(e) = handle(stream, &domain).await {
                        debug!(%peer, error = %e, "landing connection error");
                    }
                });
            }
            Err(e) => {
                // A transient accept error shouldn't spin the loop hot.
                warn!(error = %e, "landing accept failed");
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        }
    }
}

/// Read the request head, then write the 404 page and close the connection.
async fn handle(mut stream: TcpStream, domain: &str) -> anyhow::Result<()> {
    let head = read_head(&mut stream).await?;
    let (path, host) = parse_request(&head);
    let body = not_found_page(domain, host.as_deref(), path.as_deref());
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
    // Best-effort half-close; the client may already be gone.
    let _ = stream.shutdown().await;
    Ok(())
}

/// Read bytes until the blank line that ends the HTTP head, a size cap, or a
/// timeout — whichever comes first. Returned lossily as text (we only ever read
/// ASCII header lines out of it).
async fn read_head(stream: &mut TcpStream) -> anyhow::Result<String> {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 1024];
    loop {
        let n = tokio::time::timeout(READ_TIMEOUT, stream.read(&mut chunk))
            .await
            .context("timed out reading request head")?
            .context("reading request head")?;
        if n == 0 {
            break; // client closed
        }
        buf.extend_from_slice(&chunk[..n]);
        if head_complete(&buf) || buf.len() >= MAX_HEAD {
            break;
        }
    }
    Ok(String::from_utf8_lossy(&buf).into_owned())
}

/// True once the buffer contains the CRLFCRLF that terminates the request head.
fn head_complete(buf: &[u8]) -> bool {
    buf.windows(4).any(|w| w == b"\r\n\r\n")
}

/// Extract `(path, host)` from a raw request head. Both are optional — a client
/// that sends nothing useful still gets a page with sensible defaults.
fn parse_request(head: &str) -> (Option<String>, Option<String>) {
    let mut lines = head.lines();
    // Request line: `GET /some/path HTTP/1.1`.
    let path = lines
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .map(str::to_owned);
    // First `Host:` header, case-insensitive.
    let host = head.lines().skip(1).find_map(|line| {
        let (name, value) = line.split_once(':')?;
        name.trim()
            .eq_ignore_ascii_case("host")
            .then(|| value.trim().to_owned())
    });
    (path, host)
}

/// Minimal HTML-attribute/text escaping for the reflected host and path.
fn html_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(c),
        }
    }
    out
}

/// The full standalone HTML page. Self-contained (inline CSS), theme-aware.
fn not_found_page(domain: &str, host: Option<&str>, path: Option<&str>) -> String {
    let host = html_escape(host.unwrap_or("(unknown host)"));
    let path = html_escape(path.unwrap_or("/"));
    let domain = html_escape(domain);
    let version = env!("CARGO_PKG_VERSION");
    format!(
        r#"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>not found &middot; .{domain}</title>
<style>
  :root {{
    --bg: #f6f7f9; --card: #ffffff; --fg: #1b1f24; --muted: #5b636e;
    --line: #e4e7ec; --accent: #d64545; --code: #f0f2f5;
  }}
  @media (prefers-color-scheme: dark) {{
    :root {{
      --bg: #0e1116; --card: #161b22; --fg: #e6edf3; --muted: #8b949e;
      --line: #272e38; --accent: #f0776a; --code: #0b0f14;
    }}
  }}
  * {{ box-sizing: border-box; }}
  body {{
    margin: 0; min-height: 100vh; display: flex; align-items: center;
    justify-content: center; padding: 24px; background: var(--bg); color: var(--fg);
    font: 15px/1.55 -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, Helvetica, Arial, sans-serif;
  }}
  .card {{
    width: 100%; max-width: 560px; background: var(--card);
    border: 1px solid var(--line); border-radius: 14px; padding: 28px 30px;
    box-shadow: 0 1px 2px rgba(0,0,0,.04), 0 8px 30px rgba(0,0,0,.06);
  }}
  .tag {{
    display: inline-block; font-size: 12px; font-weight: 600; letter-spacing: .04em;
    text-transform: uppercase; color: var(--accent); margin-bottom: 10px;
  }}
  h1 {{ margin: 0 0 6px; font-size: 22px; letter-spacing: -.01em; }}
  p {{ margin: 0 0 18px; color: var(--muted); }}
  dl {{ margin: 0; display: grid; grid-template-columns: auto 1fr; gap: 8px 14px; }}
  dt {{ color: var(--muted); font-size: 13px; }}
  dd {{ margin: 0; }}
  code {{
    font: 13px/1.4 ui-monospace, SFMono-Regular, Menlo, Consolas, monospace;
    background: var(--code); border: 1px solid var(--line); border-radius: 6px;
    padding: 1px 6px; word-break: break-all;
  }}
  .hint {{ margin-top: 20px; padding-top: 16px; border-top: 1px solid var(--line);
    font-size: 13px; color: var(--muted); }}
  footer {{ margin-top: 16px; font-size: 12px; color: var(--muted); }}
</style>
</head>
<body>
  <main class="card">
    <span class="tag">404 &middot; not found</span>
    <h1>No <code>.{domain}</code> app is registered here</h1>
    <p>This is the <strong>adi-dns</strong> landing server. The name below resolves
       to this resolver, but nothing is bound to it yet.</p>
    <dl>
      <dt>host</dt><dd><code>{host}</code></dd>
      <dt>path</dt><dd><code>{path}</code></dd>
    </dl>
    <div class="hint">
      Register a service for <code>{host}</code>, or check that the host name is
      spelled correctly.
    </div>
    <footer>adi-dns {version} &middot; split-DNS landing for <code>.{domain}</code></footer>
  </main>
</body>
</html>
"#
    )
}

#[cfg(test)]
mod tests {
    use super::{head_complete, html_escape, not_found_page, parse_request};

    #[test]
    fn detects_end_of_head() {
        assert!(head_complete(b"GET / HTTP/1.1\r\nHost: a.adi\r\n\r\n"));
        assert!(!head_complete(b"GET / HTTP/1.1\r\nHost: a.adi\r\n"));
    }

    #[test]
    fn parses_path_and_host() {
        let head = "GET /foo/bar HTTP/1.1\r\nHost: some.adi\r\nAccept: */*\r\n\r\n";
        let (path, host) = parse_request(head);
        assert_eq!(path.as_deref(), Some("/foo/bar"));
        assert_eq!(host.as_deref(), Some("some.adi"));
    }

    #[test]
    fn host_header_is_case_insensitive() {
        let (_, host) = parse_request("GET / HTTP/1.1\r\nhOsT:  x.adi \r\n\r\n");
        assert_eq!(host.as_deref(), Some("x.adi"));
    }

    #[test]
    fn escapes_reflected_values() {
        assert_eq!(html_escape("<b>&\"x"), "&lt;b&gt;&amp;&quot;x");
    }

    #[test]
    fn page_reflects_escaped_host_and_names_domain() {
        let page = not_found_page("adi", Some("<script>.adi"), Some("/a"));
        assert!(page.contains("&lt;script&gt;.adi"), "host must be escaped");
        assert!(!page.contains("<script>.adi"), "raw host must not leak");
        assert!(page.contains("<code>.adi</code>"), "domain shown");
    }
}
