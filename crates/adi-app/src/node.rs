//! HTTP calls to paired nodes through the local mesh gateway.
//!
//! Connect to the gateway directly; `Host: app.<node>.n.adi` selects the peer without DNS.
//! Credentials are supplied by the caller and enforced by the node's Basic-auth gate.
//! Errors distinguish node refusals from the local gateway's error pages.

use adi_mesh::fleet::FleetRegistry;
use adi_webapp_api::handlers::{self, Response};
use adi_webapp_api::types::{ApiError, Reach};
use base64::Engine as _;
use tracing::debug;

pub(crate) const MESH_ZONE: &str = "n.adi";

/// Pairing grants access to this service by default.
pub(crate) const APP_SERVICE: &str = "app";

pub(crate) const LOCAL_ZONE: &str = "adi";

pub(crate) const CONTROL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

#[derive(Debug)]
pub(crate) struct CallError {
    pub(crate) status: u16,
    pub(crate) message: String,
}

/// Reject invalid or unpaired names before dialing, so callers receive JSON errors.
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

pub(crate) fn local_key() -> Option<String> {
    adi_mesh::identity::endpoint_id()
        .map(|id| id.to_string())
        .map_err(|e| debug!(error = %e, "node: cannot read this machine's mesh identity"))
        .ok()
}

/// Strip the local zone, preserving service labels (`app.nosh.adi` → `app.nosh`).
/// Hosts outside `.adi` have no mesh service name.
pub(crate) fn service_name(host: Option<&str>) -> Option<String> {
    let host = host?.trim().trim_end_matches('.').to_ascii_lowercase();
    let name = host.strip_suffix(&format!(".{LOCAL_ZONE}"))?;
    adi_mesh::protocol::is_service_name(name).then(|| name.to_string())
}

pub(crate) fn mesh_url(node: &str, host: Option<&str>) -> Option<String> {
    service_name(host).map(|name| format!("http://{name}.{node}.{MESH_ZONE}/"))
}

/// Build Basic auth, defaulting an absent or blank username to the pairing user.
pub(crate) fn basic_auth(username: Option<&str>, password: &str) -> String {
    let user = username
        .map(str::trim)
        .filter(|u| !u.is_empty())
        .unwrap_or(adi_mesh::join::PAIR_USER);
    let encoded =
        base64::engine::general_purpose::STANDARD.encode(format!("{user}:{password}").as_bytes());
    format!("Basic {encoded}")
}

pub(crate) async fn get(
    node: &str,
    path: &str,
    auth: &str,
    timeout: std::time::Duration,
) -> Result<String, CallError> {
    call(node, reqwest::Method::GET, path, auth, None, timeout).await
}

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
        Some(Payload {
            bytes: payload,
            content_type: "application/json",
            filename: None,
        }),
        timeout,
    )
    .await
}

/// POST raw attachment bytes with their content type and optional `X-Adi-Filename`.
pub(crate) async fn post_bytes(
    node: &str,
    path: &str,
    auth: &str,
    content_type: &str,
    filename: Option<&str>,
    payload: Vec<u8>,
    timeout: std::time::Duration,
) -> Result<String, CallError> {
    call(
        node,
        reqwest::Method::POST,
        path,
        auth,
        Some(Payload {
            bytes: payload,
            content_type,
            filename,
        }),
        timeout,
    )
    .await
}

struct Payload<'a> {
    bytes: Vec<u8>,
    content_type: &'a str,
    filename: Option<&'a str>,
}

