//! The reverse-proxy core: accept an HTTP/1.x connection, read its request head, pick an
//! upstream by `Host` header (and, when a service claims one, by path prefix), then splice bytes
//! both ways. Hand-rolled L7 proxy — the head is parsed only far enough to route; original bytes
//! are forwarded unchanged.
//!
//! *Unchanged* is load-bearing, not laziness. Neither the `Host` header nor the request target is
//! ever rewritten, because both may travel on to a remote node: rewriting `Host` would make that
//! node's absolute redirects point at a same-named host on the *viewer's* machine, and stripping a
//! path prefix would make a dashboard's backend answer at a different URL than the page asked for.
//! What the front door matched and what the upstream reads are the same bytes.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncReadExt as _, AsyncWrite, AsyncWriteExt as _};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::watch;
use tokio_rustls::TlsAcceptor;
use tracing::{debug, info, warn};

/// What the proxy needs of a client connection: bytes both ways, and nothing else. Implemented by
/// [`TcpStream`] for the plain front door and by `TlsStream` for the HTTPS one, so both share every
/// line of routing, error-page and splicing logic below.
pub trait ClientStream: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send> ClientStream for T {}

use crate::config::{ResolvedRoute, host_key, is_mesh_host, path_prefix};

/// Caps per-connection memory against a client that never sends the blank line.
const MAX_HEAD: usize = 16 * 1024;

/// So a silent client can't tie up a task forever.
const READ_TIMEOUT: Duration = Duration::from_secs(10);

/// One entry of the routing table, keyed by `(host, path prefix)`.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Route {
    host: String,
    /// `None` is the host's fallback: it answers whatever no prefix claimed.
    path: Option<String>,
    upstream: SocketAddr,
}

/// Where a request goes once its `Host` (and target) have been matched. Separate from the act of
/// connecting so the decision is a pure function the tests — and the mesh gateway, which reuses
/// this table — can exercise without a socket.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    /// A local service's upstream on loopback.
    Service(SocketAddr),
    /// A remote node: hand the connection to the local mesh gateway, head verbatim. The front door
    /// does not parse the node or service out of the name — that is the gateway's job, and keeping
    /// it there is what makes one rule cover the entire fleet.
    Mesh(SocketAddr),
    /// A `*.n.adi` host on a machine with no mesh gateway configured. Distinct from
    /// [`Self::NoRoute`] because the answer is different: the name is *valid*, this machine simply
    /// cannot reach it yet.
    MeshUnavailable,
    /// Nothing claims this host.
    NoRoute,
}

/// The `(host, path) → upstream` routing table. Shared through a [`watch`] channel so the config
/// reloader can hot-swap it: a service added on disk (with a `proxy.host`) starts routing without a
/// front-door restart. Each connection snapshots the current table, so an in-flight proxy keeps its
/// own.
///
/// `PartialEq` is what the reloader compares: an unchanged table is never swapped, so the common
/// tick costs nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Router {
    routes: Vec<Route>,
    mesh_gateway: Option<SocketAddr>,
}

impl Router {
    /// Build the table from resolved routes plus the optional local mesh gateway (see
    /// [`crate::config::ProxyBinds::mesh_gateway`]).
    #[must_use]
    pub fn new(routes: &[ResolvedRoute], mesh_gateway: Option<SocketAddr>) -> Self {
        Self {
            // Normalise again rather than trust the caller: `Router` is public, and a route built
            // by hand (the gateway, a test) should route identically to one the loader produced.
            routes: routes
                .iter()
                .map(|r| Route {
                    host: host_key(&r.host),
                    path: path_prefix(r.path.as_deref()),
                    upstream: r.upstream,
                })
                .collect(),
            mesh_gateway,
        }
    }

    /// How many routes the table holds — what the status file reports.
    #[must_use]
    pub fn len(&self) -> usize {
        self.routes.len()
    }

