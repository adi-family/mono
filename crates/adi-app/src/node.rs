//! One HTTP request to a paired node's control panel, over this machine's mesh gateway.
//!
//! Two features call *out* to another machine — sending a dashboard to a node ([`crate::transfer`])
//! and reading what a node already runs ([`crate::viewer`]) — and both do it the same way, so the
//! way is written once, here.
//!
//! **It goes through the local mesh gateway, not through DNS.** The request is addressed to
//! `127.0.0.1:<gateway port>` with `Host: app.<node>.n.adi` — byte for byte what the front door
//! would forward if the same URL were typed into a browser here (`adi-mesh`'s
//! `gateway::handle_client`). Resolving the name instead would add the system resolver and the root
//! front door to the path for no gain, and both can be down while the mesh is fine.
//!
//! **The credential is a parameter, never a global.** The node's Basic-auth password is the
//! human-scoped half of `docs/fleet.md` §5 and is enforced *on the node*; this module only carries
//! whatever the caller hands it. Where a caller gets one from is that caller's problem — a transfer
//! asks per transfer, the viewer keeps one per node.
//!
//! **A refusal comes back as a sentence.** [`CallError`] carries a status to answer with and a
//! message already phrased for an operator, because the three failures that actually happen — a
//! wrong password, a node too old for the endpoint, and the *local* gateway's own error page — send
//! a person in three different directions.

use adi_mesh::fleet::FleetRegistry;
use adi_webapp_api::handlers::{self, Response};
use adi_webapp_api::types::ApiError;
use base64::Engine as _;
use tracing::debug;

/// The zone every `<service>.<node>` name lives under (`docs/fleet.md` §1).
pub(crate) const MESH_ZONE: &str = "n.adi";

/// The service label of a node's own control panel — the one thing pairing grants by default.
pub(crate) const APP_SERVICE: &str = "app";

/// How long a small control-plane call may take. A relayed mesh round trip is a third of a second
/// before any payload (`docs/fleet.md` §9), so this is generous for a listing and short enough that
/// an unreachable node does not hold a page open.
pub(crate) const CONTROL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// A failed call to a node, already phrased for the operator and carrying the status to answer
/// with.
#[derive(Debug)]
pub(crate) struct CallError {
    pub(crate) status: u16,
    pub(crate) message: String,
}

/// Refuse a node this machine has never paired with, before any connection is attempted — the
/// gateway would answer its own *not paired* page, as HTML, which is not an error a page can read.
pub(crate) fn require_paired(node: &str) -> Result<(), Response> {
    if !adi_mesh::fleet::valid_name(node) {
        return Err(handlers::error(
            400,
            &format!("{node:?} is not a node name (one lowercase DNS label)"),
        ));
    }
    match FleetRegistry::load() {
        Ok(registry) if registry.get(node).is_some() => Ok(()),
        Ok(_) => Err(handlers::error(
            404,
            &format!("no node named {node:?} is paired with this machine"),
        )),
        Err(e) => Err(handlers::error(
            500,
            &format!("reading the fleet registry: {e}"),
        )),
    }
}

/// This machine's own mesh key, in the string form the registry stores. `None` when the identity
/// cannot be read, which is only ever a broken store.
pub(crate) fn local_key() -> Option<String> {
    adi_mesh::identity::endpoint_id()
        .map(|id| id.to_string())
        .map_err(|e| debug!(error = %e, "node: cannot read this machine's mesh identity"))
        .ok()
}

/// The first label of a hostname — `nosh.adi` → `nosh`. That label is both a grant's scope and the
/// name a dashboard answers to on its own machine.
pub(crate) fn host_label(host: Option<&str>) -> Option<String> {
    let label = host?.trim().trim_end_matches('.').split('.').next()?.trim();
    (!label.is_empty()).then(|| label.to_ascii_lowercase())
}

