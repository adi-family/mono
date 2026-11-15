//! The enterprise-mode licence check against the real `adi-licence-service` contract
//! (`agent:adi-agent/memory`, "ADI Enterprise beam wire contract v1") — not a guess:
//! `POST {service_url}/v1/verify`, the same request/response shape and the same
//! ed25519-over-canonical-JSON signature scheme the ADI Enterprise beam client itself uses.
//!
//! **One documented deviation**: that contract's `deployment_id` names an ADI Enterprise
//! control-plane deployment, persisted once in *its own* state store — a concept a webhook relay
//! endpoint has none of. This relay synthesizes [`LicenceClient::synthetic_deployment_id`] from
//! the registering key's fingerprint instead of reading one from the node, which preserves the
//! licence's per-deployment seat allowance for exactly the case that matters here: one node's
//! long-lived relay key (one pair per node) occupies one seat regardless of how many relay
//! regions it registers against, and a second node (a second fingerprint) occupies a second
//! seat, which is what `max_deployments` is for.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use base64::Engine as _;
use ed25519_dalek::{Signature, Verifier as _, VerifyingKey};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// `adi-enterprise-beam-wire-contract-v1`: "CLIENT ACCEPTANCE ... |`checked_at` - now| <= 300s".
const CHECKED_AT_SKEW_SECS: u64 = 300;

/// `adi-enterprise-beam-wire-contract-v1`: "SERVING VERDICTS are paid and trialing — nothing
/// else."
fn is_serving(status: &str) -> bool {
    matches!(status, "paid" | "trialing")
}

#[derive(Debug, Serialize)]
struct BeamRequest<'a> {
    protocol: u32,
    licence_code: &'a str,
    deployment_id: &'a str,
    nonce: &'a str,
    sent_at: u64,
}

#[derive(Debug, Deserialize)]
struct BeamResponse {
    verdict: Value,
    signature: String,
}

#[derive(Debug, Deserialize)]
struct VerdictFields {
    checked_at: i64,
    deployment_id: String,
    nonce: String,
    status: String,
}

/// What a decided, accepted beam says about one registering key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LicenceOutcome {
    pub valid: bool,
    pub checked_at: i64,
}

/// Every way a beam can fail to produce a trustworthy [`LicenceOutcome`] — deliberately not
/// folded into one variant, because the caller treats a transport failure very differently from
/// a decided-but-untrustworthy response: register refuses either way, but the daily recheck
/// sweep must not pause a licence over a network blip — see `sweep.rs`.
#[derive(Debug)]
pub enum LicenceError {
    /// Couldn't reach the service, or it answered a non-2xx status.
    Transport(String),
    /// The 2xx body wasn't the shape the contract promises.
    BadResponse(String),
    /// The signature doesn't verify against the configured public key.
    BadSignature,
    /// The verdict's `nonce` doesn't match the one this beam sent.
    NonceMismatch,
    /// The verdict's `deployment_id` doesn't match [`LicenceClient::synthetic_deployment_id`].
    DeploymentMismatch,
    /// `checked_at` is further than [`CHECKED_AT_SKEW_SECS`] from now.
    CheckedAtOutOfSkew,
}

impl std::fmt::Display for LicenceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Transport(e) => write!(f, "could not reach the licence service: {e}"),
            Self::BadResponse(e) => write!(f, "licence service response was not understood: {e}"),
            Self::BadSignature => write!(f, "licence service response did not verify"),
            Self::NonceMismatch => write!(f, "licence service verdict echoed the wrong nonce"),
            Self::DeploymentMismatch => write!(f, "licence service verdict echoed the wrong deployment_id"),
            Self::CheckedAtOutOfSkew => write!(f, "licence service verdict's checked_at is too far from now"),
        }
    }
}

impl std::error::Error for LicenceError {}

pub struct LicenceClient {
    http: reqwest::Client,
    service_url: String,
    public_key: VerifyingKey,
}

/// `VerifyingKey` has no `Debug`; print the service URL only, which is all this is ever useful
/// for (there's one per running instance).
impl std::fmt::Debug for LicenceClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LicenceClient")
            .field("service_url", &self.service_url)
            .finish_non_exhaustive()
    }
}

impl LicenceClient {
    /// `public_key_b64` is the service's ed25519 public key, raw 32 bytes, base64 — the value
    /// `relay.toml`'s `[licence] public_key` carries.
    ///
    /// # Errors
    /// `public_key_b64` isn't valid base64, isn't 32 bytes, or isn't a valid ed25519 point; or
    /// building the underlying HTTP client fails.
    pub fn new(service_url: String, public_key_b64: &str) -> anyhow::Result<Self> {
        let raw = base64::engine::general_purpose::STANDARD.decode(public_key_b64)?;
        let raw: [u8; 32] = raw
            .try_into()
            .map_err(|_| anyhow::anyhow!("licence public_key must be 32 bytes"))?;
        let public_key = VerifyingKey::from_bytes(&raw)?;
        Ok(Self::with_public_key(service_url, public_key))
    }