    /// Whether the table routes nothing at all.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.routes.is_empty()
    }

    /// Decide where a request goes, from a raw `Host` header value (with an optional `:port`) and
    /// the raw request target off the request line.
    ///
    /// The mesh zone is tested *first* and never falls through to the service routes: a reserved
    /// name has one answer whether or not anything local pretends to serve it.
    ///
    /// Among the routes on a host, the longest matching prefix wins and a route with no prefix is
    /// the fallback — so a dashboard's `/api` backend takes precedence over the frontend that owns
    /// the host, and a host nobody carved up behaves exactly as it did before prefixes existed.
    #[must_use]
    pub fn route(&self, host: &str, target: &str) -> Decision {
        let host = host_key(host);
        if is_mesh_host(&host) {
            return self
                .mesh_gateway
                .map_or(Decision::MeshUnavailable, Decision::Mesh);
        }
        let path = request_path(target);
        let mut best: Option<(&Route, usize)> = None;
        let mut fallback: Option<&Route> = None;
        for route in self.routes.iter().filter(|r| r.host == host) {
            match &route.path {
                None => fallback = fallback.or(Some(route)),
                Some(prefix) => {
                    if prefix_matches(prefix, path)
                        && best.is_none_or(|(_, len)| prefix.len() > len)
                    {
                        best = Some((route, prefix.len()));
                    }
                }
            }
        }
        best.map(|(route, _)| route)
            .or(fallback)
            .map_or(Decision::NoRoute, |route| Decision::Service(route.upstream))
    }
}

/// Whether `prefix` claims `path`, matched on **segment boundaries**: `/api` claims `/api` and
/// `/api/x` but not `/apifoo`, which is a different resource that merely starts with the same
/// letters. Compared against the raw target, deliberately — the head is forwarded verbatim, so
/// matching anything other than the exact bytes the upstream will read would let the two disagree.
fn prefix_matches(prefix: &str, path: &str) -> bool {
    path.strip_prefix(prefix)
        .is_some_and(|rest| rest.is_empty() || rest.starts_with('/'))
}

/// The path portion of a request target: the query and fragment dropped, and the `http://host`
/// stripped off the absolute-form target a client may legitimately send to a proxy. Anything else
/// (`*` on an `OPTIONS`, an authority-form `CONNECT`) is returned as-is and simply matches no
/// prefix, landing on the host's fallback route.
fn request_path(target: &str) -> &str {
    let target = target.trim();
    let path = if target.starts_with('/') {
        target
    } else if let Some((_, authority_and_path)) = target.split_once("://") {
        authority_and_path
            .find('/')
            .map_or("/", |i| &authority_and_path[i..])
    } else {
        target
    };
    path.split(['?', '#']).next().unwrap_or(path)
}

