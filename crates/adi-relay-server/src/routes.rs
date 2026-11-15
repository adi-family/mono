//! The six routes: sender ingress, register/list/delete, pull, and health. Each handler is a
//! plain function from state + an already-parsed request to a
//! [`Response`] — `main.rs`'s accept loop owns the socket and the method/path dispatch; nothing
//! here touches a `TcpStream`, which is what makes every case below testable without one.

use std::collections::HashMap;

use adi_relay::openssh::Fingerprint;
use base64::Engine as _;
use ed25519_dalek::VerifyingKey;
use serde::{Deserialize, Serialize};

use crate::auth::{self, AuthOutcome};
use crate::config::Mode;
use crate::store::{DeleteOutcome, PulledRecord};
use crate::{AppState, Response, err, now_secs, ok, today};

const BASE32_ALPHABET: &[u8; 32] = b"abcdefghijklmnopqrstuvwxyz234567";

/// `"wh_" + 22 random lowercase-base32 characters` (~110 bits — unguessable is the entire
/// protection a sender gets). `% 32` of a uniform byte is itself uniform (256 is an
/// exact multiple of 32), so this needs no rejection sampling.
fn mint_endpoint_id() -> String {
    use rand::TryRng as _;
    let mut bytes = [0u8; 22];
    rand::rngs::SysRng
        .try_fill_bytes(&mut bytes)
        .expect("the OS random source is unavailable");
    let mut id = String::with_capacity(25);
    id.push_str("wh_");
    for b in bytes {
        id.push(BASE32_ALPHABET[usize::from(b % 32)] as char);
    }
    id
}

fn decode_sealing_key(b64: &str) -> Result<[u8; 32], ()> {
    base64::engine::general_purpose::STANDARD
        .decode(b64)
        .map_err(|_| ())?
        .try_into()
        .map_err(|_| ())
}

/// The shared first step of every route except ingress (which has none at all) and `/healthz`:
/// verify the signature, then — self-host only — check `authorized_keys`, the optional shared
/// secret, and the optional IP allowlist. A key not in the file is `403 unknown_key` everywhere,
/// not just at registration, since nothing gets this far without first registering; the same
/// code covers all three self-host gates alike, since they are the same kind of refusal — this
/// caller may not be here.
fn authenticate(
    state: &AppState,
    headers: &HashMap<String, String>,
    method: &str,
    path_and_query: &str,
    body: &[u8],
    remote_ip: &str,
) -> Result<VerifyingKey, Response> {
    let outcome = auth::authenticate(&state.store, headers, method, path_and_query, body, now_secs())
        .map_err(|e| err(500, "internal_error", e.to_string()))?;
    let key = match outcome {
        AuthOutcome::Ok(key) => key,
        other => return Err(err(401, other.error_code(), "signature check failed")),
    };
    if let Some(self_host) = state.config.self_host.as_ref() {
        let authorized = state
            .authorized_keys
            .as_ref()
            .is_some_and(|keys| keys.is_authorized(&key));
        if !authorized {
            return Err(err(403, "unknown_key", "this key is not in authorized_keys"));
        }
        if let Some(refusal) = auth::check_shared_secret(self_host, headers) {
            return Err(refusal);
        }
        if let Some(refusal) = auth::check_ip_allowlist(self_host, remote_ip) {
            return Err(refusal);
        }
    }
    Ok(key)
}

// ---- 5.6 GET /healthz --------------------------------------------------------------------

#[must_use]
pub fn health() -> &'static str {
    "ok"
}

// ---- 5.1 sender ingress -------------------------------------------------------------------

#[derive(Debug, Serialize)]
struct IngressAck {
    ok: bool,
}

