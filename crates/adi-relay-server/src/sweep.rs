//! Background sweeps: retention/nonce/registration purge every 30 seconds, and — enterprise
//! mode only — the licence recheck once every `recheck_interval_secs` (default 24h).

use std::sync::Arc;
use std::time::Duration;

use crate::{AppState, now_secs, today};

/// A background purge pass, every 30 seconds.
const PURGE_INTERVAL: Duration = Duration::from_secs(30);

/// `registrations_by_ip` rows older than this are purged — the public cap only needs a rolling
/// day, so 2 days of margin is plenty.
const REGISTRATION_RETENTION_DAYS: u64 = 2;

/// One purge pass: expired records, expired nonces, and old `registrations_by_ip` rows.
/// Failures are logged, not fatal — a sweep that misses a cycle simply tries again at the next
/// one, same stance the retention clock itself takes toward a record nobody happened to pull.
pub fn purge_once(state: &AppState) {
    let now = i64::try_from(now_secs()).unwrap_or(0);
    if let Err(e) = state.store.purge_expired_records(now) {
        tracing::warn!(error = %e, "purging expired records failed");
    }
    if let Err(e) = state.store.purge_expired_nonces(now) {
        tracing::warn!(error = %e, "purging expired nonces failed");
    }
    let cutoff = today(now_secs().saturating_sub(REGISTRATION_RETENTION_DAYS * 86_400));
    if let Err(e) = state.store.purge_old_registrations(&cutoff) {
        tracing::warn!(error = %e, "purging old registrations_by_ip rows failed");
    }
}

/// Runs [`purge_once`] every [`PURGE_INTERVAL`] until the process exits — never returns.
pub async fn run_purge_loop(state: Arc<AppState>) {
    loop {
        tokio::time::sleep(PURGE_INTERVAL).await;
        purge_once(&state);
    }
}

/// Re-verify every cached licence verdict, once every `recheck_interval_secs`. **A beam that
/// cannot be trusted this cycle leaves that fingerprint's cached status untouched**, retried
/// next cycle — unlike `register`'s own uniform fail-closed choice, pausing every enterprise
/// customer's endpoints over one momentary network blip to the licence service would be a worse
/// failure mode than serving one more interval on a stale-but-last-known-good verdict. Only a
/// successful, signed, decided
/// *non-serving* verdict pauses a fingerprint's endpoints; a successful serving verdict unpauses
/// them (a renewal, or a row that was never paused, both no-ops either way).
pub async fn recheck_licences_once(state: &AppState) {
    let Some(licence) = state.licence.as_ref() else {
        return;
    };
    let rows = match state.store.all_licence_rows() {
        Ok(rows) => rows,
        Err(e) => {
            tracing::warn!(error = %e, "reading cached licence rows failed");
            return;
        }
    };
    for (fingerprint, licence_code) in rows {
        match licence.verify(&licence_code, &fingerprint).await {
            Ok(outcome) => {
                if let Err(e) = state.store.set_licence_status(
                    &fingerprint,
                    &licence_code,
                    outcome.valid,
                    outcome.checked_at,
                ) {
                    tracing::warn!(error = %e, %fingerprint, "recording a licence recheck failed");
                    continue;
                }
                if let Err(e) = state
                    .store
                    .set_endpoints_paused(&fingerprint, !outcome.valid)
                {
                    tracing::warn!(error = %e, %fingerprint, "(un)pausing endpoints after a licence recheck failed");
                }
            }
            Err(e) => {
                tracing::warn!(error = %e, %fingerprint, "licence recheck beam failed; keeping the last known status");
            }
        }
    }
}

/// Runs [`recheck_licences_once`] every `recheck_interval_secs` until the process exits — a
/// no-op loop (sleeps forever, checking nothing) in public or self-host mode, where
/// `state.licence` is `None`.
pub async fn run_licence_recheck_loop(state: Arc<AppState>) {
    let Some(interval) = state.licence_recheck_interval() else {
        return;
    };
    loop {
        tokio::time::sleep(interval).await;
        recheck_licences_once(&state).await;
    }
}

