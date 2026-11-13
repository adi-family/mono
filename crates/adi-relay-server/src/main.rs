//! The webhook relay binary (`docs/webhook-relay.md`, ADI-MONO-222): read `relay.toml`, open the
//! store, run the two background sweeps, and accept connections — one request per connection,
//! `Connection: close`, same as `adi-webapp-api`'s own HTTP layer.

use std::net::SocketAddr;
use std::sync::Arc;

use adi_relay_server::{AppState, Response, config::Config, err, http, routes, sweep};
use tokio::net::{TcpListener, TcpStream};
use tracing::{debug, info, warn};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    tracing_subscriber::fmt().with_env_filter(filter).with_target(false).init();
    // Only needed in enterprise mode (the licence beam), but installing it unconditionally is
    // harmless and one line simpler than threading the mode through to decide.
    rustls::crypto::ring::default_provider().install_default().ok();

    let config_path = std::env::args().nth(1).unwrap_or_else(|| "relay.toml".to_string());
    let config = Config::load(std::path::Path::new(&config_path))?;
    info!(mode = %config.mode, bind = %config.bind, hostname = %config.hostname, "starting adi-relay-server");

    let bind = config.bind;
    let state = Arc::new(AppState::open(config)?);
    let listener = TcpListener::bind(bind).await?;

    tokio::spawn(sweep::run_purge_loop(Arc::clone(&state)));
    tokio::spawn(sweep::run_licence_recheck_loop(Arc::clone(&state)));

    loop {
        tokio::select! {
            accepted = listener.accept() => match accepted {
                Ok((stream, peer)) => {
                    let state = Arc::clone(&state);
                    tokio::spawn(async move {
                        if let Err(e) = handle(stream, peer, &state).await {
                            debug!(%peer, error = %e, "connection error");
                        }
                    });
                }
                Err(e) => warn!(error = %e, "accept failed"),
            },
            () = adi_osext::shutdown_signal() => {
                info!("shutdown signal received; stopping");
                break;
            }
        }
    }
    Ok(())
}

async fn handle(mut stream: TcpStream, peer: SocketAddr, state: &AppState) -> anyhow::Result<()> {
    let Some(req) = http::read_request(&mut stream, state.config.limits.max_body_bytes).await? else {
        return Ok(());
    };
    debug!(method = %req.method, path = %req.path, "request");

    if req.method == "GET" && req.route_path() == "/healthz" {
        http::write_text(&mut stream, 200, routes::health()).await?;
        return Ok(());
    }

    let resp = dispatch(state, &req, peer.ip().to_string()).await;
    http::write_json(&mut stream, resp.status, &resp.body).await?;
    Ok(())
}