/// `POST|GET|PUT|PATCH|DELETE /h/<endpoint-id>` — capture the request verbatim, sealed, queued
/// for the owning node's next `pull`. Unauthenticated by design: a sender cannot sign with the
/// node's key, so the id itself is the only gate.
pub fn ingress(
    state: &AppState,
    endpoint_id: &str,
    method: &str,
    query: &str,
    headers: &HashMap<String, String>,
    body: &[u8],
) -> Response {
    let endpoint = match state.store.get_endpoint(endpoint_id) {
        Ok(Some(e)) => e,
        Ok(None) => return err(404, "not_found", format!("no such endpoint: {endpoint_id}")),
        Err(e) => return err(500, "internal_error", e.to_string()),
    };

    let now = i64::try_from(now_secs()).unwrap_or(0);
    let recent = state.store.records_since(endpoint_id, now - 60).unwrap_or(i64::MAX);
    if recent >= i64::from(state.config.limits.max_requests_per_minute_per_endpoint) {
        return err(429, "rate_limited", "too many requests to this endpoint");
    }
    let pending = state.store.pending_count(endpoint_id).unwrap_or(i64::MAX);
    let cap = i64::try_from(state.config.limits.max_pending_per_endpoint).unwrap_or(i64::MAX);
    if pending >= cap {
        return err(429, "rate_limited", "too many undelivered records for this endpoint");
    }

    let record = serde_json::json!({
        "method": method,
        "headers": headers,
        "query": query,
        "body_b64": base64::engine::general_purpose::STANDARD.encode(body),
    });
    let plaintext = match serde_json::to_vec(&record) {
        Ok(bytes) => bytes,
        Err(e) => return err(500, "internal_error", e.to_string()),
    };
    if plaintext.len() as u64 > state.config.limits.max_body_bytes {
        return err(413, "payload_too_large", "request is over this relay's body limit");
    }
    let Ok(sealing_public) = decode_sealing_key(&endpoint.sealing_key_b64) else {
        return err(500, "internal_error", "this endpoint's sealing key is corrupt");
    };

    // A paused endpoint (enterprise licence lapsed) still answers 202 and still queues:
    // refusing intake would make the sender's own retry/alerting treat a billing hiccup as an
    // integration failure. `store::pull` is what withholds a paused endpoint's records, not
    // this function.
    let expires_at = now + i64::try_from(endpoint.retention_secs).unwrap_or(i64::MAX);
    #[allow(clippy::cast_possible_wrap)]
    let byte_len = plaintext.len() as i64;
    match state
        .store
        .insert_sealed_record(endpoint_id, &sealing_public, now, expires_at, byte_len, &plaintext)
    {
        Ok(()) => {
            state.notify.notify_waiters();
            ok(202, &IngressAck { ok: true })
        }
        Err(e) => err(500, "internal_error", e.to_string()),
    }
}

// ---- 5.2 POST /v1/endpoints — register -----------------------------------------------------

#[derive(Debug, Deserialize)]
struct RegisterRequest {
    sealing_key: String,
    #[serde(default)]
    retention_secs: Option<u64>,
    #[serde(default)]
    licence_code: Option<String>,
}

#[derive(Debug, Serialize)]
struct RegisterResponse {
    endpoint_id: String,
    retention_secs: u64,
    url: String,
}

/// # Panics
/// Never in practice: the retry loop below only runs out after five id collisions in a row,
/// astronomically unlikely at ~110 bits of entropy per id.
pub async fn register(
    state: &AppState,
    headers: &HashMap<String, String>,
    path_and_query: &str,
    body: &[u8],
    remote_ip: &str,
) -> Response {
    let key = match authenticate(state, headers, "POST", path_and_query, body, remote_ip) {
        Ok(key) => key,
        Err(resp) => return resp,
    };
    let fingerprint = Fingerprint::of_key(&key);
    let key_line = adi_relay::openssh::encode(&key);

    let req: RegisterRequest = match serde_json::from_slice(body) {
        Ok(req) => req,
        Err(_) => {
            return err(
                400,
                "bad_request",
                "expected { sealing_key, retention_secs?, licence_code? }",
            );
        }
    };
    if decode_sealing_key(&req.sealing_key).is_err() {
        return err(400, "bad_request", "sealing_key must be base64 of 32 bytes");
    }

    match state.config.mode {
        Mode::Public => {
            let day = today(now_secs());
            let count = state.store.registrations_today(remote_ip, &day).unwrap_or(i64::MAX);
            if count >= i64::from(state.config.limits.max_registrations_per_ip_per_day) {
                return err(429, "too_many_endpoints", "too many registrations from this address today");
            }
        }
        Mode::Enterprise => {
            if let Some(resp) = check_licence(state, &req, &fingerprint).await {
                return resp;
            }
        }
        Mode::SelfHost => {}
    }

    let existing = state.store.count_endpoints(fingerprint.as_str()).unwrap_or(i64::MAX);
    if existing >= i64::from(state.config.limits.max_endpoints_per_key) {
        return err(429, "too_many_endpoints", "this key already owns the maximum number of endpoints");
    }

    let retention = req
        .retention_secs
        .unwrap_or(state.config.limits.default_retention_secs)
        .min(state.config.limits.max_retention_secs);
    let now = i64::try_from(now_secs()).unwrap_or(0);

    for _ in 0..5 {
        let id = mint_endpoint_id();
        match state.store.endpoint_exists(&id) {
            Ok(true) => continue,
            Ok(false) => {}
            Err(e) => return err(500, "internal_error", e.to_string()),
        }
        return match state
            .store
            .insert_endpoint(&id, fingerprint.as_str(), &key_line, &req.sealing_key, retention, now)
        {
            Ok(()) => {
                if state.config.mode == Mode::Public {
                    let _ = state.store.record_registration(remote_ip, &today(now_secs()));
                }
                ok(
                    201,
                    &RegisterResponse {
                        endpoint_id: id.clone(),
                        retention_secs: retention,
                        url: state.config.endpoint_url(&id),
                    },
                )
            }
            Err(e) => err(500, "internal_error", e.to_string()),
        };
    }
    err(500, "internal_error", "could not mint a unique endpoint id")
}