impl AppState {
    fn licence_recheck_interval(&self) -> Option<Duration> {
        self.config
            .licence
            .as_ref()
            .map(|l| Duration::from_secs(l.recheck_interval_secs))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::licence::LicenceClient;
    use crate::store::Store;
    use base64::Engine as _;
    use ed25519_dalek::{Signature, Signer as _, SigningKey};

    fn enterprise_state(service_url: String, public_key: ed25519_dalek::VerifyingKey) -> AppState {
        let config = Config::parse(&format!(
            "mode=\"enterprise\"\nbind=\"0.0.0.0:8080\"\ndb_path=\"x\"\nhostname=\"h\"\n[licence]\nservice_url=\"{service_url}\"\npublic_key=\"{}\"\n",
            base64::engine::general_purpose::STANDARD.encode(public_key.to_bytes())
        ))
        .expect("parses");
        AppState {
            store: Store::open_in_memory().expect("store"),
            licence: Some(LicenceClient::with_public_key(service_url, public_key)),
            config,
            authorized_keys: None,
            notify: tokio::sync::Notify::new(),
        }
    }

    #[test]
    fn purge_once_does_not_panic_on_an_empty_store() {
        let state = enterprise_state(
            "https://x".into(),
            SigningKey::from_bytes(&[1u8; 32]).verifying_key(),
        );
        purge_once(&state); // must not panic
    }

    #[tokio::test]
    async fn recheck_skips_a_fingerprint_with_no_licence_client() {
        let config = Config::parse(
            "mode=\"public\"\nbind=\"0.0.0.0:8080\"\ndb_path=\"x\"\nhostname=\"h\"\n",
        )
        .unwrap();
        let state = AppState {
            store: Store::open_in_memory().unwrap(),
            config,
            licence: None,
            authorized_keys: None,
            notify: tokio::sync::Notify::new(),
        };
        // No licence client configured (public mode) -> must return immediately, not panic.
        recheck_licences_once(&state).await;
    }

    #[test]
    fn licence_recheck_interval_is_none_without_a_licence_section() {
        let config = Config::parse(
            "mode=\"public\"\nbind=\"0.0.0.0:8080\"\ndb_path=\"x\"\nhostname=\"h\"\n",
        )
        .unwrap();
        let state = AppState {
            store: Store::open_in_memory().unwrap(),
            config,
            licence: None,
            authorized_keys: None,
            notify: tokio::sync::Notify::new(),
        };
        assert_eq!(state.licence_recheck_interval(), None);
    }

    /// A direct, no-network exercise of the pause/unpause side effect `recheck_licences_once`
    /// has once a verdict is in hand — reusing `LicenceClient::with_public_key` and a
    /// hand-signed verdict exactly as `licence.rs`'s own tests do, so no real `/v1/verify` call
    /// is needed to prove the sweep's own logic.
    #[tokio::test]
    async fn a_decided_non_serving_verdict_pauses_the_fingerprints_endpoints() {
        let signing = SigningKey::from_bytes(&[9u8; 32]);
        // A server that always answers the same signed "expired" verdict for fp1.
        let server = fake_licence_server(signing.clone(), "expired", "fp1").await;
        let state = enterprise_state(server, signing.verifying_key());
        state
            .store
            .insert_endpoint("wh_a", "fp1", "k", "seal", 300, 1000)
            .unwrap();
        state
            .store
            .set_licence_status("fp1", "ADI-ENT-X", true, 1)
            .unwrap();

        recheck_licences_once(&state).await;

        let endpoints = state.store.list_endpoints("fp1").unwrap();
        assert!(
            endpoints[0].paused,
            "a decided expired verdict must pause the endpoint"
        );
        assert!(!state.store.get_licence_status("fp1").unwrap().unwrap().0);
    }

    /// A minimal in-process HTTP server answering `/v1/verify` with one fixed, correctly signed
    /// verdict — just enough to exercise `LicenceClient::verify`'s real network path from this
    /// test module, without any external service.
    async fn fake_licence_server(
        signing: SigningKey,
        status: &'static str,
        fingerprint: &'static str,
    ) -> String {
        use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            loop {
                let Ok((mut stream, _)) = listener.accept().await else {
                    return;
                };
                let signing = signing.clone();
                tokio::spawn(async move {
                    let mut buf = [0u8; 4096];
                    let mut received = Vec::new();
                    // Read until the full head *and* its declared body have both arrived — a
                    // single short read risks splitting the small JSON body from its headers,
                    // which this test fixture (unlike the real `http.rs`) doesn't bother to
                    // frame properly otherwise.
                    let body_len = loop {
                        let n = stream.read(&mut buf).await.unwrap_or(0);
                        if n == 0 {
                            break 0;
                        }
                        received.extend_from_slice(&buf[..n]);
                        let Some(head_end) = received.windows(4).position(|w| w == b"\r\n\r\n")
                        else {
                            continue;
                        };
                        let head_text = String::from_utf8_lossy(&received[..head_end]);
                        let declared = head_text
                            .lines()
                            .find_map(|l| {
                                l.to_ascii_lowercase()
                                    .strip_prefix("content-length:")
                                    .map(|v| v.trim().to_string())
                            })
                            .and_then(|v| v.parse::<usize>().ok())
                            .unwrap_or(0);
                        if received.len() >= head_end + 4 + declared {
                            break declared;
                        }
                    };
                    let _ = body_len;
                    let text = String::from_utf8_lossy(&received);
                    let sent_nonce = text
                        .split("\"nonce\":\"")
                        .nth(1)
                        .and_then(|rest| rest.split('"').next())
                        .unwrap_or("")
                        .to_string();
                    let deployment_id =
                        crate::licence::LicenceClient::synthetic_deployment_id(fingerprint);
                    let verdict = serde_json::json!({
                        "checked_at": crate::now_secs(),
                        "company": "Test",
                        "deployment_id": deployment_id,
                        "expires_at": null,
                        "licence_code": "ADI-ENT-X",
                        "max_stale": 86_400,
                        "nonce": sent_nonce,
                        "recheck_after": 21_600,
                        "status": status,
                        "trial_ends_at": null,
                    });
                    let canonical = serde_json::to_vec(&verdict).unwrap();
                    let signature: Signature = signing.sign(&canonical);
                    let body = serde_json::json!({
                        "protocol": 1,
                        "verdict": verdict,
                        "signature": base64::engine::general_purpose::STANDARD.encode(signature.to_bytes()),
                    })
                    .to_string();
                    let response = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        body.len(),
                        body
                    );
                    let _ = stream.write_all(response.as_bytes()).await;
                });
            }
        });
        format!("http://{addr}")
    }
}
