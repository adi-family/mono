//! Node side of the webhook relay (ADI-MONO-223): this node's
//! keypair, the register/deregister calls a trigger save or delete makes against a relay, and
//! the built-in poller that pulls queued records and replays them into this node's own
//! `/api/hooks/<name>`.
//!
//! **One keypair for the whole node**, loaded lazily by [`identity`] —
//! [`adi_relay::keys::load_or_create`] under the hood, the same `load_or_create` shape
//! `adi-mesh::identity`/`devicekey` already use. Every relay-backed trigger on the node shares
//! it; the relay tells them apart by `endpoint_id`, not by key.
//!
//! **Register-on-save is a property of this API layer, not of the store.** Unlike the
//! background-trigger [`adi_triggers::Supervisor`], which re-reads *whatever is on disk* on a
//! timer so any writer (API, CLI, a hand-edited TOML) steers it alike, a relay registration is a
//! network call with a real failure mode the caller needs to see *before* anything is
//! persisted: a failed registration is meant to be a failed save, not a trigger left
//! half-configured. That
//! only works for a caller waiting on the save to answer — which is what `/api/triggers/save`'s
//! HTTP response is and a raw `Triggers::save` call (the CLI, `adi-cli/src/triggers.rs`) is not.
//! A CLI `--extra relay=…` therefore sets the field without registering anything; the task names
//! no CLI verb that would, and the panel is this feature's primary surface.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use adi_relay::keys::RelayIdentity;
use adi_relay::seal::Sealed;
use adi_triggers::{Supervisor, Trigger, TriggerManifest, Triggers};
use adi_webapp_api::handlers::{self, Response};
use base64::Engine as _;
use tracing::{debug, warn};

/// The four hosted relays the task names, plus `custom:<url>` for a
/// self-host operator's own instance. `None` for anything else — an unknown name is left for the
/// panel/CLI's own validation to catch, not guessed at here.
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

/// This node's relay keypair, generated on first use and shared by every relay-backed
/// trigger on the node.
///
/// # Errors
/// Whatever [`adi_relay::keys::load_or_create`] can fail with: an existing key file that isn't
/// exactly 32 bytes, or a store I/O error.
pub fn identity() -> anyhow::Result<RelayIdentity> {
    adi_relay::keys::load_or_create()
}

/// reqwest 0.13 is taken workspace-wide with `rustls-no-provider`; `adi-app`'s own `main` installs
/// the provider once at startup, but a unit test building a client directly never runs `main` —
/// see `adi-marketplace/src/fetch.rs::get_https` for the same line, idempotent by design.
fn ensure_crypto_provider() {
    rustls::crypto::ring::default_provider().install_default().ok();
}

fn http_client() -> reqwest::Client {
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
fn signed_headers(identity: &RelayIdentity, method: &str, path_and_query: &str, body: &[u8]) -> Vec<(String, String)> {
    let signed = adi_relay::sign::sign(
        &identity.signing,
        method,
        path_and_query,
        now_secs(),
        &adi_relay::sign::generate_nonce(),
        body,
    );
    vec![
        (adi_relay::sign::HEADER_KEY.to_string(), signed.key_line),
        (adi_relay::sign::HEADER_TIMESTAMP.to_string(), signed.timestamp.to_string()),
        (adi_relay::sign::HEADER_NONCE.to_string(), signed.nonce),
        (adi_relay::sign::HEADER_SIGNATURE.to_string(), signed.signature),
    ]
}

/// The `{ "error": "...", "message": "..." }` body every relay non-2xx answers with — the
/// message if it parses as that shape, else a generic one naming the status.
fn relay_error_message(status: u16, body: &str) -> String {
    serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|v| v["message"].as_str().map(str::to_string))
        .unwrap_or_else(|| format!("relay answered HTTP {status}"))
}

/// What `register` persists into the trigger's own `extra`. `url` (`register`'s own echo of the
/// public sender-facing URL) isn't kept here — the panel/CLI reconstruct it the same way the
/// relay built it, `{relay_url}/h/{endpoint_id}`, from `relay` + `relay_endpoint`, so there's no
/// fourth `extra` key for it — only `relay`, `relay_retention`, and `relay_endpoint`.
#[derive(Debug, Clone)]
pub struct Registered {
    pub endpoint_id: String,
}

