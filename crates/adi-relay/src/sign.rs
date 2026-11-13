//! The signature on every node→relay request (`docs/webhook-relay.md` §3): a canonical string
//! over the method, request target, timestamp, nonce, and body hash, signed with the node's
//! ed25519 key and carried in four headers. [`sign`] is the node's side; [`verify`] is the
//! relay's — it recovers and returns the caller's public key, so the relay can go on to decide
//! whether that key is *authorized* (self-register, licence, `authorized_keys` — §6), which is a
//! question this module has no opinion on.

use base64::Engine as _;
use ed25519_dalek::{Signature, Signer as _, SigningKey, Verifier as _, VerifyingKey};
use sha2::{Digest as _, Sha256};

use crate::openssh::{self, ParseError};

/// The four headers a signed request carries, lowercase (every header name in this codebase's
/// own HTTP layer is read lowercase — `adi-webapp-api/src/http.rs`).
pub const HEADER_KEY: &str = "x-relay-key";
pub const HEADER_TIMESTAMP: &str = "x-relay-timestamp";
pub const HEADER_NONCE: &str = "x-relay-nonce";
pub const HEADER_SIGNATURE: &str = "x-relay-signature";

/// One signed request's worth of header values, read off or about to be written onto the wire —
/// `key_line` is the full `X-Relay-Key` value, not a fingerprint (§3: "the full public key rides
/// on every request... the signature has to be self-contained").
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignedRequest {
    pub key_line: String,
    pub timestamp: u64,
    pub nonce: String,
    pub signature: String,
}

/// The exact bytes that get signed: `METHOD "\n" PATH_AND_QUERY "\n" TIMESTAMP "\n" NONCE "\n"
/// SHA256_HEX(BODY)` (§3) — `SHA256_HEX` of the empty string for a bodyless request.
#[must_use]
pub fn canonical_string(method: &str, path_and_query: &str, timestamp: u64, nonce: &str, body: &[u8]) -> String {
    format!(
        "{method}\n{path_and_query}\n{timestamp}\n{nonce}\n{}",
        sha256_hex(body)
    )
}

#[must_use]
pub fn sha256_hex(body: &[u8]) -> String {
    hex::encode(Sha256::digest(body))
}

/// A fresh nonce: 16 random bytes, base64url, no padding (§3) — call once per request, never
/// reused, which is the entire defense [`crate::sign`]'s nonce replay check relies on.
#[must_use]
pub fn generate_nonce() -> String {
    use rand::TryRng as _;
    let mut bytes = [0u8; 16];
    rand::rngs::SysRng
        .try_fill_bytes(&mut bytes)
        .expect("the OS random source is unavailable");
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

/// Sign one request as the holder of `signing` — the node's side. `path_and_query` must be the
/// exact request-target as it will be sent (`/v1/pull?batch=50`), and `body` the exact bytes
/// that will ride the wire (empty for a bodyless `GET`).
#[must_use]
pub fn sign(
    signing: &SigningKey,
    method: &str,
    path_and_query: &str,
    timestamp: u64,
    nonce: &str,
    body: &[u8],
) -> SignedRequest {
    let canonical = canonical_string(method, path_and_query, timestamp, nonce, body);
    let signature: Signature = signing.sign(canonical.as_bytes());
    SignedRequest {
        key_line: openssh::encode(&signing.verifying_key()),
        timestamp,
        nonce: nonce.to_string(),
        signature: base64::engine::general_purpose::STANDARD.encode(signature.to_bytes()),
    }
}

/// Everything that can go wrong verifying a signed request — deliberately distinguished (unlike
/// [`crate::seal::OpenError`]'s single "didn't open"), because the relay's own error codes (§5)
/// are stated per failure: `bad_signature` covers everything here except a timestamp outside the
/// skew window, which the caller checks separately with [`within_skew`] before ever reaching
/// this function (§3: "cheaper to reject").
#[derive(Debug)]
pub enum VerifyError {
    /// `X-Relay-Key` isn't a parseable `ssh-ed25519` line.
    BadKeyLine(ParseError),
    /// `X-Relay-Signature` isn't valid base64, or isn't 64 bytes once decoded.
    BadSignatureEncoding,
    /// The signature doesn't verify against the canonical string built from the given fields.
    BadSignature,
}

impl std::fmt::Display for VerifyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::BadKeyLine(e) => write!(f, "bad X-Relay-Key: {e}"),
            Self::BadSignatureEncoding => write!(f, "X-Relay-Signature is not a valid signature"),
            Self::BadSignature => write!(f, "signature does not verify"),
        }
    }
}