/// Accept loop for one listener; per-connection errors are logged, not returned, until the task is
/// aborted. Each accepted connection snapshots the *current* routing table from `table`, so a
/// hot-swap by the config reloader takes effect on the next connection.
pub async fn serve(listener: TcpListener, table: watch::Receiver<Arc<Router>>) {
    loop {
        match listener.accept().await {
            Ok((stream, peer)) => {
                // Cheap Arc clone of whatever router is current right now.
                let router = table.borrow().clone();
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

/// Accept loop for a TLS listener: complete the handshake, then hand the decrypted stream to the
/// same [`handle`] the plain front door uses.
///
/// A failed handshake is logged at debug and dropped. That is deliberately quiet — a browser that
/// hasn't been given the CA, a health probe speaking plain HTTP to the TLS port, or a port scanner
/// all land here, and none of them is an operational problem worth a warning per connection.
pub async fn serve_tls(
    listener: TcpListener,
    acceptor: TlsAcceptor,
    table: watch::Receiver<Arc<Router>>,
) {
    loop {
        match listener.accept().await {
            Ok((stream, peer)) => {
                let router = table.borrow().clone();
                let acceptor = acceptor.clone();
                tokio::spawn(async move {
                    // Bound the handshake too: an idle client that opens a socket and says nothing
                    // would otherwise hold the task open indefinitely.
                    let accepted = tokio::time::timeout(READ_TIMEOUT, acceptor.accept(stream)).await;
                    let tls = match accepted {
                        Ok(Ok(tls)) => tls,
                        Ok(Err(e)) => {
                            debug!(%peer, error = %e, "TLS handshake failed");
                            return;
                        }
                        Err(_) => {
                            debug!(%peer, "TLS handshake timed out");
                            return;
                        }
                    };
                    if let Err(e) = handle(tls, &router).await {
                        debug!(%peer, error = %e, "proxy connection error");
                    }
                });
            }
            Err(e) => {
                warn!(error = %e, "accept failed");
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        }
    }
}

async fn handle<S: ClientStream>(mut client: S, router: &Router) -> anyhow::Result<()> {
    let head = read_head(&mut client).await?;

    let Some(host) = extract_host(&head) else {
        return respond_error(&mut client, 400, "Bad Request", "Missing Host header.").await;
    };
    // The target only picks the route. A missing/unparsable request line routes as `/`, which is
    // the host's fallback — the same place a pre-prefix config always sent it.
    let target = extract_target(&head);
    let upstream = match router.route(&host, target.as_deref().unwrap_or("/")) {
        Decision::Service(upstream) | Decision::Mesh(upstream) => upstream,
        Decision::MeshUnavailable => {
            info!(%host, "mesh host, but no local mesh gateway is configured");
            return respond_mesh_unavailable(&mut client, &host).await;
        }
        Decision::NoRoute => {
            // Reached the front door but no app answers this host: 404 fallback page (distinct
            // from the 502 below, which means the app exists but its upstream is down).
            info!(%host, "no route");
            return respond_not_found(&mut client).await;
        }
    };

    let mut server = match TcpStream::connect(upstream).await {
        Ok(s) => s,
        Err(e) => {
            warn!(%host, %upstream, error = %e, "upstream connect failed");
            // A dead gateway is the same situation as an unconfigured one from the browser's side:
            // the remote node is unreachable from here. Say that, rather than the generic 502 that
            // would send the reader looking for a local service that was never involved.
            if is_mesh_host(&host) {
                return respond_mesh_unavailable(&mut client, &host).await;
            }
            return respond_error(&mut client, 502, "Bad Gateway", "Upstream is unavailable.")
                .await;
        }
    };
    debug!(%host, %upstream, "proxying");

    // Forward the head bytes we already consumed, then splice the rest both ways.
    server.write_all(&head).await?;

    // `tokio::io::split` rather than TcpStream's inherent borrow-split: the client half is generic
    // now, and this is the one form that works for any stream (TLS included).
    let (mut cread, mut cwrite) = tokio::io::split(client);
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
async fn read_head<S: ClientStream>(stream: &mut S) -> anyhow::Result<Vec<u8>> {
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

/// Pull the request target (the path) out of the request line: `METHOD SP target SP HTTP/1.1`.
fn extract_target(head: &[u8]) -> Option<String> {
    let text = String::from_utf8_lossy(head);
    let line = text.split("\r\n").next()?;
    let mut parts = line.split(' ').filter(|p| !p.is_empty());
    let _method = parts.next()?;
    let target = parts.next()?;
    // A request line with no version is a truncated head, not a target we should route on.
    parts.next()?;
    Some(target.to_string())
}

/// Serve the animated `4XX` fallback page with a `404` — `Host` matched no configured route.
async fn respond_not_found<S: ClientStream>(stream: &mut S) -> anyhow::Result<()> {
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

/// Serve the mesh page with a `502` — the host names a remote node this machine cannot reach,
/// because no local gateway is configured or the one that is would not answer.
async fn respond_mesh_unavailable<S: ClientStream>(
    stream: &mut S,
    host: &str,
) -> anyhow::Result<()> {
    let body = crate::notfound::mesh_unavailable(host, crate::config::mesh_node(host).as_deref());
    let response = format!(
        "HTTP/1.1 502 Bad Gateway\r\n\
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
async fn respond_error<S: ClientStream>(
    stream: &mut S,
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

    fn route(host: &str, path: Option<&str>, upstream: &str) -> ResolvedRoute {
        ResolvedRoute {
            host: host.to_string(),
            path: path.map(str::to_string),
            upstream: upstream.parse().unwrap(),
        }
    }

    fn service(addr: &str) -> Decision {
        Decision::Service(addr.parse().unwrap())
    }

    fn router() -> Router {
        Router::new(
            &[
                route("App.Test", None, "127.0.0.1:8010"),
                route("api.test", None, "127.0.0.1:8009"),
            ],
            None,
        )
    }

    #[test]
    fn resolves_host_case_insensitively_and_ignores_port() {
        let r = router();
        assert_eq!(r.route("app.test", "/"), service("127.0.0.1:8010"));
        assert_eq!(r.route("APP.TEST", "/"), service("127.0.0.1:8010"));
        assert_eq!(r.route("app.test:8080", "/"), service("127.0.0.1:8010"));
        assert_eq!(r.route("api.test", "/"), service("127.0.0.1:8009"));
        assert_eq!(r.route("unknown.test", "/"), Decision::NoRoute);
    }

    /// The shape every config had before `proxy.path`: one service owns the whole host, and the
    /// path must not change a thing about where a request lands.
    #[test]
    fn a_host_only_route_still_answers_every_path() {
        let r = router();
        for target in [
            "/",
            "/anything",
            "/api",
            "/api/deep/er?q=1",
            "*",
            "http://app.test/absolute",
        ] {
            assert_eq!(
                r.route("app.test", target),
                service("127.0.0.1:8010"),
                "target {target}"
            );
        }
    }

    #[test]
    fn the_longest_matching_path_prefix_wins_over_shorter_ones_and_the_host_fallback() {
        // One dashboard host, three claims on it: the page, its API, and a deeper slice of that API.
        let r = Router::new(
            &[
                route("nosh.adi", None, "127.0.0.1:8010"),
                route("nosh.adi", Some("/api"), "127.0.0.1:8011"),
                route("nosh.adi", Some("/api/v2"), "127.0.0.1:8012"),
            ],
            None,
        );
        assert_eq!(r.route("nosh.adi", "/"), service("127.0.0.1:8010"));
        assert_eq!(r.route("nosh.adi", "/assets/app.js"), service("127.0.0.1:8010"));
        assert_eq!(r.route("nosh.adi", "/api"), service("127.0.0.1:8011"));
        assert_eq!(r.route("nosh.adi", "/api/v1/things"), service("127.0.0.1:8011"));
        assert_eq!(r.route("nosh.adi", "/api/v2"), service("127.0.0.1:8012"));
        assert_eq!(r.route("nosh.adi", "/api/v2/things?x=1"), service("127.0.0.1:8012"));
    }

    #[test]
    fn a_path_prefix_matches_only_on_segment_boundaries() {
        let r = Router::new(
            &[
                route("nosh.adi", None, "127.0.0.1:8010"),
                route("nosh.adi", Some("/api"), "127.0.0.1:8011"),
            ],
            None,
        );
        // Same letters, different resource — the frontend keeps it.
        assert_eq!(r.route("nosh.adi", "/apifoo"), service("127.0.0.1:8010"));
        assert_eq!(r.route("nosh.adi", "/api-docs"), service("127.0.0.1:8010"));
        assert_eq!(r.route("nosh.adi", "/apis/x"), service("127.0.0.1:8010"));
        // Exact match and a child segment are both the backend's.
        assert_eq!(r.route("nosh.adi", "/api"), service("127.0.0.1:8011"));
        assert_eq!(r.route("nosh.adi", "/api/"), service("127.0.0.1:8011"));
        assert_eq!(r.route("nosh.adi", "/api?x=1"), service("127.0.0.1:8011"));
    }

    #[test]
    fn a_path_route_with_no_host_fallback_leaves_the_rest_of_the_host_unrouted() {
        // Only `/api` is claimed: everything else on the host has no answer, which is a 404 and
        // not a silent hand-off to the one service that happens to be there.
        let r = Router::new(&[route("nosh.adi", Some("/api"), "127.0.0.1:8011")], None);
        assert_eq!(r.route("nosh.adi", "/api/x"), service("127.0.0.1:8011"));
        assert_eq!(r.route("nosh.adi", "/"), Decision::NoRoute);
    }

    #[test]
    fn a_mesh_host_goes_to_the_gateway_verbatim_whatever_the_path() {
        let gateway: SocketAddr = "127.0.0.1:8099".parse().unwrap();
        let r = Router::new(&[route("app.adi", None, "127.0.0.1:8010")], Some(gateway));
        assert_eq!(
            r.route("nosh.laptop-b.n.adi", "/api/things"),
            Decision::Mesh(gateway)
        );
        assert_eq!(r.route("app.laptop-b.n.adi", "/"), Decision::Mesh(gateway));
        assert_eq!(r.route("NOSH.Tower.N.ADI:443", "/"), Decision::Mesh(gateway));
        // Local names are untouched by the gateway rule.
        assert_eq!(r.route("app.adi", "/"), service("127.0.0.1:8010"));
    }

    #[test]
    fn a_mesh_host_without_a_gateway_gets_its_own_answer_not_a_404() {
        let r = Router::new(&[route("app.adi", None, "127.0.0.1:8010")], None);
        assert_eq!(
            r.route("nosh.laptop-b.n.adi", "/"),
            Decision::MeshUnavailable
        );
        assert_eq!(r.route("nothing.adi", "/"), Decision::NoRoute);
    }

    /// Belt and braces on the reserved namespace: the loader already refuses such a route, but the
    /// router must not honour one even if it is handed one directly.
    #[test]
    fn a_route_that_claims_a_mesh_host_never_wins_over_the_gateway_rule() {
        let gateway: SocketAddr = "127.0.0.1:8099".parse().unwrap();
        let r = Router::new(
            &[route("app.laptop-b.n.adi", None, "127.0.0.1:8010")],
            Some(gateway),
        );
        assert_eq!(r.route("app.laptop-b.n.adi", "/"), Decision::Mesh(gateway));

        let no_gateway = Router::new(&[route("app.laptop-b.n.adi", None, "127.0.0.1:8010")], None);
        assert_eq!(
            no_gateway.route("app.laptop-b.n.adi", "/"),
            Decision::MeshUnavailable,
        );
    }

    #[test]
    fn a_configured_slash_path_routes_exactly_like_no_path() {
        let with_slash = Router::new(&[route("app.adi", Some("/"), "127.0.0.1:8010")], None);
        let without = Router::new(&[route("app.adi", None, "127.0.0.1:8010")], None);
        assert_eq!(with_slash, without, "`path: /` is the host fallback");
    }

    #[test]
    fn request_path_drops_the_query_and_the_absolute_form_prefix() {
        assert_eq!(request_path("/api/x"), "/api/x");
        assert_eq!(request_path("/api/x?y=1&z=2"), "/api/x");
        assert_eq!(request_path("/api/x#frag"), "/api/x");
        assert_eq!(request_path("http://nosh.adi/api/x?y=1"), "/api/x");
        assert_eq!(request_path("https://nosh.adi"), "/");
        assert_eq!(request_path("*"), "*");
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
    fn extracts_the_request_target_from_the_request_line() {
        let head = b"GET /api/things?x=1 HTTP/1.1\r\nHost: nosh.adi\r\n\r\n";
        assert_eq!(extract_target(head).as_deref(), Some("/api/things?x=1"));
        let absolute = b"GET http://nosh.adi/api HTTP/1.1\r\nHost: nosh.adi\r\n\r\n";
        assert_eq!(
            extract_target(absolute).as_deref(),
            Some("http://nosh.adi/api")
        );
        // A truncated request line yields nothing, and the caller routes it as `/`.
        assert_eq!(extract_target(b"GET /api\r\n\r\n"), None);
        assert_eq!(extract_target(b""), None);
    }

    #[test]
    fn detects_end_of_head() {
        assert!(head_complete(b"GET / HTTP/1.1\r\nHost: a.adi\r\n\r\n"));
        assert!(!head_complete(b"GET / HTTP/1.1\r\nHost: a.adi\r\n"));
    }

    /// End to end over an in-memory client: a `*.n.adi` host with no gateway must come back as the
    /// mesh page with a `502`, not the 404 an unknown host would get.
    #[tokio::test]
    async fn a_mesh_host_with_no_gateway_is_answered_with_the_mesh_page() {
        let router = Arc::new(Router::new(&[], None));
        let (mut probe, front) = tokio::io::duplex(16 * 1024);
        let served = tokio::spawn(async move { handle(front, &router).await });

        probe
            .write_all(b"GET /api/x HTTP/1.1\r\nHost: nosh.laptop-b.n.adi\r\n\r\n")
            .await
            .unwrap();
        let mut response = String::new();
        probe.read_to_string(&mut response).await.unwrap();
        served.await.unwrap().expect("handled");

        assert!(
            response.starts_with("HTTP/1.1 502 Bad Gateway\r\n"),
            "got: {}",
            &response[..response.len().min(64)]
        );
        assert!(response.contains("laptop-b"), "the page names the node");
        assert!(response.contains("mesh gateway"), "and what is missing");
    }

    /// The other half of the mesh rule: with a gateway configured the head is handed over **byte
    /// for byte** — no rewritten `Host`, no stripped path — because the far node's absolute
    /// redirects are built from what it reads here (docs/fleet.md §3).
    #[tokio::test]
    async fn a_mesh_host_reaches_the_gateway_with_its_head_untouched() {
        const REQUEST: &[u8] =
            b"GET /api/x?y=1 HTTP/1.1\r\nHost: nosh.laptop-b.n.adi\r\nAccept: */*\r\n\r\n";

        // Port 0: the OS picks a free one, so a test never collides with a live service.
        let gateway = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = gateway.local_addr().unwrap();
        let received = tokio::spawn(async move {
            let (mut sock, _) = gateway.accept().await.unwrap();
            let mut buf = Vec::new();
            sock.read_to_end(&mut buf).await.unwrap();
            buf
        });

        let router = Arc::new(Router::new(
            &[route("app.adi", None, "127.0.0.1:9")],
            Some(addr),
        ));
        let (mut probe, front) = tokio::io::duplex(16 * 1024);
        let served = tokio::spawn(async move { handle(front, &router).await });

        probe.write_all(REQUEST).await.unwrap();
        probe.shutdown().await.unwrap();
        served.await.unwrap().expect("handled");

        assert_eq!(
            received.await.unwrap(),
            REQUEST,
            "the gateway must see the original head, host and path included",
        );
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
