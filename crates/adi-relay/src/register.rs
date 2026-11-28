//! The node-side relay calls a trigger save or delete makes against a relay (ADI-MONO-223): HTTP
//! signing, `POST /v1/endpoints` (register), `DELETE /v1/endpoints/<id>` (best-effort), and the
//! `relay`/`relay_endpoint` bookkeeping ([`plan`], [`register_on_save`]) that decides whether a
//! save needs either.
//!
//! **One implementation, shared by `adi-app`'s `POST /api/triggers/save` handler and `adi-mono
//! triggers save`'s own save (ADI-MONO-227)** — a trigger registered through the panel and one
//! registered through the CLI behave identically: a failed registration is a failed save, never
//! a trigger left half-configured with a `relay` nobody answered for. Neither caller is a store:
//! [`register_on_save`] only decides what the relay call should be and mutates the manifest's
//! `extra` in place — persisting it is still the caller's job.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use base64::Engine as _;
use tracing::debug;

use crate::keys::RelayIdentity;

/// The four hosted relays the task names, plus `custom:<url>` for a self-host operator's own
/// instance. `None` for anything else — an unknown name is left for the panel/CLI's own
/// validation to catch, not guessed at here.
#[must_use]
pub fn resolve_url(relay: &str) -> Option<String> {
    match relay {
        "public-eu" => Some("https://eu.webhooks.withadi.dev".to_string()),
        "public-us" => Some("https://us.webhooks.withadi.dev".to_string()),
        "enterprise-eu" => Some("https://eu.ent.webhooks.withadi.dev".to_string()),
        "enterprise-us" => Some("https://us.ent.webhooks.withadi.dev".to_string()),
        other => other
            .strip_prefix("custom:")
            .map(|u| u.trim().trim_end_matches('/').to_string())
            .filter(|u| !u.is_empty()),
    }
}

/// The public sender-facing URL for a live registration — `{relay_url}/h/{endpoint_id}`, exactly
/// how the relay itself builds it (`adi-relay-server::config::Config::public_url`). `None` when
/// `relay` doesn't resolve to a URL.
#[must_use]
pub fn public_url(relay: &str, endpoint_id: &str) -> Option<String> {
    resolve_url(relay).map(|base| format!("{base}/h/{endpoint_id}"))
}

/// reqwest 0.13 is taken workspace-wide with `rustls-no-provider`; a binary's own `main` usually
/// installs the provider once at startup, but a one-shot caller (a unit test, a CLI command that
/// never runs that `main`) never does — see `adi-marketplace/src/fetch.rs::get_https` for the
/// same line, idempotent by design.
fn ensure_crypto_provider() {
    rustls::crypto::ring::default_provider()
        .install_default()
        .ok();
}

/// A client for one relay call, shared by every caller in this module — the register/delete
/// calls here and `adi-app`'s own poller (`pull`, `replay`).
#[must_use]
pub fn http_client() -> reqwest::Client {
    ensure_crypto_provider();
    reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .build()
        .unwrap_or_default()
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

/// The four `X-Relay-*` headers for one request, signed with this node's key.
#[must_use]
pub fn signed_headers(
    identity: &RelayIdentity,
    method: &str,
    path_and_query: &str,
    body: &[u8],
) -> Vec<(String, String)> {
    let signed = crate::sign::sign(
        &identity.signing,
        method,
        path_and_query,
        now_secs(),
        &crate::sign::generate_nonce(),
        body,
    );
    vec![
        (crate::sign::HEADER_KEY.to_string(), signed.key_line),
        (
            crate::sign::HEADER_TIMESTAMP.to_string(),
            signed.timestamp.to_string(),
        ),
        (crate::sign::HEADER_NONCE.to_string(), signed.nonce),
        (crate::sign::HEADER_SIGNATURE.to_string(), signed.signature),
    ]
}

/// The `{ "error": "...", "message": "..." }` body every relay non-2xx answers with — the
/// message if it parses as that shape, else a generic one naming the status.
#[must_use]
pub fn relay_error_message(status: u16, body: &str) -> String {
    serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|v| v["message"].as_str().map(str::to_string))
        .unwrap_or_else(|| format!("relay answered HTTP {status}"))
}

/// What `register` persists into the trigger's own `extra`. `url` (`register`'s own echo of the
/// public sender-facing URL) isn't kept here — see [`public_url`], which reconstructs it the same
/// way the relay built it, from `relay` + `relay_endpoint`, so there's no fourth `extra` key for
/// it — only `relay`, `relay_retention`, and `relay_endpoint`.
#[derive(Debug, Clone)]
pub struct Registered {
    pub endpoint_id: String,
}