impl std::error::Error for VerifyError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::BadKeyLine(e) => Some(e),
            Self::BadSignatureEncoding | Self::BadSignature => None,
        }
    }
}

/// Verify `req` was signed by the key it names, over the canonical string built from `method`,
/// `path_and_query`, and `body` — the relay's side. Returns the verified public key so the
/// caller can decide whether it's *authorized*; this function only answers "did this key sign
/// this exact request", nothing about who that key is allowed to be.
///
/// # Errors
/// See [`VerifyError`]'s variants.
pub fn verify(
    req: &SignedRequest,
    method: &str,
    path_and_query: &str,
    body: &[u8],
) -> Result<VerifyingKey, VerifyError> {
    let key = openssh::parse(&req.key_line).map_err(VerifyError::BadKeyLine)?;
    let canonical = canonical_string(method, path_and_query, req.timestamp, &req.nonce, body);
    let raw = base64::engine::general_purpose::STANDARD
        .decode(&req.signature)
        .map_err(|_| VerifyError::BadSignatureEncoding)?;
    let raw: [u8; 64] = raw
        .try_into()
        .map_err(|_| VerifyError::BadSignatureEncoding)?;
    let signature = Signature::from_bytes(&raw);
    key.verify(canonical.as_bytes(), &signature)
        .map_err(|_| VerifyError::BadSignature)?;
    Ok(key)
}

/// Is `timestamp` within `window_secs` of `now`, either direction? (§3's ±300s clock-skew check,
/// checked by the caller *before* [`verify`] — "cheaper to reject".)
#[must_use]
pub fn within_skew(timestamp: u64, now: u64, window_secs: u64) -> bool {
    now.abs_diff(timestamp) <= window_secs
}

#[cfg(test)]
mod tests {
    use super::*;

    fn signing_key(seed: u8) -> SigningKey {
        SigningKey::from_bytes(&[seed; 32])
    }

    #[test]
    fn a_signed_request_verifies_against_the_same_fields() {
        let key = signing_key(1);
        let req = sign(&key, "GET", "/v1/pull?batch=10", 1_000, "nonce-a", b"");
        let verified = verify(&req, "GET", "/v1/pull?batch=10", b"").expect("verifies");
        assert_eq!(verified, key.verifying_key());
    }

    #[test]
    fn a_different_method_fails_verification() {
        let key = signing_key(2);
        let req = sign(&key, "GET", "/v1/pull", 1_000, "n", b"");
        assert!(matches!(
            verify(&req, "POST", "/v1/pull", b""),
            Err(VerifyError::BadSignature)
        ));
    }

    #[test]
    fn a_rewritten_query_fails_verification() {
        let key = signing_key(3);
        let req = sign(&key, "GET", "/v1/pull?batch=50", 1_000, "n", b"");
        assert!(matches!(
            verify(&req, "GET", "/v1/pull?batch=5", b""),
            Err(VerifyError::BadSignature)
        ));
    }

    #[test]
    fn a_tampered_body_fails_verification() {
        let key = signing_key(4);
        let req = sign(&key, "POST", "/v1/endpoints", 1_000, "n", b"{\"a\":1}");
        assert!(matches!(
            verify(&req, "POST", "/v1/endpoints", b"{\"a\":2}"),
            Err(VerifyError::BadSignature)
        ));
    }

    #[test]
    fn a_signature_from_a_different_key_fails() {
        let key = signing_key(5);
        let other = signing_key(6);
        let mut req = sign(&key, "GET", "/v1/pull", 1_000, "n", b"");
        req.key_line = crate::openssh::encode(&other.verifying_key());
        assert!(matches!(
            verify(&req, "GET", "/v1/pull", b""),
            Err(VerifyError::BadSignature)
        ));
    }

    #[test]
    fn a_malformed_key_line_is_its_own_error() {
        let key = signing_key(7);
        let mut req = sign(&key, "GET", "/v1/pull", 1_000, "n", b"");
        req.key_line = "ssh-rsa AAAA".to_string();
        assert!(matches!(
            verify(&req, "GET", "/v1/pull", b""),
            Err(VerifyError::BadKeyLine(_))
        ));
    }

    #[test]
    fn skew_window_is_symmetric_and_inclusive() {
        assert!(within_skew(1_000, 1_300, 300));
        assert!(within_skew(1_300, 1_000, 300));
        assert!(!within_skew(1_000, 1_301, 300));
    }

    #[test]
    fn generated_nonces_differ_and_carry_no_padding() {
        let a = generate_nonce();
        let b = generate_nonce();
        assert_ne!(a, b);
        assert!(!a.contains('='), "base64url, no padding");
    }
}