/// Where to open one of a node's services from *this* machine: its label under the node's mesh
/// zone. `None` when there is no routable name over there, in which case there is nothing on this
/// side to link to either.
pub(crate) fn mesh_url(node: &str, host: Option<&str>) -> Option<String> {
    host_label(host).map(|label| format!("http://{label}.{node}.{MESH_ZONE}/"))
}

/// The `Authorization` header value for a node's Basic-auth gate, defaulting the user to the one
/// pairing mints.
pub(crate) fn basic_auth(username: Option<&str>, password: &str) -> String {
    let user = username
        .map(str::trim)
        .filter(|u| !u.is_empty())
        .unwrap_or(adi_mesh::join::PAIR_USER);
    let encoded =
        base64::engine::general_purpose::STANDARD.encode(format!("{user}:{password}").as_bytes());
    format!("Basic {encoded}")
}

/// `GET <path>` on a node's control panel.
pub(crate) async fn get(
    node: &str,
    path: &str,
    auth: &str,
    timeout: std::time::Duration,
) -> Result<String, CallError> {
    call(node, reqwest::Method::GET, path, auth, None, timeout).await
}

/// `POST <path>` on a node's control panel, with a JSON body.
pub(crate) async fn post(
    node: &str,
    path: &str,
    auth: &str,
    payload: Vec<u8>,
    timeout: std::time::Duration,
) -> Result<String, CallError> {
    call(
        node,
        reqwest::Method::POST,
        path,
        auth,
        Some(payload),
        timeout,
    )
    .await
}

/// One request to `app.<node>.n.adi`, at whatever address the local gateway is listening on.
async fn call(
    node: &str,
    method: reqwest::Method,
    path: &str,
    auth: &str,
    payload: Option<Vec<u8>>,
    timeout: std::time::Duration,
) -> Result<String, CallError> {
    call_at(
        adi_mesh::gateway::configured_addr(),
        node,
        method,
        path,
        auth,
        payload,
        timeout,
    )
    .await
}

/// The gateway address is a parameter and not read here, so the one assumption this whole path
/// rests on — that *our* `Host` reaches the wire, rather than the URL's authority — is pinned by a
/// test against a real socket instead of by reading someone else's client.
///
/// Made the way the front door would make it: straight at the local mesh gateway, with the fleet
/// hostname in the `Host` header. The gateway is what turns that name into a peer key and a
/// bi-stream; nothing here knows about iroh.
async fn call_at(
    gateway: std::net::SocketAddr,
    node: &str,
    method: reqwest::Method,
    path: &str,
    auth: &str,
    payload: Option<Vec<u8>>,
    timeout: std::time::Duration,
) -> Result<String, CallError> {
    let host = format!("{APP_SERVICE}.{node}.{MESH_ZONE}");
    crate::ensure_tls_provider();
    let client = reqwest::Client::builder()
        .timeout(timeout)
        .build()
        .map_err(|e| CallError {
            status: 500,
            message: format!("building the HTTP client: {e}"),
        })?;

    let mut request = client
        .request(method, format!("http://{gateway}{path}"))
        // The gateway routes on this and nothing else — the URL's authority is only how the
        // connection finds the loopback listener.
        .header(reqwest::header::HOST, &host)
        .header(reqwest::header::AUTHORIZATION, auth);
    if let Some(payload) = payload {
        request = request
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .body(payload);
    }

    let response = request
        .send()
        .await
        .map_err(|e| unreachable(node, gateway, &e))?;
    let status = response.status().as_u16();
    let html = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.starts_with("text/html"));
    let body = response.text().await.unwrap_or_default();

    if status == 200 {
        return Ok(body);
    }
    debug!(%node, %path, status, "node: the node refused");
    Err(CallError {
        status: if status == 401 { 401 } else { 502 },
        message: refusal(node, path, status, html, &body),
    })
}