/// `POST /v1/endpoints`: register this node's sealing key against `relay_url`. Not
/// best-effort — see this module's own doc comment.
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
    let mut req = http_client().post(&url).header("content-type", "application/json");
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
    let v: serde_json::Value =
        serde_json::from_str(&text).map_err(|_| format!("{relay_url} answered a body that wasn't JSON"))?;
    let endpoint_id = v["endpoint_id"]
        .as_str()
        .ok_or_else(|| format!("{relay_url}'s register response had no endpoint_id"))?
        .to_string();
    Ok(Registered { endpoint_id })
}

/// `DELETE /v1/endpoints/<id>`, best-effort — a relay delete that never lands costs
/// nothing beyond retention's own clock, the same stance a fleet grant transfer takes toward
/// its own failure mode: "a link that refuses, not a transfer that failed." Fire-and-forget: spawned,
/// never awaited by the caller, so disabling or deleting a trigger never waits on a relay.
pub fn spawn_delete_best_effort(relay_url: String, identity: Arc<RelayIdentity>, endpoint_id: String) {
    tokio::spawn(async move {
        let path = format!("/v1/endpoints/{endpoint_id}");
        let url = format!("{}{path}", relay_url.trim_end_matches('/'));
        let mut req = http_client().delete(&url);
        for (k, v) in signed_headers(&identity, "DELETE", &path, b"") {
            req = req.header(k, v);
        }
        match req.send().await {
            Ok(resp) if resp.status().is_success() || resp.status() == reqwest::StatusCode::NOT_FOUND => {}
            Ok(resp) => debug!(%relay_url, %endpoint_id, status = %resp.status(), "best-effort relay delete answered non-2xx"),
            Err(error) => debug!(%relay_url, %endpoint_id, %error, "best-effort relay delete could not reach the relay"),
        }
    });
}

/// One record off `GET /v1/pull`, decoded from the wire.
#[derive(Debug, Clone)]
struct PulledRecord {
    id: i64,
    endpoint_id: String,
    sealed: Sealed,
}

/// `GET /v1/pull`: long-polls server-side for up to the relay's own window, so this call
/// itself paces the poll loop — nothing extra to sleep between calls on success.
///
/// # Errors
/// A short, user-facing reason: a transport failure, a non-2xx relay response, or one that
/// didn't answer the shape the pull response promises.
async fn pull(relay_url: &str, identity: &RelayIdentity) -> Result<Vec<PulledRecord>, String> {
    let path = "/v1/pull";
    let url = format!("{}{path}", relay_url.trim_end_matches('/'));
    let mut req = http_client().get(&url);
    for (k, v) in signed_headers(identity, "GET", path, b"") {
        req = req.header(k, v);
    }
    let resp = req.send().await.map_err(|e| format!("could not reach {relay_url}: {e}"))?;
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(relay_error_message(status.as_u16(), &text));
    }
    let v: serde_json::Value =
        serde_json::from_str(&text).map_err(|_| format!("{relay_url} answered a body that wasn't JSON"))?;
    v["records"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .map(|r| {
            let id = r["id"].as_i64().ok_or("a pulled record had no id")?;
            let endpoint_id = r["endpoint_id"]
                .as_str()
                .ok_or("a pulled record had no endpoint_id")?
                .to_string();
            let sealed = Sealed::from_b64(
                r["sealed"]["ephemeral_pubkey"].as_str().unwrap_or_default(),
                r["sealed"]["nonce"].as_str().unwrap_or_default(),
                r["sealed"]["ciphertext"].as_str().unwrap_or_default(),
            )
            .map_err(|e| format!("a pulled record's seal didn't decode: {e}"))?;
            Ok(PulledRecord { id, endpoint_id, sealed })
        })
        .collect::<Result<Vec<_>, String>>()
}

/// The sealed envelope's own shape: the sender's request exactly as
/// `/h/<endpoint-id>` captured it.
#[derive(Debug, serde::Deserialize)]
struct SenderRequest {
    method: String,
    headers: HashMap<String, String>,
    query: String,
    body_b64: String,
}

/// Headers a request's own client sets for itself from the URL/body/connection it's actually
/// making — forwarding the sender's original copy of these would either be rejected outright or
/// lie about the loopback call that's really happening.
const HOP_BY_HOP: &[&str] = &["host", "content-length", "connection", "transfer-encoding"];