async fn call(
    node: &str,
    method: reqwest::Method,
    path: &str,
    auth: &str,
    payload: Option<Payload<'_>>,
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

async fn call_at(
    gateway: std::net::SocketAddr,
    node: &str,
    method: reqwest::Method,
    path: &str,
    auth: &str,
    payload: Option<Payload<'_>>,
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
        // The URL selects the loopback socket; only Host selects the peer and service.
        .header(reqwest::header::HOST, &host)
        .header(reqwest::header::AUTHORIZATION, auth);
    if let Some(payload) = payload {
        request = request.header(reqwest::header::CONTENT_TYPE, payload.content_type);
        if let Some(filename) = payload.filename {
            request = request.header("x-adi-filename", filename);
        }
        request = request.body(payload.bytes);
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

/// HTML errors come from the local gateway; JSON errors come from the node.
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

/// Probe the node; even a `401` proves it answered.
pub(crate) async fn reach(node: &str, auth: &str, timeout: std::time::Duration) -> Reach {
    reach_at(adi_mesh::gateway::configured_addr(), node, auth, timeout).await
}

async fn reach_at(
    gateway: std::net::SocketAddr,
    node: &str,
    auth: &str,
    timeout: std::time::Duration,
) -> Reach {
    // The client builder panics if no TLS provider is installed.
    crate::ensure_tls_provider();
    let Ok(client) = reqwest::Client::builder().timeout(timeout).build() else {
        return Reach::Unreachable;
    };
    let answered = client
        .get(format!("http://{gateway}/api/health"))
        .header(
            reqwest::header::HOST,
            format!("{APP_SERVICE}.{node}.{MESH_ZONE}"),
        )
        .header(reqwest::header::AUTHORIZATION, auth)
        .send()
        .await;
    let response = match answered {
        Ok(response) => response,
        Err(e) if e.is_connect() => return Reach::MeshOff,
        Err(_) => return Reach::Unreachable,
    };
    let status = response.status().as_u16();
    let html = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.starts_with("text/html"));
    let body = if html {
        response.text().await.unwrap_or_default()
    } else {
        String::new()
    };
    classify(status, html, &body)
}

/// Gateway failures are `502 text/html`; its "refused the request" heading means the
/// peer answered. Keep this in sync with `adi-mesh`'s `gateway::reason_heading`.
fn classify(status: u16, html: bool, body: &str) -> Reach {
    if !(html && status == 502) {
        return Reach::Reachable;
    }
    if body.contains("refused the request") {
        Reach::Refused
    } else {
        Reach::Unreachable
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_gateways_own_page_says_a_node_was_not_there() {
        assert_eq!(classify(200, false, ""), Reach::Reachable);
        assert_eq!(classify(401, false, ""), Reach::Reachable, "a 401 is the node answering");
        assert_eq!(
            classify(502, true, "<h1>That machine refused the request</h1>"),
            Reach::Refused
        );
        assert_eq!(
            classify(502, true, "<h1>That machine is not reachable from here</h1>"),
            Reach::Unreachable
        );
        assert_eq!(
            classify(502, true, "<h1>That machine has not paired with this one</h1>"),
            Reach::Unreachable
        );
    }

    #[tokio::test]
    async fn nothing_listening_on_the_gateway_is_the_mesh_being_off() {
        // Drop the listener after obtaining its address to probe a closed port.
        let addr = std::net::TcpListener::bind("127.0.0.1:0")
            .and_then(|l| l.local_addr())
            .expect("bind");
        assert_eq!(
            reach_at(addr, "laptop-b", "", std::time::Duration::from_secs(2)).await,
            Reach::MeshOff
        );
    }

    #[test]
    fn a_dashboard_host_becomes_a_service_name_and_a_mesh_url() {
        assert_eq!(service_name(Some("nosh.adi")).as_deref(), Some("nosh"));
        assert_eq!(service_name(Some("NOSH.adi.")).as_deref(), Some("nosh"));
        assert_eq!(
            service_name(Some("app.nosh.adi")).as_deref(),
            Some("app.nosh")
        );
        assert_eq!(service_name(None), None);
        assert_eq!(service_name(Some("  ")), None);
        assert_eq!(service_name(Some("nosh.guide")), None);
        assert_eq!(service_name(Some("adi")), None);

        assert_eq!(
            mesh_url("laptop-b", Some("nosh.adi")).as_deref(),
            Some("http://nosh.laptop-b.n.adi/")
        );
        assert_eq!(
            mesh_url("laptop-b", Some("app.nosh.adi")).as_deref(),
            Some("http://app.nosh.laptop-b.n.adi/")
        );
        assert_eq!(mesh_url("laptop-b", None), None);
    }

    #[test]
    fn the_credential_defaults_to_the_user_pairing_mints() {
        assert_eq!(basic_auth(None, "hunter2"), "Basic YWRpOmh1bnRlcjI=");
        assert_eq!(
            basic_auth(Some("  "), "hunter2"),
            basic_auth(None, "hunter2")
        );
        assert_ne!(
            basic_auth(Some("root"), "hunter2"),
            basic_auth(None, "hunter2")
        );
    }

    #[test]
    fn each_refusal_names_the_thing_to_fix() {
        let wrong = refusal("laptop-b", "/api/dashboards/import", 401, false, "");
        assert!(wrong.contains("password"), "{wrong}");

        let page = refusal("laptop-b", "/api/dashboards/import", 502, true, "<html>…");
        assert!(page.contains("paired"), "{page}");
        assert!(!page.contains("password"), "{page}");

        let old = refusal("laptop-b", "/api/dashboards/import", 404, false, "");
        assert!(old.contains("update it"), "{old}");

        let json = refusal(
            "laptop-b",
            "/api/dashboards/import",
            413,
            false,
            r#"{"ok":false,"error":"the bundle is too large"}"#,
        );
        assert!(json.contains("the bundle is too large"), "{json}");
    }

    /// Capture the request head from a real socket to check what reqwest sends.
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
        // The request head may arrive across multiple reads.
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
            Some(Payload {
                bytes: b"{}".to_vec(),
                content_type: "application/json",
                filename: None,
            }),
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
        assert!(
            head.contains("authorization: basic ywrpomh1bnrlcji="),
            "{head}"
        );
        assert!(head.contains("content-length: 2"), "{head}");
        assert_eq!(body, "{\"ok\":1}");
    }

    #[tokio::test]
    async fn an_attachments_bytes_carry_their_own_type_and_filename() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let gateway = listener.local_addr().expect("addr");
        let png = vec![0x89, b'P', b'N', b'G', 0, 1, 2, 3];
        let server = tokio::spawn(one_request(
            listener,
            "200 OK",
            "application/json",
            "{\"id\":\"att-1\"}",
        ));

        let body = call_at(
            gateway,
            "laptop-b",
            reqwest::Method::POST,
            "/api/agents/attachment",
            &basic_auth(None, "hunter2"),
            Some(Payload {
                bytes: png.clone(),
                content_type: "image/png",
                filename: Some("screenshot.png"),
            }),
            CONTROL_TIMEOUT,
        )
        .await
        .expect("the node answered");

        let head = server.await.expect("server").to_lowercase();
        assert!(head.contains("content-type: image/png"), "{head}");
        assert!(head.contains("x-adi-filename: screenshot.png"), "{head}");
        assert!(
            !head.contains("content-type: application/json"),
            "the raw bytes must not be wrapped as JSON: {head}"
        );
        assert!(
            head.contains(&format!("content-length: {}", png.len())),
            "{head}"
        );
        assert_eq!(body, "{\"id\":\"att-1\"}");
    }

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
        for bad in ["Laptop-B", "laptop b", "", "a.b"] {
            let refused = require_paired(bad).expect_err("must be refused");
            assert_eq!(refused.status, 400, "{bad}: {}", refused.body);
        }
    }
}