/// Why a request never got an answer. Almost always one thing — the mesh daemon is not running on
/// this machine, so nothing is listening on the gateway port — and that is worth saying outright
/// rather than surfacing a connection-refused.
fn unreachable(node: &str, gateway: std::net::SocketAddr, e: &reqwest::Error) -> CallError {
    if e.is_connect() {
        return CallError {
            status: 503,
            message: format!(
                "the mesh gateway is not listening on {gateway}, so nothing here can reach \
                 {node} — start the mesh from the Mesh page and try again"
            ),
        };
    }
    if e.is_timeout() {
        return CallError {
            status: 504,
            message: format!("{node} did not answer in time"),
        };
    }
    CallError {
        status: 502,
        message: format!("reaching {node}: {e}"),
    }
}

/// Turn a node's refusal into one sentence an operator can act on.
///
/// The three cases are genuinely different problems: a wrong password is typed again, a 404 means
/// the node is running an adi that predates this endpoint, and a `text/html` body is the local
/// gateway's own error page — which means the request never left this machine, so the node's
/// password had nothing to do with it.
fn refusal(node: &str, path: &str, status: u16, html: bool, body: &str) -> String {
    if status == 401 {
        return format!("{node} refused the password");
    }
    if html {
        return format!(
            "the mesh gateway could not reach {node} (it answered {status}) — check the node is \
             paired, that its mesh daemon is up, and that it has granted this machine `http:app`"
        );
    }
    if status == 404 {
        return format!(
            "{node} has no {path} endpoint — its adi is older than this one, so update it first"
        );
    }
    let detail = serde_json::from_str::<ApiError>(body)
        .map_or_else(|_| body.chars().take(300).collect(), |e| e.error);
    if detail.trim().is_empty() {
        format!("{node} answered {status}")
    } else {
        format!("{node} answered {status}: {detail}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_dashboard_host_becomes_a_label_and_a_mesh_url() {
        assert_eq!(host_label(Some("nosh.adi")).as_deref(), Some("nosh"));
        assert_eq!(host_label(Some("NOSH.adi.")).as_deref(), Some("nosh"));
        assert_eq!(host_label(None), None);
        assert_eq!(host_label(Some("  ")), None);

        // The node cannot know what we call it, so the viewer builds the name it will type.
        assert_eq!(
            mesh_url("laptop-b", Some("nosh.adi")).as_deref(),
            Some("http://nosh.laptop-b.n.adi/")
        );
        // No routable name over there means no link over here — never `http://.laptop-b.n.adi/`.
        assert_eq!(mesh_url("laptop-b", None), None);
    }

    #[test]
    fn the_credential_defaults_to_the_user_pairing_mints() {
        // `adi:hunter2`
        assert_eq!(basic_auth(None, "hunter2"), "Basic YWRpOmh1bnRlcjI=");
        assert_eq!(basic_auth(Some("  "), "hunter2"), basic_auth(None, "hunter2"));
        assert_ne!(
            basic_auth(Some("root"), "hunter2"),
            basic_auth(None, "hunter2")
        );
    }

    #[test]
    fn each_refusal_names_the_thing_to_fix() {
        let wrong = refusal("laptop-b", "/api/dashboards/import", 401, false, "");
        assert!(wrong.contains("password"), "{wrong}");

        // An HTML body is the *local* gateway's error page: the request never left this machine,
        // so telling the operator to check their password would send them the wrong way.
        let page = refusal("laptop-b", "/api/dashboards/import", 502, true, "<html>…");
        assert!(page.contains("paired"), "{page}");
        assert!(!page.contains("password"), "{page}");

        let old = refusal("laptop-b", "/api/dashboards/import", 404, false, "");
        assert!(old.contains("update it"), "{old}");

        // A JSON error from the node itself is quoted, not swallowed.
        let json = refusal(
            "laptop-b",
            "/api/dashboards/import",
            413,
            false,
            r#"{"ok":false,"error":"the bundle is too large"}"#,
        );
        assert!(json.contains("the bundle is too large"), "{json}");
    }

    /// Answer one request with `status` and `body`, and hand back the request head as it arrived
    /// on the wire. A real socket, because what is under test is what reqwest actually writes.
    async fn one_request(
        listener: tokio::net::TcpListener,
        status: &'static str,
        content_type: &'static str,
        body: &'static str,
    ) -> String {
        use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

        let (mut sock, _) = listener.accept().await.expect("accept");
        let mut buf = Vec::new();
        let mut chunk = [0u8; 1024];
        // Until the blank line: the head may not arrive in one segment, and asserting on half of
        // it would make this test pass for the wrong reason.
        while !buf.windows(4).any(|w| w == b"\r\n\r\n") {
            let n = sock.read(&mut chunk).await.expect("read");
            if n == 0 {
                break;
            }
            buf.extend_from_slice(&chunk[..n]);
        }
        sock.write_all(
            format!(
                "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\n\r\n{body}",
                body.len(),
            )
            .as_bytes(),
        )
        .await
        .expect("write");
        sock.flush().await.expect("flush");
        String::from_utf8_lossy(&buf).into_owned()
    }

    /// The single load-bearing assumption of this module: the gateway routes on `Host`, and the
    /// URL only says which loopback socket to open. If a client library ever decided its own
    /// authority wins, every call would be addressed to the gateway itself — which parses that as
    /// "not a `*.n.adi` name" and answers 400. Pinned against a real socket rather than by reading
    /// someone else's source.
    #[tokio::test]
    async fn a_request_is_addressed_to_the_node_and_carries_the_credential() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let gateway = listener.local_addr().expect("addr");
        let server = tokio::spawn(one_request(
            listener,
            "200 OK",
            "application/json",
            "{\"ok\":1}",
        ));

        let body = call_at(
            gateway,
            "laptop-b",
            reqwest::Method::POST,
            "/api/dashboards/import",
            &basic_auth(None, "hunter2"),
            Some(b"{}".to_vec()),
            CONTROL_TIMEOUT,
        )
        .await
        .expect("the node answered");

        let head = server.await.expect("server").to_lowercase();
        assert!(
            head.starts_with("post /api/dashboards/import http/1.1"),
            "{head}"
        );
        assert_eq!(
            head.matches("\r\nhost:").count(),
            1,
            "exactly one Host, or the gateway reads whichever it happens to find first: {head}"
        );
        assert!(head.contains("\r\nhost: app.laptop-b.n.adi\r\n"), "{head}");
        assert!(
            !head.contains(&gateway.to_string()),
            "the loopback address must not reach the wire as a name: {head}"
        );
        // `adi:hunter2`, and a content length so the node reads the whole bundle.
        assert!(head.contains("authorization: basic ywrpomh1bnrlcji="), "{head}");
        assert!(head.contains("content-length: 2"), "{head}");
        assert_eq!(body, "{\"ok\":1}");
    }

    /// A refusal is turned into a message here, not left as a status for the page to guess at —
    /// and the *local* gateway's HTML error page must never be reported as the node's answer.
    #[tokio::test]
    async fn the_local_gateways_own_error_page_is_not_read_as_the_node_refusing() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let gateway = listener.local_addr().expect("addr");
        let server = tokio::spawn(one_request(
            listener,
            "502 Bad Gateway",
            "text/html; charset=utf-8",
            "<html>not paired</html>",
        ));

        let refused = call_at(
            gateway,
            "laptop-b",
            reqwest::Method::GET,
            "/api/fleet",
            &basic_auth(None, "hunter2"),
            None,
            CONTROL_TIMEOUT,
        )
        .await
        .expect_err("a 502 is not an answer");
        let _ = server.await;

        assert_eq!(refused.status, 502);
        assert!(refused.message.contains("paired"), "{}", refused.message);
        assert!(
            !refused.message.contains("<html>"),
            "an error page is not an error message: {}",
            refused.message
        );
    }

    #[test]
    fn an_unpaired_node_is_refused_before_anything_is_dialled() {
        // Not a DNS label: caught by name, without touching the registry at all.
        for bad in ["Laptop-B", "laptop b", "", "a.b"] {
            let refused = require_paired(bad).expect_err("must be refused");
            assert_eq!(refused.status, 400, "{bad}: {}", refused.body);
        }
    }
}