/// `POST /v1/endpoints`: register this node's sealing key against `relay_url`. Not best-effort —
/// see this module's own doc comment.
///
/// # Errors
/// A short, user-facing reason: a transport failure, a non-2xx relay response, or one that
/// didn't answer the shape the register response promises.
pub async fn register(
    relay_url: &str,
    identity: &RelayIdentity,
    retention_secs: Option<u64>,
    licence_code: Option<&str>,
) -> Result<Registered, String> {
    let mut body = serde_json::json!({
        "sealing_key": base64::engine::general_purpose::STANDARD.encode(identity.sealing_public()),
    });
    if let Some(secs) = retention_secs {
        body["retention_secs"] = serde_json::json!(secs);
    }
    if let Some(code) = licence_code.filter(|c| !c.is_empty()) {
        body["licence_code"] = serde_json::json!(code);
    }
    let body = serde_json::to_vec(&body).map_err(|e| e.to_string())?;

    let path = "/v1/endpoints";
    let url = format!("{}{path}", relay_url.trim_end_matches('/'));
    let mut req = http_client()
        .post(&url)
        .header("content-type", "application/json");
    for (k, v) in signed_headers(identity, "POST", path, &body) {
        req = req.header(k, v);
    }
    let resp = req
        .body(body)
        .send()
        .await
        .map_err(|e| format!("could not reach {relay_url}: {e}"))?;
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(relay_error_message(status.as_u16(), &text));
    }
    let v: serde_json::Value = serde_json::from_str(&text)
        .map_err(|_| format!("{relay_url} answered a body that wasn't JSON"))?;
    let endpoint_id = v["endpoint_id"]
        .as_str()
        .ok_or_else(|| format!("{relay_url}'s register response had no endpoint_id"))?
        .to_string();
    Ok(Registered { endpoint_id })
}

/// `DELETE /v1/endpoints/<id>`, best-effort — a relay delete that never lands costs nothing
/// beyond retention's own clock, the same stance a fleet grant transfer takes toward its own
/// failure mode: "a link that refuses, not a transfer that failed." Never propagates a failure;
/// only logs it. Run this through [`Cleanup`], not directly — see its own doc comment for why a
/// bare `tokio::spawn` of this is wrong for a one-shot caller.
async fn delete_best_effort(relay_url: String, identity: Arc<RelayIdentity>, endpoint_id: String) {
    let path = format!("/v1/endpoints/{endpoint_id}");
    let url = format!("{}{path}", relay_url.trim_end_matches('/'));
    let mut req = http_client().delete(&url);
    for (k, v) in signed_headers(&identity, "DELETE", &path, b"") {
        req = req.header(k, v);
    }
    match req.send().await {
        Ok(resp)
            if resp.status().is_success() || resp.status() == reqwest::StatusCode::NOT_FOUND => {}
        Ok(resp) => {
            debug!(%relay_url, %endpoint_id, status = %resp.status(), "best-effort relay delete answered non-2xx")
        }
        Err(error) => {
            debug!(%relay_url, %endpoint_id, %error, "best-effort relay delete could not reach the relay")
        }
    }
}

/// Fire-and-forget: spawn the best-effort delete, never awaited by the caller — right for a
/// sync context in a long-running process (`adi-app`'s own `delete_trigger`, outside any save),
/// which stays up long after this returns to let the spawned task actually run. [`Cleanup::Spawn`]
/// is this same call, for an async caller that also has a [`Cleanup::Await`] mode to choose.
pub fn spawn_delete_best_effort(
    relay_url: String,
    identity: Arc<RelayIdentity>,
    endpoint_id: String,
) {
    tokio::spawn(delete_best_effort(relay_url, identity, endpoint_id));
}

