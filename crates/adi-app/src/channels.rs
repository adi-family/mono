//! Forward channel API requests to the local service supervised by Hive.
//!
//! Hive owns the service's port and routes its stable internal domain. Requests connect to
//! Hive's local listener with that Host, including installs without system DNS integration.
//! The control panel never starts router sockets or resolves the service's allocated port.

use std::sync::OnceLock;
use std::time::Duration;

use adi_webapp_api::handlers::{self, Response};

use crate::http::Request;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(3);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(60);

pub(crate) fn matches(path: &str) -> bool {
    path == "/api/channels" || path.starts_with("/api/channels/")
}

/// Incoming browser requests have already passed the app's origin check. Only their
/// content type, method, path and body cross the local service boundary.
pub(crate) async fn forward(req: &Request) -> Response {
    forward_to(
        req,
        &adi_channels::service::transport_url(),
        &adi_channels::service::host(),
    )
    .await
}

async fn forward_to(req: &Request, transport_url: &str, service_host: &str) -> Response {
    static CLIENT: OnceLock<Result<reqwest::Client, String>> = OnceLock::new();
    let client = CLIENT.get_or_init(|| {
        crate::ensure_tls_provider();
        reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(CONNECT_TIMEOUT)
            .timeout(REQUEST_TIMEOUT)
            .build()
            .map_err(|e| e.to_string())
    });
    let client = match client {
        Ok(client) => client,
        Err(e) => return handlers::error(500, &format!("preparing channel service request: {e}")),
    };
    let method = match reqwest::Method::from_bytes(req.method.as_bytes()) {
        Ok(method) => method,
        Err(_) => return handlers::error(400, "invalid HTTP method"),
    };
    let url = format!("{}{}", transport_url.trim_end_matches('/'), req.path);
    let mut request = client
        .request(method, url)
        .header(reqwest::header::HOST, service_host)
        .body(req.body.clone());
    if let Some(content_type) = req.header("content-type") {
        request = request.header(reqwest::header::CONTENT_TYPE, content_type);
    }
    let upstream = match request.send().await {
        Ok(response) => response,
        Err(e) => {
            return handlers::error(
                503,
                &format!("the Hive channel service is unavailable at http://{service_host}: {e}"),
            );
        }
    };
    let status = upstream.status().as_u16();
    match upstream.text().await {
        Ok(body) => Response { status, body },
        Err(e) => handlers::error(
            502,
            &format!("reading the Hive channel service response: {e}"),
        ),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use tokio::net::TcpListener;

    use super::*;

    #[tokio::test]
    async fn forwards_mutation_and_query_and_preserves_upstream_errors() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let transport_url = format!("http://{}", listener.local_addr().unwrap());
        let service_host = "channels.adi-dev";
        let expected_body = r#"{"ok":false,"error":"unknown connection"}"#;
        let upstream = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let req = crate::http::read_request(&mut stream)
                .await
                .unwrap()
                .unwrap();
            assert_eq!(req.method, "POST");
            assert_eq!(req.header("host"), Some(service_host));
            assert_eq!(req.path, "/api/channels/pause?source=panel");
            assert_eq!(req.header("content-type"), Some("application/json"));
            assert_eq!(req.body, br#"{"id":"missing","paused":true}"#);
            assert!(req.header("origin").is_none());
            assert!(req.header("authorization").is_none());
            crate::http::write_json(&mut stream, 404, expected_body)
                .await
                .unwrap();
        });
        let req = Request {
            method: "POST".into(),
            path: "/api/channels/pause?source=panel".into(),
            headers: HashMap::from([
                ("content-type".into(), "application/json".into()),
                ("host".into(), "app.adi".into()),
                ("origin".into(), "http://app.adi".into()),
                ("authorization".into(), "Basic panel-only".into()),
            ]),
            body: br#"{"id":"missing","paused":true}"#.to_vec(),
            rest: Vec::new(),
        };
        let response = forward_to(&req, &transport_url, service_host).await;
        assert_eq!(response.status, 404);
        assert_eq!(response.body, expected_body);
        upstream.await.unwrap();
    }
}