async fn dispatch(state: &AppState, req: &http::Request, remote_ip: String) -> Response {
    let path = req.route_path();
    let path_and_query = req.path.as_str();
    match (req.method.as_str(), path) {
        ("POST", "/v1/endpoints") => {
            routes::register(state, &req.headers, path_and_query, &req.body, &remote_ip).await
        }
        ("GET", "/v1/endpoints") => routes::list(state, &req.headers, path_and_query),
        ("GET", "/v1/pull") => routes::pull(state, &req.headers, path_and_query, req.query_param("batch")).await,
        ("DELETE", p) if p.starts_with("/v1/endpoints/") => {
            let id = &p["/v1/endpoints/".len()..];
            routes::delete(state, &req.headers, path_and_query, id)
        }
        // §5.1: any method is captured, including a provider's GET ping.
        (m, p) if p.starts_with("/h/") && matches!(m, "POST" | "GET" | "PUT" | "PATCH" | "DELETE") => {
            let id = &p["/h/".len()..];
            let query = req.path.split_once('?').map_or("", |(_, q)| q);
            routes::ingress(state, id, &req.method, query, &req.headers, &req.body)
        }
        _ => err(404, "not_found", "no such route"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use adi_relay_server::config::Config;
    use base64::Engine as _;
    use ed25519_dalek::SigningKey;
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

    /// Start a real relay (public mode, in-memory-equivalent scratch db) on an ephemeral port
    /// and hand back the address every test in this module speaks raw HTTP to.
    async fn start_server() -> SocketAddr {
        let db = tempfile::NamedTempFile::new().expect("scratch db");
        let config = Config::parse(&format!(
            "mode=\"public\"\nbind=\"127.0.0.1:0\"\ndb_path=\"{}\"\nhostname=\"relay.example\"\n",
            db.path().display()
        ))
        .expect("parses");
        let state = Arc::new(AppState::open(config).expect("open"));
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("addr");
        tokio::spawn(async move {
            loop {
                let Ok((stream, peer)) = listener.accept().await else { return };
                let state = Arc::clone(&state);
                tokio::spawn(async move {
                    let _ = handle(stream, peer, &state).await;
                });
            }
        });
        // Leak the temp file's lifetime into this task's own leak — fine for a short-lived test.
        std::mem::forget(db);
        addr
    }

    async fn raw_request(addr: SocketAddr, head: &str, body: &[u8]) -> (u16, String) {
        let mut stream = TcpStream::connect(addr).await.expect("connect");
        let full = format!("{head}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
        stream.write_all(full.as_bytes()).await.unwrap();
        stream.write_all(body).await.unwrap();
        let mut response = Vec::new();
        stream.read_to_end(&mut response).await.unwrap();
        let text = String::from_utf8_lossy(&response);
        let status = text
            .split_whitespace()
            .nth(1)
            .and_then(|s| s.parse::<u16>().ok())
            .unwrap_or(0);
        let body = text.split("\r\n\r\n").nth(1).unwrap_or("").to_string();
        (status, body)
    }

    #[tokio::test]
    async fn healthz_answers_plain_text_ok() {
        let addr = start_server().await;
        let (status, body) = raw_request(addr, "GET /healthz HTTP/1.1\r\nHost: x", b"").await;
        assert_eq!(status, 200);
        assert_eq!(body, "ok");
    }

    #[tokio::test]
    async fn an_unknown_route_is_a_404() {
        let addr = start_server().await;
        let (status, body) = raw_request(addr, "GET /nope HTTP/1.1\r\nHost: x", b"").await;
        assert_eq!(status, 404);
        assert!(body.contains("not_found"));
    }

    #[tokio::test]
    async fn end_to_end_register_ingress_pull_over_real_sockets() {
        let addr = start_server().await;
        let signing = SigningKey::from_bytes(&[42u8; 32]);
        let sealing_secret = x25519_dalek::StaticSecret::from([7u8; 32]);
        let sealing_public = *x25519_dalek::PublicKey::from(&sealing_secret).as_bytes();

        // 1. register, signed.
        let reg_body = serde_json::to_vec(&serde_json::json!({
            "sealing_key": base64::engine::general_purpose::STANDARD.encode(sealing_public),
        }))
        .unwrap();
        let now = adi_relay_server::now_secs();
        let signed = adi_relay::sign::sign(
            &signing,
            "POST",
            "/v1/endpoints",
            now,
            &adi_relay::sign::generate_nonce(),
            &reg_body,
        );
        let head = format!(
            "POST /v1/endpoints HTTP/1.1\r\nHost: relay\r\n{}: {}\r\n{}: {}\r\n{}: {}\r\n{}: {}",
            adi_relay::sign::HEADER_KEY,
            signed.key_line,
            adi_relay::sign::HEADER_TIMESTAMP,
            signed.timestamp,
            adi_relay::sign::HEADER_NONCE,
            signed.nonce,
            adi_relay::sign::HEADER_SIGNATURE,
            signed.signature,
        );
        let (status, body) = raw_request(addr, &head, &reg_body).await;
        assert_eq!(status, 201, "{body}");
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        let endpoint_id = v["endpoint_id"].as_str().unwrap().to_string();

        // 2. a sender posts to /h/<id>, unauthenticated.
        let sender_body = br#"{"event":"push"}"#;
        let (status, body) = raw_request(
            addr,
            &format!("POST /h/{endpoint_id} HTTP/1.1\r\nHost: relay\r\nX-Hub-Signature-256: sha256=abc"),
            sender_body,
        )
        .await;
        assert_eq!(status, 202, "{body}");

        // 3. the node pulls, signed, and opens the sealed record.
        let pull_nonce = adi_relay::sign::generate_nonce();
        let pull_now = adi_relay_server::now_secs();
        let pull_signed = adi_relay::sign::sign(&signing, "GET", "/v1/pull", pull_now, &pull_nonce, b"");
        let head = format!(
            "GET /v1/pull HTTP/1.1\r\nHost: relay\r\n{}: {}\r\n{}: {}\r\n{}: {}\r\n{}: {}",
            adi_relay::sign::HEADER_KEY,
            pull_signed.key_line,
            adi_relay::sign::HEADER_TIMESTAMP,
            pull_signed.timestamp,
            adi_relay::sign::HEADER_NONCE,
            pull_signed.nonce,
            adi_relay::sign::HEADER_SIGNATURE,
            pull_signed.signature,
        );
        let (status, body) = raw_request(addr, &head, b"").await;
        assert_eq!(status, 200, "{body}");
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        let records = v["records"].as_array().unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0]["endpoint_id"], endpoint_id);

        let sealed = adi_relay::seal::Sealed::from_b64(
            records[0]["sealed"]["ephemeral_pubkey"].as_str().unwrap(),
            records[0]["sealed"]["nonce"].as_str().unwrap(),
            records[0]["sealed"]["ciphertext"].as_str().unwrap(),
        )
        .expect("decodes");
        let opened = adi_relay::seal::open(&sealing_secret, &endpoint_id, 1, &sealed).expect("opens");
        let record: serde_json::Value = serde_json::from_slice(&opened).unwrap();
        assert_eq!(record["method"], "POST");
        assert_eq!(record["headers"]["x-hub-signature-256"], "sha256=abc");
        let sender_bytes = base64::engine::general_purpose::STANDARD
            .decode(record["body_b64"].as_str().unwrap())
            .unwrap();
        assert_eq!(sender_bytes, sender_body);
    }
}