/// How [`register_on_save`] should run a best-effort relay delete — the two shapes its two real
/// callers need, and the reason there are two: a bare `tokio::spawn` of an async block that is
/// never awaited is **not** reliably seen through by a `tokio::Runtime` dropped right after, as
/// the obvious fix once looked like it should be — confirmed by hand (a task spawned just before
/// `block_on` returns, sleeping 200ms, left no trace of having run at all once the temporary
/// `Runtime` `Builder::new_multi_thread()...build()` produces was dropped). A `Runtime::drop`
/// only waits out tasks already making progress on a worker, not one that never got polled.
#[derive(Debug, Clone, Copy)]
pub enum Cleanup {
    /// Fire-and-forget: spawned, never awaited by the caller, so a save or delete never waits on
    /// a relay. Right for `adi-app`, whose process stays up long after this call returns to let
    /// the spawned task actually run.
    Spawn,
    /// Awaited before returning. Right for a one-shot caller (`adi-mono triggers save`) with no
    /// process left around afterward for a spawned task to finish in — "best-effort" still means
    /// a failure here is swallowed, never propagated, just not dispatched-and-abandoned.
    Await,
}

impl Cleanup {
    async fn run(self, relay_url: String, identity: Arc<RelayIdentity>, endpoint_id: String) {
        match self {
            Cleanup::Spawn => spawn_delete_best_effort(relay_url, identity, endpoint_id),
            Cleanup::Await => delete_best_effort(relay_url, identity, endpoint_id).await,
        }
    }
}

/// Whether, and how, a save needs to touch the relay before anything is persisted. Operates on
/// the manifest's `extra` map directly (not `adi_triggers::Trigger`/`TriggerManifest`) so this
/// crate never has to depend on the trigger-store crate for it.
#[derive(Debug)]
pub enum Plan {
    /// `relay` is unchanged from the stored trigger (or there is none), or the trigger is new
    /// with no `relay` set at all — nothing to do.
    Unchanged,
    /// `relay` is newly set, or changed to a different relay: register against `url` before the
    /// save lands. If the trigger previously had a *different* live endpoint, it is deregistered
    /// (best effort) once the new registration succeeds.
    Register { url: String },
    /// `relay` was cleared (the trigger no longer names one) and the stored trigger had a live
    /// endpoint: deregister it (best effort); nothing to register.
    Deregister { url: String, endpoint_id: String },
}

#[must_use]
pub fn plan(
    previous_extra: Option<&BTreeMap<String, String>>,
    incoming_extra: &BTreeMap<String, String>,
) -> Plan {
    let new_relay = incoming_extra.get("relay").filter(|s| !s.is_empty());
    let old_relay = previous_extra
        .and_then(|e| e.get("relay"))
        .filter(|s| !s.is_empty());
    let old_endpoint = previous_extra
        .and_then(|e| e.get("relay_endpoint"))
        .filter(|s| !s.is_empty());

    match new_relay {
        Some(new) if Some(new) == old_relay && old_endpoint.is_some() => Plan::Unchanged,
        Some(new) => match resolve_url(new) {
            Some(url) => Plan::Register { url },
            // An unknown relay name: nothing this module can register against. Left for the
            // panel/CLI's own validation to have caught; treated as "no relay" here rather than
            // refusing the whole save over it.
            None => Plan::Unchanged,
        },
        None => match (old_relay, old_endpoint) {
            (Some(old), Some(endpoint_id)) => match resolve_url(old) {
                Some(url) => Plan::Deregister {
                    url,
                    endpoint_id: endpoint_id.clone(),
                },
                None => Plan::Unchanged,
            },
            _ => Plan::Unchanged,
        },
    }
}

/// Whatever a `Plan::Unchanged` save still owes `manifest_extra`: carrying `relay_endpoint`
/// forward from `previous_extra` when the incoming manifest doesn't already have one.
///
/// The panel's own edit form always round-trips every relay key through its own form state (it
/// loads the existing trigger's `extra` and only overwrites the fields the user actually
/// touched), so a save it submits already carries `relay_endpoint` forward on its own. Nothing
/// requires every caller to — `adi-mono triggers save` builds its manifest from scratch out of
/// this invocation's own `--extra` flags, which never include `relay_endpoint` (nobody sets it
/// by hand) — so without this, an unrelated re-save of a relay-backed trigger would silently
/// drop its live registration. Safe to call unconditionally: a no-op for a trigger with no
/// relay, or one whose incoming manifest already names its own `relay_endpoint`.
pub fn keep_unchanged_endpoint(
    previous_extra: Option<&BTreeMap<String, String>>,
    manifest_extra: &mut BTreeMap<String, String>,
) {
    if let Some(endpoint) = previous_extra
        .and_then(|e| e.get("relay_endpoint"))
        .filter(|s| !s.is_empty())
    {
        manifest_extra
            .entry("relay_endpoint".to_string())
            .or_insert_with(|| endpoint.clone());
    }
}