/// Open `record` with this node's sealing secret and replay it into the local loopback listener
/// — never an in-process call straight into `handlers::hook_trigger`, so a relay-delivered
/// hook and a directly-received one are guaranteed to pass through *exactly* the same gating
/// (`enabled`, `secret`, `trigger_on`), with no second copy of that logic to keep in step.
///
/// `trigger.extra["secret"]` rides as a query param on the replay — the node's own answer to the
/// local gate a direct caller would otherwise have had to supply, never sent by the external
/// sender (a sender can't sign with our key, so the sender side stays open in every mode).
async fn replay(loopback_base: &str, identity: &RelayIdentity, trigger: &Trigger, record: &PulledRecord) {
    let Ok(opened) = adi_relay::seal::open(&identity.sealing, &record.endpoint_id, record.id, &record.sealed) else {
        warn!(trigger = %trigger.name, endpoint_id = %record.endpoint_id, "a pulled record did not open with this node's sealing key — dropped");
        return;
    };
    let sender: SenderRequest = match serde_json::from_slice(&opened) {
        Ok(s) => s,
        Err(error) => {
            warn!(trigger = %trigger.name, %error, "a pulled record's opened body wasn't the expected shape — dropped");
            return;
        }
    };
    let body = base64::engine::general_purpose::STANDARD
        .decode(&sender.body_b64)
        .unwrap_or_default();

    let mut query = sender.query.clone();
    if let Some(secret) = trigger.manifest.extra.get("secret").filter(|s| !s.is_empty()) {
        if !query.is_empty() {
            query.push('&');
        }
        query.push_str("secret=");
        query.push_str(secret);
    }
    let url = format!(
        "{}/api/hooks/{}{}{}",
        loopback_base.trim_end_matches('/'),
        trigger.name,
        if query.is_empty() { "" } else { "?" },
        query
    );

    let method = sender.method.parse::<reqwest::Method>().unwrap_or(reqwest::Method::POST);
    let mut req = http_client().request(method, &url);
    for (k, v) in &sender.headers {
        if !HOP_BY_HOP.contains(&k.to_ascii_lowercase().as_str()) {
            req = req.header(k, v);
        }
    }
    match req.body(body).send().await {
        Ok(resp) if resp.status().is_success() => {
            debug!(trigger = %trigger.name, endpoint_id = %record.endpoint_id, "replayed a relay-delivered webhook");
        }
        Ok(resp) => warn!(trigger = %trigger.name, status = %resp.status(), "the local replay of a relay-delivered webhook was refused"),
        Err(error) => warn!(trigger = %trigger.name, %error, "could not reach this node's own loopback listener to replay a webhook"),
    }
}

/// How often the supervisor rescans the trigger store for the set of relays currently in use —
/// cheap (one `triggers.list()`) and also the worst-case delay before a freshly registered
/// trigger's relay starts being polled.
const RESCAN: Duration = Duration::from_secs(15);

const INITIAL_BACKOFF: Duration = Duration::from_secs(1);
const MAX_BACKOFF: Duration = Duration::from_secs(30);

/// Start the webhook relay poller: one `tokio::spawn`'d loop per distinct relay in use,
/// beside every other background loop in this crate (`prober::start`, `event_log::start`,
/// `lease_worker::start`). `loopback_base` must always be `http://127.0.0.1:<this boot's own
/// port>` — never the address a peer would dial this node at, the same reasoning `main`'s own
/// `agents.with_lease_url` gives for its loopback-only BYO-credentials lease URL.
pub fn start(triggers: Triggers, loopback_base: String) {
    tokio::spawn(supervise(triggers, loopback_base));
}

async fn supervise(triggers: Triggers, loopback_base: String) {
    let identity = match identity() {
        Ok(id) => Arc::new(id),
        Err(error) => {
            warn!(%error, "could not load this node's relay keypair; the webhook relay poller is disabled for this boot");
            return;
        }
    };
    let mut running: std::collections::HashSet<String> = std::collections::HashSet::new();
    loop {
        for url in relays_in_use(&triggers) {
            if running.insert(url.clone()) {
                tokio::spawn(poll_loop(url, triggers.clone(), loopback_base.clone(), Arc::clone(&identity)));
            }
        }
        tokio::time::sleep(RESCAN).await;
    }
}