/// Enterprise mode's register-time licence gate. `None` means proceed; `Some(resp)` is the
/// refusal to answer with. A beam that can't be trusted (transport failure, bad signature, a
/// mismatched nonce/`deployment_id`) refuses registration exactly like a decided non-serving
/// verdict — a deliberate choice to fail closed uniformly under `402 licence_lapsed`, rather
/// than inventing a separate code for "the licence service could not be reached right now".
async fn check_licence(state: &AppState, req: &RegisterRequest, fingerprint: &Fingerprint) -> Option<Response> {
    let Some(code) = req.licence_code.as_deref().filter(|c| !c.is_empty()) else {
        return Some(err(402, "licence_lapsed", "licence_code is required in enterprise mode"));
    };
    let Some(licence) = state.licence.as_ref() else {
        return Some(err(500, "internal_error", "enterprise mode has no licence client configured"));
    };
    match licence.verify(code, fingerprint.as_str()).await {
        Ok(outcome) if outcome.valid => {
            let _ = state
                .store
                .set_licence_status(fingerprint.as_str(), code, true, outcome.checked_at);
            None
        }
        Ok(outcome) => {
            let _ = state
                .store
                .set_licence_status(fingerprint.as_str(), code, false, outcome.checked_at);
            Some(err(402, "licence_lapsed", "licence_code did not verify as paid or trialing"))
        }
        Err(e) => Some(err(402, "licence_lapsed", format!("licence check failed: {e}"))),
    }
}

// ---- 5.3 GET /v1/endpoints — list -----------------------------------------------------------

pub fn list(state: &AppState, headers: &HashMap<String, String>, path_and_query: &str, remote_ip: &str) -> Response {
    let key = match authenticate(state, headers, "GET", path_and_query, b"", remote_ip) {
        Ok(key) => key,
        Err(resp) => return resp,
    };
    let fingerprint = Fingerprint::of_key(&key);
    match state.store.list_endpoints(fingerprint.as_str()) {
        Ok(rows) => {
            let endpoints: Vec<_> = rows
                .into_iter()
                .map(|r| {
                    serde_json::json!({
                        "endpoint_id": r.endpoint_id,
                        "retention_secs": r.retention_secs,
                        "paused": r.paused,
                        "created_at": r.created_at,
                        "pending": r.pending,
                    })
                })
                .collect();
            ok(200, &serde_json::json!({ "endpoints": endpoints }))
        }
        Err(e) => err(500, "internal_error", e.to_string()),
    }
}

// ---- 5.4 DELETE /v1/endpoints/<id> — delete -------------------------------------------------