/// Register (or re-register, or deregister) `manifest_extra`'s relay against the relay it names,
/// before anything persists. Mutates `manifest_extra` in place — inserts `relay_endpoint` on a
/// fresh registration, removes it when the relay is cleared — but never touches a store; the
/// caller still owns persisting the manifest once this returns `Ok`.
///
/// Callers that want to skip loading `identity` for a save with nothing to do should check
/// [`plan`] themselves first; this recomputes it, which is cheap, but loading the node's
/// keypair (and minting one on first use) is not something every trigger save should pay for.
///
/// # Errors
/// The relay's own short, user-facing reason a registration failed — a transport failure, a
/// non-2xx response, or one that didn't answer the shape [`register`] promises. A failed
/// deregistration never surfaces here: it's best-effort, same as a direct [`Plan::Deregister`].
pub async fn register_on_save(
    identity: &Arc<RelayIdentity>,
    previous_extra: Option<&BTreeMap<String, String>>,
    manifest_extra: &mut BTreeMap<String, String>,
    cleanup: Cleanup,
) -> Result<(), String> {
    match plan(previous_extra, manifest_extra) {
        Plan::Unchanged => Ok(()),
        Plan::Deregister { url, endpoint_id } => {
            manifest_extra.remove("relay_endpoint");
            cleanup.run(url, Arc::clone(identity), endpoint_id).await;
            Ok(())
        }
        Plan::Register { url } => {
            let retention = manifest_extra
                .get("relay_retention")
                .and_then(|s| s.parse::<u64>().ok());
            let licence_code = manifest_extra.get("licence_code").cloned();
            let registered = register(&url, identity, retention, licence_code.as_deref()).await?;
            manifest_extra.insert("relay_endpoint".to_string(), registered.endpoint_id);
            // Changing relays leaves the old endpoint behind — best-effort clean it up now that
            // the new one is live.
            if let Some(prev) = previous_extra {
                let old_relay = prev.get("relay").filter(|s| !s.is_empty());
                let old_endpoint = prev.get("relay_endpoint").filter(|s| !s.is_empty());
                if let (Some(old_relay), Some(old_endpoint)) = (old_relay, old_endpoint)
                    && Some(old_relay.as_str()) != manifest_extra.get("relay").map(String::as_str)
                    && let Some(old_url) = resolve_url(old_relay)
                {
                    cleanup
                        .run(old_url, Arc::clone(identity), old_endpoint.clone())
                        .await;
                }
            }
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn extra(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn resolve_url_covers_the_four_hosted_relays_and_custom() {
        assert_eq!(
            resolve_url("public-eu"),
            Some("https://eu.webhooks.withadi.dev".to_string())
        );
        assert_eq!(
            resolve_url("public-us"),
            Some("https://us.webhooks.withadi.dev".to_string())
        );
        assert_eq!(
            resolve_url("enterprise-eu"),
            Some("https://eu.ent.webhooks.withadi.dev".to_string())
        );
        assert_eq!(
            resolve_url("enterprise-us"),
            Some("https://us.ent.webhooks.withadi.dev".to_string())
        );
        assert_eq!(
            resolve_url("custom:https://relay.example.com/"),
            Some("https://relay.example.com".to_string())
        );
        assert_eq!(
            resolve_url("custom:"),
            None,
            "a blank custom URL names nothing"
        );
        assert_eq!(resolve_url("bogus"), None);
    }

    #[test]
    fn public_url_joins_the_resolved_base_and_the_endpoint() {
        assert_eq!(
            public_url("public-eu", "wh_abc"),
            Some("https://eu.webhooks.withadi.dev/h/wh_abc".to_string())
        );
        assert_eq!(public_url("bogus", "wh_abc"), None);
    }

    #[test]
    fn a_fresh_relay_assignment_plans_to_register() {
        let incoming = extra(&[("relay", "public-eu")]);
        assert!(matches!(plan(None, &incoming), Plan::Register { .. }));
    }

    #[test]
    fn an_unchanged_relay_with_a_live_endpoint_plans_nothing() {
        let previous = extra(&[("relay", "public-eu"), ("relay_endpoint", "wh_abc")]);
        let incoming = extra(&[("relay", "public-eu"), ("relay_endpoint", "wh_abc")]);
        assert!(matches!(plan(Some(&previous), &incoming), Plan::Unchanged));
    }

    #[test]
    fn changing_the_relay_plans_to_register_against_the_new_one() {
        let previous = extra(&[("relay", "public-eu"), ("relay_endpoint", "wh_abc")]);
        let incoming = extra(&[("relay", "public-us")]);
        match plan(Some(&previous), &incoming) {
            Plan::Register { url } => assert_eq!(url, "https://us.webhooks.withadi.dev"),
            Plan::Deregister { .. } | Plan::Unchanged => panic!("expected Register"),
        }
    }

    #[test]
    fn clearing_the_relay_plans_to_deregister_the_old_endpoint() {
        let previous = extra(&[("relay", "public-eu"), ("relay_endpoint", "wh_abc")]);
        let incoming = extra(&[]);
        match plan(Some(&previous), &incoming) {
            Plan::Deregister { url, endpoint_id } => {
                assert_eq!(url, "https://eu.webhooks.withadi.dev");
                assert_eq!(endpoint_id, "wh_abc");
            }
            Plan::Register { .. } | Plan::Unchanged => panic!("expected Deregister"),
        }
    }

    #[test]
    fn a_relay_with_no_prior_endpoint_still_plans_to_register() {
        // A previous save named a relay but, for whatever reason, never got a live endpoint
        // (e.g. this trigger's extra was set by hand, or by the CLI before ADI-MONO-227) — the
        // same relay named again must still register, not be read as "unchanged".
        let previous = extra(&[("relay", "public-eu")]);
        let incoming = extra(&[("relay", "public-eu")]);
        assert!(matches!(
            plan(Some(&previous), &incoming),
            Plan::Register { .. }
        ));
    }

    /// The exact bug ADI-MONO-227 fixed: a caller that builds its save from scratch (the CLI's
    /// own `--extra` flags) never includes `relay_endpoint` — without carrying it forward, an
    /// unrelated re-save of the same relay would silently drop the live registration.
    #[test]
    fn keep_unchanged_endpoint_carries_the_endpoint_forward_when_the_incoming_manifest_lacks_one() {
        let previous = extra(&[("relay", "public-eu"), ("relay_endpoint", "wh_kept")]);
        let mut incoming = extra(&[("relay", "public-eu")]);
        assert!(matches!(plan(Some(&previous), &incoming), Plan::Unchanged));
        keep_unchanged_endpoint(Some(&previous), &mut incoming);
        assert_eq!(incoming.get("relay_endpoint"), Some(&"wh_kept".to_string()));
    }

    #[test]
    fn keep_unchanged_endpoint_is_a_no_op_without_a_previous_relay() {
        let mut incoming = extra(&[]);
        keep_unchanged_endpoint(None, &mut incoming);
        assert!(incoming.is_empty());
    }

    #[test]
    fn an_unknown_relay_name_plans_nothing_rather_than_erroring() {
        let incoming = extra(&[("relay", "not-a-real-relay")]);
        assert!(matches!(plan(None, &incoming), Plan::Unchanged));
    }

    /// A bare HTTP/1.1 response with a correct `Content-Length`, written to `stream`.
    async fn respond(stream: &mut tokio::net::TcpStream, status: &str, body: &str) {
        use tokio::io::AsyncWriteExt as _;
        stream
            .write_all(
                format!(
                    "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                )
                .as_bytes(),
            )
            .await
            .unwrap();
    }

    /// `register_on_save` against a fake relay standing in for the real one: a registration
    /// failure must propagate without mutating `manifest_extra`, so the caller is free to treat
    /// it as a failed save.
    #[tokio::test]
    async fn register_on_save_propagates_a_registration_failure_without_mutating_extra() {
        let identity = scratch_identity("register-on-save-fail");
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            use tokio::io::AsyncReadExt as _;
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut buf = vec![0u8; 8192];
            let _ = stream.read(&mut buf).await.unwrap();
            respond(
                &mut stream,
                "500 Internal Server Error",
                "{\"message\":\"relay is down\"}",
            )
            .await;
        });

        let mut manifest_extra = extra(&[]);
        manifest_extra.insert("relay".to_string(), format!("custom:http://{addr}"));
        let result = register_on_save(&identity, None, &mut manifest_extra, Cleanup::Await).await;

        server.await.unwrap();
        assert_eq!(result, Err("relay is down".to_string()));
        assert!(
            !manifest_extra.contains_key("relay_endpoint"),
            "a failed registration must not leave a relay_endpoint behind"
        );
    }

    /// The success path end to end against a fake relay: `relay_endpoint` lands in
    /// `manifest_extra`, and switching away from a previously live endpoint queues its
    /// best-effort delete (observed here as a second request the fake relay answers).
    #[tokio::test]
    async fn register_on_save_mints_an_endpoint_and_best_effort_deletes_the_old_one() {
        let identity = scratch_identity("register-on-save-ok");
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            use tokio::io::AsyncReadExt as _;
            // 1. POST /v1/endpoints — the fresh registration.
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut buf = vec![0u8; 8192];
            let n = stream.read(&mut buf).await.unwrap();
            let request = String::from_utf8_lossy(&buf[..n]).to_string();
            assert!(request.starts_with("POST /v1/endpoints"));
            respond(&mut stream, "200 OK", "{\"endpoint_id\":\"wh_fresh\"}").await;
            drop(stream);

            // 2. DELETE /v1/endpoints/wh_old — the best-effort cleanup of the endpoint this save
            // replaced, awaited before `register_on_save` returns (`Cleanup::Await`, below).
            let (mut stream, _) = listener.accept().await.unwrap();
            let n = stream.read(&mut buf).await.unwrap();
            let request = String::from_utf8_lossy(&buf[..n]).to_string();
            assert!(request.starts_with("DELETE /v1/endpoints/wh_old"));
            respond(&mut stream, "200 OK", "").await;
        });

        // Both relays must resolve to the fake listener — only its *string* has to change for
        // `plan` to see a switch (a trailing slash does nothing to the resolved URL, but is
        // enough to make the two `relay` values compare unequal).
        let previous_extra = extra(&[
            ("relay", &format!("custom:http://{addr}/")),
            ("relay_endpoint", "wh_old"),
        ]);
        let mut manifest_extra = extra(&[("relay", &format!("custom:http://{addr}"))]);
        // `Cleanup::Await`, the CLI's own mode — the one where the delete reaching the fake
        // relay at all is not a question of the surrounding runtime happening to keep running
        // long enough to poll an abandoned spawned task (see `Cleanup`'s own doc comment).
        register_on_save(
            &identity,
            Some(&previous_extra),
            &mut manifest_extra,
            Cleanup::Await,
        )
        .await
        .expect("registration succeeds");

        assert_eq!(
            manifest_extra.get("relay_endpoint"),
            Some(&"wh_fresh".to_string())
        );
        server.await.unwrap();
    }

    /// `Cleanup::Spawn` fires the same delete without the caller awaiting it — right for
    /// `adi-app`, which stays up after `register_on_save` returns. Proven here by awaiting the
    /// fake relay's own second `accept` on the *same* runtime afterward, which is exactly the
    /// condition `Cleanup::Spawn`'s doc comment says a one-shot `block_on` caller cannot promise.
    #[tokio::test]
    async fn cleanup_spawn_fires_the_delete_without_the_caller_awaiting_it() {
        let identity = scratch_identity("register-on-save-spawn");
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            use tokio::io::AsyncReadExt as _;
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut buf = vec![0u8; 8192];
            let n = stream.read(&mut buf).await.unwrap();
            let request = String::from_utf8_lossy(&buf[..n]).to_string();
            assert!(request.starts_with("POST /v1/endpoints"));
            respond(&mut stream, "200 OK", "{\"endpoint_id\":\"wh_fresh2\"}").await;
            drop(stream);

            let (mut stream, _) = listener.accept().await.unwrap();
            let n = stream.read(&mut buf).await.unwrap();
            let request = String::from_utf8_lossy(&buf[..n]).to_string();
            assert!(request.starts_with("DELETE /v1/endpoints/wh_old2"));
            respond(&mut stream, "200 OK", "").await;
        });

        let previous_extra = extra(&[
            ("relay", &format!("custom:http://{addr}/")),
            ("relay_endpoint", "wh_old2"),
        ]);
        let mut manifest_extra = extra(&[("relay", &format!("custom:http://{addr}"))]);
        register_on_save(
            &identity,
            Some(&previous_extra),
            &mut manifest_extra,
            Cleanup::Spawn,
        )
        .await
        .expect("registration succeeds");

        server.await.unwrap();
    }

    fn scratch_identity(tag: &str) -> Arc<RelayIdentity> {
        let root = std::env::temp_dir().join(format!(
            "adi-relay-register-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id(),
        ));
        let _ = std::fs::remove_dir_all(&root);
        Arc::new(
            crate::keys::load_or_create_in(&adi_config::Config::with_root(root))
                .expect("mint a scratch identity"),
        )
    }
}