/// Every distinct relay URL an enabled, registered webhook trigger names. A trigger counts only
/// once it has a live `relay_endpoint`: disabling or deleting one deregisters it (best effort),
/// so an unregistered relay has nothing queued for this node to pull in the first place.
fn relays_in_use(triggers: &Triggers) -> std::collections::HashSet<String> {
    triggers
        .list()
        .unwrap_or_default()
        .into_iter()
        .filter(|t| t.manifest.kind == adi_triggers::KIND_WEBHOOK && t.manifest.enabled)
        .filter(|t| t.manifest.extra.get("relay_endpoint").is_some_and(|s| !s.is_empty()))
        .filter_map(|t| t.manifest.extra.get("relay").and_then(|r| resolve_url(r)))
        .collect()
}

async fn poll_loop(url: String, triggers: Triggers, loopback_base: String, identity: Arc<RelayIdentity>) {
    let mut backoff = INITIAL_BACKOFF;
    loop {
        match pull(&url, &identity).await {
            Ok(records) => {
                backoff = INITIAL_BACKOFF;
                if records.is_empty() {
                    continue;
                }
                let Ok(all) = triggers.list() else { continue };
                for record in &records {
                    let Some(trigger) = all.iter().find(|t| {
                        t.manifest.extra.get("relay_endpoint").map(String::as_str) == Some(record.endpoint_id.as_str())
                    }) else {
                        debug!(endpoint_id = %record.endpoint_id, "pulled a record for an endpoint no local trigger names any more — dropped");
                        continue;
                    };
                    replay(&loopback_base, &identity, trigger, record).await;
                }
            }
            Err(error) => {
                debug!(%url, %error, "the webhook relay poller could not reach a relay; retrying with backoff");
                tokio::time::sleep(backoff).await;
                backoff = (backoff * 2).min(MAX_BACKOFF);
            }
        }
    }
}

// ---- register-on-save / deregister-on-delete, the API-layer half ----------------------