pub fn delete(state: &AppState, headers: &HashMap<String, String>, path_and_query: &str, id: &str, remote_ip: &str) -> Response {
    let key = match authenticate(state, headers, "DELETE", path_and_query, b"", remote_ip) {
        Ok(key) => key,
        Err(resp) => return resp,
    };
    let fingerprint = Fingerprint::of_key(&key);
    match state.store.delete_endpoint(id, fingerprint.as_str()) {
        Ok(DeleteOutcome::Deleted) => Response {
            status: 204,
            body: String::new(),
        },
        // Not `404`: telling the legitimate owner "that's not yours" versus "doesn't exist" is
        // worth keeping honest — "forbidden" is this implementation's own choice of code.
        Ok(DeleteOutcome::Forbidden) => err(403, "forbidden", "this endpoint belongs to a different key"),
        Ok(DeleteOutcome::NotFound) => err(404, "not_found", format!("no such endpoint: {id}")),
        Err(e) => err(500, "internal_error", e.to_string()),
    }
}

// ---- 5.5 GET /v1/pull — long-poll -----------------------------------------------------------

fn pull_response(records: Vec<PulledRecord>) -> serde_json::Value {
    let records: Vec<_> = records
        .into_iter()
        .map(|r| {
            serde_json::json!({
                "id": r.id,
                "endpoint_id": r.endpoint_id,
                "received_at": r.received_at,
                "sealed": {
                    "ephemeral_pubkey": r.sealed.ephemeral_pubkey_b64(),
                    "nonce": r.sealed.nonce_b64(),
                    "ciphertext": r.sealed.ciphertext_b64(),
                },
            })
        })
        .collect();
    serde_json::json!({ "records": records, "has_more": false })
}