    /// Build directly from an already-parsed key — what tests use to stand in for "the licence
    /// service's" key without touching base64 or the network.
    #[must_use]
    pub fn with_public_key(service_url: String, public_key: VerifyingKey) -> Self {
        // reqwest 0.13 is taken workspace-wide with `rustls-no-provider`, so nobody installs a
        // crypto provider for us and the client panics on its first HTTPS request until somebody
        // does. Every client site in this tree opens with this line; it is idempotent — a second
        // install returns `Err`, which is why the result is dropped.
        rustls::crypto::ring::default_provider().install_default().ok();
        Self {
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(10))
                .build()
                .unwrap_or_default(),
            service_url,
            public_key,
        }
    }

    /// A synthetic stand-in for a real ADI Enterprise `deployment_id`.
    #[must_use]
    pub fn synthetic_deployment_id(owner_fingerprint: &str) -> String {
        format!("webhook-relay:{owner_fingerprint}")
    }

    /// Beam `licence_code` for `owner_fingerprint`, verify the response, and decide.
    ///
    /// # Errors
    /// See [`LicenceError`]'s variants.
    pub async fn verify(&self, licence_code: &str, owner_fingerprint: &str) -> Result<LicenceOutcome, LicenceError> {
        let deployment_id = Self::synthetic_deployment_id(owner_fingerprint);
        let nonce = adi_relay::sign::generate_nonce();
        let request = BeamRequest {
            protocol: 1,
            licence_code,
            deployment_id: &deployment_id,
            nonce: &nonce,
            sent_at: now_secs(),
        };
        let url = format!("{}/v1/verify", self.service_url.trim_end_matches('/'));
        let response = self
            .http
            .post(&url)
            .json(&request)
            .send()
            .await
            .map_err(|e| LicenceError::Transport(e.to_string()))?;
        if !response.status().is_success() {
            return Err(LicenceError::Transport(format!("HTTP {}", response.status())));
        }
        let body = response
            .bytes()
            .await
            .map_err(|e| LicenceError::Transport(e.to_string()))?;
        self.decide(&body, &nonce, &deployment_id, i64::try_from(now_secs()).unwrap_or(i64::MAX))
    }

    /// The pure half of [`Self::verify`]: everything after the bytes are already in hand —
    /// parsing, signature verification, and the three "CLIENT ACCEPTANCE" checks the beam
    /// contract names beyond the signature (nonce, `deployment_id`, `checked_at` skew). Kept
    /// separate so tests can exercise it without a real HTTP round trip.
    fn decide(
        &self,
        body: &[u8],
        expected_nonce: &str,
        expected_deployment_id: &str,
        now: i64,
    ) -> Result<LicenceOutcome, LicenceError> {
        let parsed: BeamResponse =
            serde_json::from_slice(body).map_err(|e| LicenceError::BadResponse(e.to_string()))?;

        // Canonical bytes: compact JSON, keys sorted ascending — `serde_json::Value`'s object map
        // is a `BTreeMap` in this workspace's default build (no `preserve_order` feature), so
        // `to_vec` already emits keys in that order; this is the same canonicalization
        // `crypto.ts::canonicalBytes` performs on the signing side.
        let canonical = serde_json::to_vec(&parsed.verdict).map_err(|e| LicenceError::BadResponse(e.to_string()))?;
        let signature_bytes = base64::engine::general_purpose::STANDARD
            .decode(&parsed.signature)
            .map_err(|_| LicenceError::BadSignature)?;
        let signature_bytes: [u8; 64] = signature_bytes.try_into().map_err(|_| LicenceError::BadSignature)?;
        let signature = Signature::from_bytes(&signature_bytes);
        self.public_key
            .verify(&canonical, &signature)
            .map_err(|_| LicenceError::BadSignature)?;

        let fields: VerdictFields =
            serde_json::from_value(parsed.verdict).map_err(|e| LicenceError::BadResponse(e.to_string()))?;
        if fields.nonce != expected_nonce {
            return Err(LicenceError::NonceMismatch);
        }
        if fields.deployment_id != expected_deployment_id {
            return Err(LicenceError::DeploymentMismatch);
        }
        if now.abs_diff(fields.checked_at) > CHECKED_AT_SKEW_SECS {
            return Err(LicenceError::CheckedAtOutOfSkew);
        }
        Ok(LicenceOutcome {
            valid: is_serving(&fields.status),
            checked_at: fields.checked_at,
        })
    }
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer as _, SigningKey};

    /// Build a signed `BeamResponse` body exactly as `adi-licence-service` would, for a given
    /// verdict's fields — the one piece of test fixture every case below shares.
    fn signed_body(signing: &SigningKey, verdict: &serde_json::Value) -> Vec<u8> {
        let canonical = serde_json::to_vec(verdict).unwrap();
        let signature: Signature = signing.sign(&canonical);
        let body = serde_json::json!({
            "protocol": 1,
            "verdict": verdict,
            "signature": base64::engine::general_purpose::STANDARD.encode(signature.to_bytes()),
        });
        serde_json::to_vec(&body).unwrap()
    }

    fn verdict(status: &str, nonce: &str, deployment_id: &str, checked_at: i64) -> serde_json::Value {
        serde_json::json!({
            "checked_at": checked_at,
            "company": "Acme GmbH",
            "deployment_id": deployment_id,
            "expires_at": null,
            "licence_code": "ADI-ENT-TEST",
            "max_stale": 86_400,
            "nonce": nonce,
            "recheck_after": 21_600,
            "status": status,
            "trial_ends_at": null,
        })
    }

    fn client(signing: &SigningKey) -> LicenceClient {
        LicenceClient::with_public_key("https://licence.example".to_string(), signing.verifying_key())
    }

    #[test]
    fn a_correctly_signed_paid_verdict_is_accepted_as_valid() {
        let signing = SigningKey::from_bytes(&[1u8; 32]);
        let now = 1_000_000;
        let body = signed_body(&signing, &verdict("paid", "n1", "webhook-relay:fp1", now));
        let outcome = client(&signing).decide(&body, "n1", "webhook-relay:fp1", now).expect("accepted");
        assert!(outcome.valid);
        assert_eq!(outcome.checked_at, now);
    }

    #[test]
    fn trialing_is_also_valid_but_expired_is_not() {
        let signing = SigningKey::from_bytes(&[2u8; 32]);
        let now = 1_000_000;
        let trialing = signed_body(&signing, &verdict("trialing", "n", "webhook-relay:fp1", now));
        assert!(client(&signing).decide(&trialing, "n", "webhook-relay:fp1", now).unwrap().valid);

        let expired = signed_body(&signing, &verdict("expired", "n", "webhook-relay:fp1", now));
        assert!(!client(&signing).decide(&expired, "n", "webhook-relay:fp1", now).unwrap().valid);
    }

    #[test]
    fn a_tampered_verdict_fails_signature_verification() {
        let signing = SigningKey::from_bytes(&[3u8; 32]);
        let now = 1_000_000;
        let mut body: serde_json::Value = serde_json::from_slice(&signed_body(
            &signing,
            &verdict("expired", "n", "webhook-relay:fp1", now),
        ))
        .unwrap();
        body["verdict"]["status"] = serde_json::json!("paid");
        let tampered = serde_json::to_vec(&body).unwrap();
        assert!(matches!(
            client(&signing).decide(&tampered, "n", "webhook-relay:fp1", now),
            Err(LicenceError::BadSignature)
        ));
    }

    #[test]
    fn a_response_signed_by_a_different_key_is_refused() {
        let signing = SigningKey::from_bytes(&[4u8; 32]);
        let other = SigningKey::from_bytes(&[5u8; 32]);
        let now = 1_000_000;
        let body = signed_body(&other, &verdict("paid", "n", "webhook-relay:fp1", now));
        assert!(matches!(
            client(&signing).decide(&body, "n", "webhook-relay:fp1", now),
            Err(LicenceError::BadSignature)
        ));
    }

    #[test]
    fn a_mismatched_nonce_or_deployment_id_is_refused() {
        let signing = SigningKey::from_bytes(&[6u8; 32]);
        let now = 1_000_000;
        let body = signed_body(&signing, &verdict("paid", "n", "webhook-relay:fp1", now));
        assert!(matches!(
            client(&signing).decide(&body, "wrong-nonce", "webhook-relay:fp1", now),
            Err(LicenceError::NonceMismatch)
        ));
        assert!(matches!(
            client(&signing).decide(&body, "n", "webhook-relay:fp-other", now),
            Err(LicenceError::DeploymentMismatch)
        ));
    }

    #[test]
    fn a_checked_at_too_far_from_now_is_refused() {
        let signing = SigningKey::from_bytes(&[7u8; 32]);
        let now = 1_000_000;
        let body = signed_body(&signing, &verdict("paid", "n", "webhook-relay:fp1", now - 1000));
        assert!(matches!(
            client(&signing).decide(&body, "n", "webhook-relay:fp1", now),
            Err(LicenceError::CheckedAtOutOfSkew)
        ));
    }

    #[test]
    fn synthetic_deployment_id_is_stable_for_a_given_fingerprint() {
        assert_eq!(
            LicenceClient::synthetic_deployment_id("fp1"),
            LicenceClient::synthetic_deployment_id("fp1")
        );
        assert_ne!(
            LicenceClient::synthetic_deployment_id("fp1"),
            LicenceClient::synthetic_deployment_id("fp2")
        );
    }
}