/// Whether, and how, a save needs to touch the relay before anything is persisted.
enum Plan {
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

fn plan(previous: Option<&Trigger>, incoming: &TriggerManifest) -> Plan {
    let new_relay = incoming.extra.get("relay").filter(|s| !s.is_empty());
    let old_relay = previous.and_then(|t| t.manifest.extra.get("relay")).filter(|s| !s.is_empty());
    let old_endpoint = previous.and_then(|t| t.manifest.extra.get("relay_endpoint")).filter(|s| !s.is_empty());

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

/// `POST /api/triggers/save`, relay-aware: decide whether this save needs to register (or
/// re-register) against a relay *before* writing anything persists, so a registration failure is
/// a failed save — never a trigger left half-configured with a `relay` nobody answered for.
pub async fn save_trigger(triggers: &Triggers, supervisor: &Supervisor, body: &[u8]) -> Response {
    let (name, mut manifest) = match handlers::parse_save_trigger(body) {
        Ok(parsed) => parsed,
        Err(resp) => return resp,
    };
    let previous = triggers.get(&name).ok().flatten();

    match plan(previous.as_ref(), &manifest) {
        Plan::Unchanged => {}
        Plan::Deregister { url, endpoint_id } => {
            manifest.extra.remove("relay_endpoint");
            if let Ok(id) = identity() {
                spawn_delete_best_effort(url, Arc::new(id), endpoint_id);
            }
        }
        Plan::Register { url } => {
            let id = match identity() {
                Ok(id) => id,
                Err(error) => {
                    return handlers::error(500, &format!("could not load this node's relay keypair: {error}"));
                }
            };
            let retention = manifest.extra.get("relay_retention").and_then(|s| s.parse::<u64>().ok());
            let licence_code = manifest.extra.get("licence_code").map(String::as_str);
            match register(&url, &id, retention, licence_code).await {
                Ok(registered) => {
                    manifest.extra.insert("relay_endpoint".to_string(), registered.endpoint_id);
                }
                Err(error) => {
                    return handlers::error(502, &format!("registering against the relay failed: {error}"));
                }
            }
            // Changing relays leaves the old endpoint behind — best-effort clean it up now that
            // the new one is live.
            if let Some(prev) = &previous {
                let old_relay = prev.manifest.extra.get("relay").filter(|s| !s.is_empty());
                let old_endpoint = prev.manifest.extra.get("relay_endpoint").filter(|s| !s.is_empty());
                if let (Some(old_relay), Some(old_endpoint)) = (old_relay, old_endpoint)
                    && Some(old_relay.as_str()) != manifest.extra.get("relay").map(String::as_str)
                    && let Some(old_url) = resolve_url(old_relay)
                {
                    spawn_delete_best_effort(old_url, Arc::new(id), old_endpoint.clone());
                }
            }
        }
    }
    handlers::commit_trigger(triggers, supervisor, &name, manifest)
}

/// `POST /api/triggers/delete`, relay-aware: deregister the trigger's live endpoint (best
/// effort) before deleting its definition. Not actually async — the best-effort delete is a
/// fire-and-forget spawn, never awaited here — but called from `async_route` beside
/// [`save_trigger`], so it takes the same shape.
pub fn delete_trigger(triggers: &Triggers, supervisor: &Supervisor, body: &[u8]) -> Response {
    let name = match handlers::parse_trigger_ref(body) {
        Ok(name) => name,
        Err(resp) => return resp,
    };
    if let Ok(Some(trigger)) = triggers.get(&name) {
        let relay = trigger.manifest.extra.get("relay").filter(|s| !s.is_empty());
        let endpoint = trigger.manifest.extra.get("relay_endpoint").filter(|s| !s.is_empty());
        if let (Some(relay), Some(endpoint)) = (relay, endpoint)
            && let Some(url) = resolve_url(relay)
            && let Ok(id) = identity()
        {
            spawn_delete_best_effort(url, Arc::new(id), endpoint.clone());
        }
    }
    handlers::commit_delete_trigger(triggers, supervisor, &name)
}

/// `GET /api/triggers/relay-key`: this node's webhook-relay public signing key, for the
/// panel's own "copy public key" button and the CLI's `triggers relay-key`'s own HTTP twin.
#[must_use]
pub fn relay_key() -> Response {
    match identity() {
        Ok(id) => handlers::ok_json(&adi_webapp_api::types::RelayKey {
            public_key: id.public_key_line(),
        }),
        Err(error) => handlers::error(500, &format!("could not load this node's relay keypair: {error}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest_with(extra: &[(&str, &str)]) -> TriggerManifest {
        TriggerManifest {
            kind: adi_triggers::KIND_WEBHOOK.to_string(),
            extra: extra.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect(),
            ..TriggerManifest::default()
        }
    }

    fn trigger_with(extra: &[(&str, &str)]) -> Trigger {
        Trigger {
            name: "hook".to_string(),
            manifest: manifest_with(extra),
        }
    }

    #[test]
    fn resolve_url_covers_the_four_hosted_relays_and_custom() {
        assert_eq!(resolve_url("public-eu"), Some("https://eu.webhooks.withadi.dev".to_string()));
        assert_eq!(resolve_url("public-us"), Some("https://us.webhooks.withadi.dev".to_string()));
        assert_eq!(resolve_url("enterprise-eu"), Some("https://eu.ent.webhooks.withadi.dev".to_string()));
        assert_eq!(resolve_url("enterprise-us"), Some("https://us.ent.webhooks.withadi.dev".to_string()));
        assert_eq!(resolve_url("custom:https://relay.example.com/"), Some("https://relay.example.com".to_string()));
        assert_eq!(resolve_url("custom:"), None, "a blank custom URL names nothing");
        assert_eq!(resolve_url("bogus"), None);
    }

    #[test]
    fn a_fresh_relay_assignment_plans_to_register() {
        let incoming = manifest_with(&[("relay", "public-eu")]);
        assert!(matches!(plan(None, &incoming), Plan::Register { .. }));
    }

    #[test]
    fn an_unchanged_relay_with_a_live_endpoint_plans_nothing() {
        let previous = trigger_with(&[("relay", "public-eu"), ("relay_endpoint", "wh_abc")]);
        let incoming = manifest_with(&[("relay", "public-eu"), ("relay_endpoint", "wh_abc")]);
        assert!(matches!(plan(Some(&previous), &incoming), Plan::Unchanged));
    }

    #[test]
    fn changing_the_relay_plans_to_register_against_the_new_one() {
        let previous = trigger_with(&[("relay", "public-eu"), ("relay_endpoint", "wh_abc")]);
        let incoming = manifest_with(&[("relay", "public-us")]);
        match plan(Some(&previous), &incoming) {
            Plan::Register { url } => assert_eq!(url, "https://us.webhooks.withadi.dev"),
            Plan::Deregister { .. } | Plan::Unchanged => panic!("expected Register"),
        }
    }

    #[test]
    fn clearing_the_relay_plans_to_deregister_the_old_endpoint() {
        let previous = trigger_with(&[("relay", "public-eu"), ("relay_endpoint", "wh_abc")]);
        let incoming = manifest_with(&[]);
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
        // (e.g. this trigger's extra was set by hand or by the CLI, which does not register) —
        // the same relay named again must still register, not be read as "unchanged".
        let previous = trigger_with(&[("relay", "public-eu")]);
        let incoming = manifest_with(&[("relay", "public-eu")]);
        assert!(matches!(plan(Some(&previous), &incoming), Plan::Register { .. }));
    }

    #[test]
    fn an_unknown_relay_name_plans_nothing_rather_than_erroring() {
        let incoming = manifest_with(&[("relay", "not-a-real-relay")]);
        assert!(matches!(plan(None, &incoming), Plan::Unchanged));
    }

    fn scratch_identity(tag: &str) -> RelayIdentity {
        let root = std::env::temp_dir().join(format!(
            "adi-app-relay-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id(),
        ));
        let _ = std::fs::remove_dir_all(&root);
        adi_relay::keys::load_or_create_in(&adi_config::Config::with_root(root)).expect("mint a scratch identity")
    }

    /// `replay` end to end against a real loopback socket standing in for `adi-app`'s own
    /// listener: opens a record genuinely sealed to this identity's key, and the fake listener
    /// reads back the exact HTTP request it produced — method preserved, the sender's own
    /// `host` dropped (never forwarded), the trigger's `secret` appended as a query param
    /// nobody but this node ever sees, and the body round-tripped intact.
    #[tokio::test]
    async fn replay_opens_and_forwards_a_sealed_record_preserving_method_headers_and_secret() {
        let identity = scratch_identity("replay");
        let trigger = Trigger {
            name: "deploy-hook".to_string(),
            manifest: manifest_with(&[("secret", "shh")]),
        };
        let plaintext = serde_json::to_vec(&serde_json::json!({
            "method": "GET",
            "headers": {
                "x-hub-signature-256": "sha256=abc",
                "host": "eu.webhooks.withadi.dev",
            },
            "query": "foo=bar",
            "body_b64": base64::engine::general_purpose::STANDARD.encode(b"{\"a\":1}"),
        }))
        .unwrap();
        let sealed = adi_relay::seal::seal(&identity.sealing_public(), "wh_test", 7, &plaintext);
        let record = PulledRecord {
            id: 7,
            endpoint_id: "wh_test".to_string(),
            sealed,
        };

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let captured = tokio::spawn(async move {
            use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut buf = vec![0u8; 8192];
            let n = stream.read(&mut buf).await.unwrap();
            stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok")
                .await
                .unwrap();
            String::from_utf8_lossy(&buf[..n]).to_string()
        });

        replay(&format!("http://{addr}"), &identity, &trigger, &record).await;

        let request = captured.await.unwrap();
        let request_line = request.lines().next().unwrap();
        assert_eq!(request_line, "GET /api/hooks/deploy-hook?foo=bar&secret=shh HTTP/1.1");
        assert!(request.to_ascii_lowercase().contains("x-hub-signature-256: sha256=abc"));
        // The sender's own captured `host` header must never ride the replay — reqwest
        // still sets its own `Host: 127.0.0.1:<port>` for the request it's actually making, so
        // the assertion is on the *value*, not the header's bare absence.
        assert!(
            !request.to_ascii_lowercase().contains("eu.webhooks.withadi.dev"),
            "the sender's captured Host header must not ride the loopback replay: {request}"
        );
        assert!(request.ends_with("{\"a\":1}"), "the body round-trips intact: {request}");
    }
}