/// `GET /v1/pull?batch=<n>` — waits up to `pull_long_poll_secs` for at least one record across
/// every endpoint the caller's key owns, woken early by [`AppState::notify`] whenever ingress
/// accepts something.
pub async fn pull(
    state: &AppState,
    headers: &HashMap<String, String>,
    path_and_query: &str,
    batch_param: Option<&str>,
    remote_ip: &str,
) -> Response {
    let key = match authenticate(state, headers, "GET", path_and_query, b"", remote_ip) {
        Ok(key) => key,
        Err(resp) => return resp,
    };
    let fingerprint = Fingerprint::of_key(&key);
    let batch = batch_param
        .and_then(|s| s.parse::<u32>().ok())
        .unwrap_or(state.config.limits.pull_batch_max)
        .clamp(1, state.config.limits.pull_batch_max);

    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(state.config.limits.pull_long_poll_secs);
    loop {
        match state.store.pull(fingerprint.as_str(), batch) {
            Ok(records) if !records.is_empty() => return ok(200, &pull_response(records)),
            Ok(_) => {}
            Err(e) => return err(500, "internal_error", e.to_string()),
        }
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            return ok(200, &pull_response(Vec::new()));
        }
        tokio::select! {
            () = state.notify.notified() => {}
            () = tokio::time::sleep(remaining) => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Config, Mode as ConfigMode};
    use crate::store::Store;
    use ed25519_dalek::SigningKey;

    fn state(mode: ConfigMode) -> AppState {
        let config = Config::parse(match mode {
            ConfigMode::Public => {
                "mode=\"public\"\nbind=\"0.0.0.0:8080\"\ndb_path=\"x\"\nhostname=\"eu.webhooks.withadi.dev\"\n"
            }
            ConfigMode::SelfHost => {
                "mode=\"self-host\"\nbind=\"0.0.0.0:8080\"\ndb_path=\"x\"\nhostname=\"h\"\n[self_host]\nauthorized_keys_path=\"/nonexistent\"\n"
            }
            ConfigMode::Enterprise => unreachable!("not exercised directly in this module's tests"),
        })
        .expect("parses");
        AppState {
            store: Store::open_in_memory().expect("store"),
            config,
            licence: None,
            authorized_keys: config_authorized_keys(mode),
            notify: tokio::sync::Notify::new(),
        }
    }

    fn config_authorized_keys(mode: ConfigMode) -> Option<auth::AuthorizedKeys> {
        matches!(mode, ConfigMode::SelfHost).then(|| auth::AuthorizedKeys::new("/nonexistent".into()))
    }

    fn signed_headers(signing: &SigningKey, method: &str, path_and_query: &str, body: &[u8]) -> HashMap<String, String> {
        let req = adi_relay::sign::sign(signing, method, path_and_query, now_secs(), &adi_relay::sign::generate_nonce(), body);
        HashMap::from([
            (adi_relay::sign::HEADER_KEY.to_string(), req.key_line),
            (adi_relay::sign::HEADER_TIMESTAMP.to_string(), req.timestamp.to_string()),
            (adi_relay::sign::HEADER_NONCE.to_string(), req.nonce),
            (adi_relay::sign::HEADER_SIGNATURE.to_string(), req.signature),
        ])
    }

    fn node_sealing_public() -> ([u8; 32], x25519_dalek::StaticSecret) {
        let secret = x25519_dalek::StaticSecret::from([9u8; 32]);
        let public = *x25519_dalek::PublicKey::from(&secret).as_bytes();
        (public, secret)
    }

    #[tokio::test]
    async fn register_then_list_then_delete_round_trips_in_public_mode() {
        let state = state(ConfigMode::Public);
        let signing = SigningKey::from_bytes(&[1u8; 32]);
        let (sealing_public, _secret) = node_sealing_public();
        let body = serde_json::to_vec(&serde_json::json!({
            "sealing_key": base64::engine::general_purpose::STANDARD.encode(sealing_public),
        }))
        .unwrap();
        let headers = signed_headers(&signing, "POST", "/v1/endpoints", &body);
        let resp = register(&state, &headers, "/v1/endpoints", &body, "1.2.3.4").await;
        assert_eq!(resp.status, 201, "{}", resp.body);
        let v: serde_json::Value = serde_json::from_str(&resp.body).unwrap();
        let id = v["endpoint_id"].as_str().unwrap().to_string();
        assert!(id.starts_with("wh_"));
        assert_eq!(v["retention_secs"], 300);
        assert!(v["url"].as_str().unwrap().ends_with(&id));

        let list_headers = signed_headers(&signing, "GET", "/v1/endpoints", b"");
        let listed = list(&state, &list_headers, "/v1/endpoints", "203.0.113.1");
        assert_eq!(listed.status, 200, "{}", listed.body);
        let v: serde_json::Value = serde_json::from_str(&listed.body).unwrap();
        assert_eq!(v["endpoints"].as_array().unwrap().len(), 1);
        assert_eq!(v["endpoints"][0]["endpoint_id"], id);

        let other = SigningKey::from_bytes(&[2u8; 32]);
        let path = format!("/v1/endpoints/{id}");
        let forbidden_headers = signed_headers(&other, "DELETE", &path, b"");
        let forbidden = delete(&state, &forbidden_headers, &path, &id, "203.0.113.1");
        assert_eq!(forbidden.status, 403);

        let delete_headers = signed_headers(&signing, "DELETE", &path, b"");
        let deleted = delete(&state, &delete_headers, &path, &id, "203.0.113.1");
        assert_eq!(deleted.status, 204);
        let again = delete(&state, &signed_headers(&signing, "DELETE", &path, b""), &path, &id, "203.0.113.1");
        assert_eq!(again.status, 404);
    }

    #[test]
    fn ingress_queues_a_record_and_pull_delivers_and_removes_it() {
        let state = state(ConfigMode::Public);
        let (sealing_public, sealing_secret) = node_sealing_public();
        state
            .store
            .insert_endpoint(
                "wh_test",
                "fp1",
                "ssh-ed25519 AAA",
                &base64::engine::general_purpose::STANDARD.encode(sealing_public),
                300,
                1000,
            )
            .unwrap();

        let mut headers = HashMap::new();
        headers.insert("x-hub-signature-256".to_string(), "sha256=abc".to_string());
        let resp = ingress(&state, "wh_test", "POST", "", &headers, b"{\"a\":1}");
        assert_eq!(resp.status, 202, "{}", resp.body);

        let pulled = state.store.pull("fp1", 10).expect("pull");
        assert_eq!(pulled.len(), 1);
        let opened = adi_relay::seal::open(&sealing_secret, "wh_test", pulled[0].id, &pulled[0].sealed).expect("open");
        let record: serde_json::Value = serde_json::from_slice(&opened).unwrap();
        assert_eq!(record["method"], "POST");
        assert_eq!(record["headers"]["x-hub-signature-256"], "sha256=abc");
        let body = base64::engine::general_purpose::STANDARD
            .decode(record["body_b64"].as_str().unwrap())
            .unwrap();
        assert_eq!(body, b"{\"a\":1}");

        assert!(state.store.pull("fp1", 10).unwrap().is_empty(), "delete-at-handout");
    }

    #[test]
    fn ingress_404s_an_unknown_endpoint() {
        let state = state(ConfigMode::Public);
        let resp = ingress(&state, "wh_ghost", "GET", "", &HashMap::new(), b"");
        assert_eq!(resp.status, 404);
    }

    #[tokio::test]
    async fn self_host_refuses_a_key_not_in_authorized_keys() {
        let state = state(ConfigMode::SelfHost);
        let signing = SigningKey::from_bytes(&[3u8; 32]);
        let (sealing_public, _secret) = node_sealing_public();
        let body = serde_json::to_vec(&serde_json::json!({
            "sealing_key": base64::engine::general_purpose::STANDARD.encode(sealing_public),
        }))
        .unwrap();
        let headers = signed_headers(&signing, "POST", "/v1/endpoints", &body);
        let resp = register(&state, &headers, "/v1/endpoints", &body, "1.2.3.4").await;
        assert_eq!(resp.status, 403);
        let v: serde_json::Value = serde_json::from_str(&resp.body).unwrap();
        assert_eq!(v["error"], "unknown_key");
    }

    /// Self-host's two defence-in-depth layers, both only reached once `authorized_keys`
    /// already passed — an authorized key from the wrong address, or with no shared secret,
    /// must still be refused.
    #[tokio::test]
    async fn self_host_enforces_its_optional_shared_secret_and_ip_allowlist() {
        let signing = SigningKey::from_bytes(&[8u8; 32]);
        let dir = tempfile::tempdir().unwrap();
        let keys_path = dir.path().join("authorized_keys");
        std::fs::write(&keys_path, adi_relay::openssh::encode(&signing.verifying_key())).unwrap();

        // SAFETY: test-only, and this name is unique to this test.
        unsafe { std::env::set_var("ADI_RELAY_ROUTES_TEST_SECRET", "s3cr3t") };
        let config = crate::config::Config::parse(&format!(
            "mode=\"self-host\"\nbind=\"0.0.0.0:8080\"\ndb_path=\"x\"\nhostname=\"h\"\n\
             [self_host]\nauthorized_keys_path=\"{}\"\nshared_secret_env=\"ADI_RELAY_ROUTES_TEST_SECRET\"\nip_allowlist=[\"10.0.0.0/8\"]\n",
            keys_path.display()
        ))
        .expect("parses");
        let state = AppState {
            store: Store::open_in_memory().expect("store"),
            authorized_keys: Some(auth::AuthorizedKeys::new(keys_path)),
            licence: None,
            config,
            notify: tokio::sync::Notify::new(),
        };

        let (sealing_public, _secret) = node_sealing_public();
        let body = serde_json::to_vec(&serde_json::json!({
            "sealing_key": base64::engine::general_purpose::STANDARD.encode(sealing_public),
        }))
        .unwrap();
        // Each call signs its own request — a signature is good for exactly one nonce.

        // The right address, but no shared secret header at all.
        let headers = signed_headers(&signing, "POST", "/v1/endpoints", &body);
        let resp = register(&state, &headers, "/v1/endpoints", &body, "10.1.2.3").await;
        assert_eq!(resp.status, 403, "{}", resp.body);

        // The right secret, but an address outside the allowlist.
        let mut with_secret = signed_headers(&signing, "POST", "/v1/endpoints", &body);
        with_secret.insert(auth::HEADER_SHARED_SECRET.to_string(), "s3cr3t".to_string());
        let resp = register(&state, &with_secret, "/v1/endpoints", &body, "203.0.113.1").await;
        assert_eq!(resp.status, 403, "{}", resp.body);

        // Both satisfied: through.
        let mut both = signed_headers(&signing, "POST", "/v1/endpoints", &body);
        both.insert(auth::HEADER_SHARED_SECRET.to_string(), "s3cr3t".to_string());
        let resp = register(&state, &both, "/v1/endpoints", &body, "10.1.2.3").await;
        assert_eq!(resp.status, 201, "{}", resp.body);
    }

    #[tokio::test]
    async fn a_bad_signature_is_refused_before_anything_else() {
        let state = state(ConfigMode::Public);
        let mut headers = HashMap::new();
        headers.insert(adi_relay::sign::HEADER_KEY.to_string(), "not a key line".to_string());
        headers.insert(adi_relay::sign::HEADER_TIMESTAMP.to_string(), now_secs().to_string());
        headers.insert(adi_relay::sign::HEADER_NONCE.to_string(), "n".to_string());
        headers.insert(adi_relay::sign::HEADER_SIGNATURE.to_string(), "bad".to_string());
        let resp = register(&state, &headers, "/v1/endpoints", b"{}", "1.2.3.4").await;
        assert_eq!(resp.status, 401);
        let v: serde_json::Value = serde_json::from_str(&resp.body).unwrap();
        assert_eq!(v["error"], "bad_signature");
    }

    #[tokio::test]
    async fn pull_returns_immediately_when_something_is_already_queued() {
        let state = state(ConfigMode::Public);
        let signing = SigningKey::from_bytes(&[4u8; 32]);
        let fingerprint = Fingerprint::of_key(&signing.verifying_key());
        let (sealing_public, _secret) = node_sealing_public();
        state
            .store
            .insert_endpoint(
                "wh_a",
                fingerprint.as_str(),
                "k",
                &base64::engine::general_purpose::STANDARD.encode(sealing_public),
                300,
                1000,
            )
            .unwrap();
        state
            .store
            .insert_sealed_record("wh_a", &sealing_public, 1, 2, 1, b"x")
            .unwrap();

        let headers = signed_headers(&signing, "GET", "/v1/pull?batch=10", b"");
        let resp = tokio::time::timeout(
            std::time::Duration::from_secs(1),
            pull(&state, &headers, "/v1/pull?batch=10", Some("10"), "203.0.113.1"),
        )
        .await
        .expect("must not block when a record is already queued");
        assert_eq!(resp.status, 200);
        let v: serde_json::Value = serde_json::from_str(&resp.body).unwrap();
        assert_eq!(v["records"].as_array().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn pull_times_out_and_answers_empty_when_nothing_is_queued() {
        let mut state = state(ConfigMode::Public);
        state.config.limits.pull_long_poll_secs = 0;
        let signing = SigningKey::from_bytes(&[5u8; 32]);
        let headers = signed_headers(&signing, "GET", "/v1/pull", b"");
        let resp = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            pull(&state, &headers, "/v1/pull", None, "203.0.113.1"),
        )
            .await
            .expect("must not hang");
        assert_eq!(resp.status, 200);
        let v: serde_json::Value = serde_json::from_str(&resp.body).unwrap();
        assert!(v["records"].as_array().unwrap().is_empty());
    }

    #[tokio::test]
    async fn pull_wakes_early_when_ingress_notifies() {
        let state = std::sync::Arc::new(state(ConfigMode::Public));
        let signing = SigningKey::from_bytes(&[6u8; 32]);
        let fingerprint = Fingerprint::of_key(&signing.verifying_key());
        let (sealing_public, _secret) = node_sealing_public();
        state
            .store
            .insert_endpoint(
                "wh_a",
                fingerprint.as_str(),
                "k",
                &base64::engine::general_purpose::STANDARD.encode(sealing_public),
                300,
                1000,
            )
            .unwrap();

        let headers = signed_headers(&signing, "GET", "/v1/pull", b"");
        let state2 = std::sync::Arc::clone(&state);
        let handle = tokio::spawn(async move { pull(&state2, &headers, "/v1/pull", None, "203.0.113.1").await });

        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        state
            .store
            .insert_sealed_record("wh_a", &sealing_public, 1, 2, 1, b"x")
            .unwrap();
        state.notify.notify_waiters();

        let resp = tokio::time::timeout(std::time::Duration::from_secs(2), handle)
            .await
            .expect("must wake promptly")
            .unwrap();
        assert_eq!(resp.status, 200);
        let v: serde_json::Value = serde_json::from_str(&resp.body).unwrap();
        assert_eq!(v["records"].as_array().unwrap().len(), 1);
    }
}
