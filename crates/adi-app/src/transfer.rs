//! `POST /api/dashboards/transfer` — take a dashboard that runs here and make it run on a node.
//!
//! A dashboard is a directory (`docs/fleet.md` §4), and every machine in the fleet already knows
//! how to turn one into a running pair of bun servers. So "run this in the cloud" needs no new
//! deployment machinery at all: pack the directory, hand it to the node's own control panel, and
//! let the node's supervisor do there exactly what ours does here.
//!
//! **This is the one endpoint that calls *out* to another machine**, which is why it lives beside
//! the app rather than in `adi-webapp-api` with the rest of the dashboards logic. Three things
//! about that call are worth stating.
//!
//! **It goes through the local mesh gateway, not through DNS.** The request is addressed to
//! `127.0.0.1:<gateway port>` with `Host: app.<node>.n.adi` — byte for byte what the front door
//! would forward if the same URL were typed into a browser here (`adi-mesh`'s
//! `gateway::handle_client`). Resolving the name instead would add the system resolver and the
//! root front door to the path for no gain, and both can be down while the mesh is fine.
//!
//! **The password is asked for and never kept.** This machine holds a *verifier* for each node's
//! credential, not the credential (`docs/fleet.md` §8), and a deploy button is not a reason to
//! start keeping one. It rides in the request body, becomes an `Authorization` header, and is
//! dropped with the request.
//!
//! **The local copy is stood down last, and only on a `200`.** [`handlers::complete_move`] runs
//! after the node has confirmed it holds the files — never in parallel, never optimistically. A
//! move whose upload failed leaves this machine exactly as it was.

use adi_mesh::fleet::FleetRegistry;
use adi_ports_manager::Ports;
use adi_projects::Projects;
use adi_webapp_api::handlers::{self, Response};
use adi_webapp_api::types::{
    ApiError, Dashboard, DashboardTransferred, DashboardsState, FleetGrantRef, FleetState,
    TransferDashboard, TransferMode,
};
use base64::Engine as _;
use tracing::{debug, warn};

use crate::scan;

/// How long the upload may take. Generous next to the other two: it carries the whole dashboard,
/// and a relayed mesh round trip is a third of a second before any payload (`docs/fleet.md` §9).
const UPLOAD_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(120);

/// How long the two small bookkeeping calls may take.
const CONTROL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// The zone every `<service>.<node>` name lives under (`docs/fleet.md` §1).
const MESH_ZONE: &str = "n.adi";

/// The service label of a node's own control panel — the one thing pairing grants by default.
const APP_SERVICE: &str = "app";

/// Send a dashboard to a paired node, and in [`TransferMode::Move`] stand the local copy down.
pub(crate) async fn transfer_dashboard(
    projects: &Projects,
    ports: &Ports,
    body: &[u8],
) -> Response {
    let req: TransferDashboard = match serde_json::from_slice(body) {
        Ok(req) => req,
        Err(e) => return handlers::error(400, &format!("invalid request body: {e}")),
    };
    let (id, node) = (req.id.trim().to_string(), req.node.trim().to_string());
    if id.is_empty() || node.is_empty() {
        return handlers::error(400, "a transfer needs both a dashboard and a node");
    }
    if req.password.is_empty() {
        return handlers::error(
            400,
            &format!(
                "{node} asks for its password before it will accept anything — it was printed \
                 once, on the node, when it joined this fleet"
            ),
        );
    }
    if let Err(response) = require_paired(&node) {
        return response;
    }

    // Pack before dialling: an unknown id or an oversized directory is a local answer, and there
    // is no reason to have opened a connection to learn it.
    let bundle = match handlers::export_bundle(projects.config(), &id) {
        Ok(bundle) => bundle,
        Err(response) => return response,
    };
    let payload = match serde_json::to_vec(&bundle) {
        Ok(payload) => payload,
        Err(e) => return handlers::error(500, &format!("packing the dashboard: {e}")),
    };

    let auth = basic_auth(req.username.as_deref(), &req.password);
    let sent = post(&node, "/api/dashboards/import", &auth, payload, UPLOAD_TIMEOUT).await;
    let remote: Dashboard = match sent {
        Ok(body) => match serde_json::from_str(&body) {
            Ok(remote) => remote,
            Err(e) => {
                return handlers::error(
                    502,
                    &format!("{node} accepted the dashboard but answered something unreadable: {e}"),
                );
            }
        },
        Err(e) => return handlers::error(e.status, &e.message),
    };

    // Reaching it from here needs a grant on the node, and pairing hands out only `http:app`
    // (`docs/fleet.md` §8). Best-effort by design: the dashboard is already running over there,
    // so a grant we could not add is a link that 502s, not a transfer that failed.
    let granted = grant_self(&node, &auth, remote.host.as_deref()).await;

    let local = match req.mode {
        TransferMode::Copy => handlers::dashboards(projects.config(), ports, &scan::listening_ports()),
        TransferMode::Move => handlers::complete_move(
            projects.config(),
            ports,
            &scan::listening_ports(),
            &id,
            &node,
            req.delete_local,
        ),
    };
    if local.status != 200 {
        // The node has it; only the local half went wrong. Say so with the node's status intact,
        // rather than reporting a failure for a transfer that did happen.
        return local;
    }
    let Ok(dashboards) = serde_json::from_str::<DashboardsState>(&local.body) else {
        return handlers::error(500, "could not read back the local dashboards listing");
    };

    handlers::ok_json(&DashboardTransferred {
        url: mesh_url(&node, remote.host.as_deref()),
        node,
        dashboard: remote,
        granted,
        dashboards,
    })
}

/// Refuse a node this machine has never paired with, before any connection is attempted — the
/// gateway would answer its own *not paired* page, as HTML, which is not an error a page can read.
fn require_paired(node: &str) -> Result<(), Response> {
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

/// Ask the node to let this machine reach the dashboard it just took, so the link on the page
/// works on the first click rather than after a manual grant.
///
/// The node files us under a petname of *its* choosing, which we have no way of knowing — so the
/// key is what identifies us: read its fleet, find the record whose key is ours, and grant against
/// that name. Every step is allowed to fail quietly; the caller reports what happened either way.
async fn grant_self(node: &str, auth: &str, host: Option<&str>) -> bool {
    let Some(label) = host_label(host) else {
        return false;
    };
    let Some(us) = local_key() else {
        return false;
    };
    let fleet = match get(node, "/api/fleet", auth, CONTROL_TIMEOUT).await {
        Ok(body) => body,
        Err(e) => {
            debug!(%node, error = %e.message, "transfer: could not read the node's fleet");
            return false;
        }
    };
    let Ok(fleet) = serde_json::from_str::<FleetState>(&fleet) else {
        return false;
    };
    let Some(petname) = fleet.nodes.iter().find(|n| n.key == us).map(|n| n.petname.clone()) else {
        warn!(%node, "transfer: the node does not have this machine's key on file");
        return false;
    };
    let grant = FleetGrantRef {
        petname,
        grant: format!("http:{label}"),
    };
    let Ok(payload) = serde_json::to_vec(&grant) else {
        return false;
    };
    match post(node, "/api/fleet/grants/add", auth, payload, CONTROL_TIMEOUT).await {
        Ok(_) => true,
        Err(e) => {
            debug!(%node, error = %e.message, "transfer: the node refused the grant");
            false
        }
    }
}

/// This machine's own mesh key, in the string form the registry stores. `None` when the identity
/// cannot be read, which is only ever a broken store.
fn local_key() -> Option<String> {
    adi_mesh::identity::endpoint_id()
        .map(|id| id.to_string())
        .map_err(|e| warn!(error = %e, "transfer: cannot read this machine's mesh identity"))
        .ok()
}

/// The first label of a hostname — `nosh.adi` → `nosh`. That label is both the grant's scope and
/// the name the dashboard answers to on the node.
fn host_label(host: Option<&str>) -> Option<String> {
    let label = host?.trim().trim_end_matches('.').split('.').next()?.trim();
    (!label.is_empty()).then(|| label.to_ascii_lowercase())
}

/// Where to open the transferred dashboard from *this* machine: its label under the node's mesh
/// zone. `None` when the node gave the copy no routable name, in which case there is nothing on
/// this side to link to either.
fn mesh_url(node: &str, host: Option<&str>) -> Option<String> {
    host_label(host).map(|label| format!("http://{label}.{node}.{MESH_ZONE}/"))
}

/// The `Authorization` header value for a node's Basic-auth gate, defaulting the user to the one
/// pairing mints.
fn basic_auth(username: Option<&str>, password: &str) -> String {
    let user = username
        .map(str::trim)
        .filter(|u| !u.is_empty())
        .unwrap_or(adi_mesh::join::PAIR_USER);
    let encoded =
        base64::engine::general_purpose::STANDARD.encode(format!("{user}:{password}").as_bytes());
    format!("Basic {encoded}")
}

/// A failed call to a node, already phrased for the operator and carrying the status to answer
/// with.
#[derive(Debug)]
struct CallError {
    status: u16,
    message: String,
}

/// `GET <path>` on a node's control panel.
async fn get(
    node: &str,
    path: &str,
    auth: &str,
    timeout: std::time::Duration,
) -> Result<String, CallError> {
    call(node, reqwest::Method::GET, path, auth, None, timeout).await
}

/// `POST <path>` on a node's control panel, with a JSON body.
async fn post(
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

    let response = request.send().await.map_err(|e| unreachable(node, gateway, &e))?;
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
    debug!(%node, %path, status, "transfer: the node refused");
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
            "{node} has no {path} endpoint — its adi is older than this one, so update it before \
             transferring dashboards to it"
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
        assert_ne!(basic_auth(Some("root"), "hunter2"), basic_auth(None, "hunter2"));
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
        let json = refusal("laptop-b", "/api/dashboards/import", 413, false,
            r#"{"ok":false,"error":"the bundle is too large"}"#);
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
    /// authority wins, every transfer would be addressed to the gateway itself — which parses that
    /// as "not a `*.n.adi` name" and answers 400. Pinned against a real socket rather than by
    /// reading someone else's source.
    #[tokio::test]
    async fn a_request_is_addressed_to_the_node_and_carries_the_credential() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let gateway = listener.local_addr().expect("addr");
        let server = tokio::spawn(one_request(listener, "200 OK", "application/json", "{\"ok\":1}"));

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
        assert!(head.starts_with("post /api/dashboards/import http/1.1"), "{head}");
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
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind");
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
