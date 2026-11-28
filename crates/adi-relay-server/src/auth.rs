//! Two separate questions for every node→relay request, kept deliberately apart:
//! [`authenticate`] answers "did this exact request come from the key it names" (signature,
//! clock skew, nonce replay — nothing about who that key is allowed to be); [`AuthorizedKeys`]
//! answers the self-host mode's own extra question, "is that key one an operator actually
//! listed" — the only mode where authorization is a per-request check rather than a register-time
//! decision cached as a fact about the endpoint: nothing gets this far without first registering,
//! so a key struck from the file is locked out everywhere, not just from a future register.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Instant, SystemTime};

use adi_relay::openssh::{self, Fingerprint};
use adi_relay::sign::{self, SignedRequest};
use ed25519_dalek::VerifyingKey;
use subtle::ConstantTimeEq as _;

use crate::config::SelfHostConfig;
use crate::store::Store;
use crate::{Response, err};

/// The request-signing clock-skew window.
pub const SKEW_WINDOW_SECS: u64 = 300;

/// Self-host's optional extra header: never part of the signed canonical string, since it's a
/// static value a signature can't meaningfully protect any further.
pub const HEADER_SHARED_SECRET: &str = "x-relay-shared-secret";

/// Read the four `X-Relay-*` headers off a request into a [`SignedRequest`]. `None` if any of
/// them is missing or the timestamp isn't a plain integer — indistinguishable, on the wire, from
/// a bad signature (the `bad_signature` error code covers both).
#[must_use]
pub fn extract(headers: &HashMap<String, String>) -> Option<SignedRequest> {
    Some(SignedRequest {
        key_line: headers.get(sign::HEADER_KEY)?.clone(),
        timestamp: headers.get(sign::HEADER_TIMESTAMP)?.parse().ok()?,
        nonce: headers.get(sign::HEADER_NONCE)?.clone(),
        signature: headers.get(sign::HEADER_SIGNATURE)?.clone(),
    })
}

/// The three ways [`authenticate`] can refuse a request, named exactly as the wire contract's
/// error codes — `Ok` carries the verified key, so a caller that needs the fingerprint doesn't
/// have to re-derive it from anything.
#[derive(Debug)]
pub enum AuthOutcome {
    Ok(VerifyingKey),
    BadSignature,
    StaleTimestamp,
    ReplayedNonce,
}

impl AuthOutcome {
    #[must_use]
    pub fn error_code(&self) -> &'static str {
        match self {
            Self::Ok(_) => "",
            Self::BadSignature => "bad_signature",
            Self::StaleTimestamp => "stale_timestamp",
            Self::ReplayedNonce => "replayed_nonce",
        }
    }
}

/// Authenticate one signed request: the headers parse, the timestamp is within [`SKEW_WINDOW_SECS`]
/// (checked *before* the signature, since that's cheaper to reject), the signature verifies over
/// the exact method/path-and-query/body given, and the nonce hasn't been seen before from this
/// key inside the window.
///
/// # Errors
/// Any `rusqlite` error from the nonce-replay check — a store failure, never a refusal; the
/// caller maps it to a 500, not one of [`AuthOutcome`]'s codes.
pub fn authenticate(
    store: &Store,
    headers: &HashMap<String, String>,
    method: &str,
    path_and_query: &str,
    body: &[u8],
    now: u64,
) -> rusqlite::Result<AuthOutcome> {
    let Some(req) = extract(headers) else {
        return Ok(AuthOutcome::BadSignature);
    };
    if !sign::within_skew(req.timestamp, now, SKEW_WINDOW_SECS) {
        return Ok(AuthOutcome::StaleTimestamp);
    }
    let Ok(key) = sign::verify(&req, method, path_and_query, body) else {
        return Ok(AuthOutcome::BadSignature);
    };
    let fingerprint = Fingerprint::of_key(&key);
    let expires_at = i64::try_from(now + SKEW_WINDOW_SECS).unwrap_or(i64::MAX);
    let fresh = store.remember_nonce(fingerprint.as_str(), &req.nonce, expires_at)?;
    if !fresh {
        return Ok(AuthOutcome::ReplayedNonce);
    }
    Ok(AuthOutcome::Ok(key))
}

/// Self-host mode's `authorized_keys` file: reloaded by mtime, at most every 5 seconds — the
/// same cadence `adi-mesh`'s own gateway reload uses for `fleet.toml`. A `stat()` is cheap enough
/// to pay on every request; actually re-reading and re-parsing the file only happens when the
/// mtime has moved, and even the stat is skipped for 5 seconds after the last one.
pub struct AuthorizedKeys {
    path: PathBuf,
    state: Mutex<KeysState>,
}

/// `VerifyingKey` has no `Debug`, so neither does `KeysState`; print the path only.
impl std::fmt::Debug for AuthorizedKeys {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AuthorizedKeys")
            .field("path", &self.path)
            .finish_non_exhaustive()
    }
}

struct KeysState {
    keys: Vec<VerifyingKey>,
    mtime: Option<SystemTime>,
    last_checked: Instant,
}

const RELOAD_INTERVAL: std::time::Duration = std::time::Duration::from_secs(5);

impl AuthorizedKeys {
    /// Load `path` once, eagerly — a missing file reads as empty (every key is then
    /// `unknown_key`) rather than a startup error, since an operator may stand the relay up
    /// before populating the file.
    #[must_use]
    pub fn new(path: PathBuf) -> Self {
        let (keys, mtime) = Self::load(&path);
        Self {
            path,
            state: Mutex::new(KeysState {
                keys,
                mtime,
                last_checked: Instant::now(),
            }),
        }
    }

    fn load(path: &Path) -> (Vec<VerifyingKey>, Option<SystemTime>) {
        let mtime = std::fs::metadata(path).and_then(|m| m.modified()).ok();
        let keys = std::fs::read_to_string(path)
            .map(|text| openssh::parse_authorized_keys(&text))
            .unwrap_or_default();
        (keys, mtime)
    }

    fn maybe_reload(&self) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if state.last_checked.elapsed() < RELOAD_INTERVAL {
            return;
        }
        state.last_checked = Instant::now();
        let mtime = std::fs::metadata(&self.path)
            .and_then(|m| m.modified())
            .ok();
        if mtime == state.mtime {
            return;
        }
        let (keys, mtime) = Self::load(&self.path);
        state.keys = keys;
        state.mtime = mtime;
    }

    /// Is `key` one of the lines in the file, as of the last reload?
    #[must_use]
    pub fn is_authorized(&self, key: &VerifyingKey) -> bool {
        self.maybe_reload();
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .keys
            .contains(key)
    }
}

/// Self-host's optional shared secret — defence in depth on top of the signature, not instead
/// of it. `None` (pass) if the config names no `shared_secret_env`; otherwise the
/// request must carry [`HEADER_SHARED_SECRET`] matching the named env var's *current* value,
/// compared in constant time. An env var the config names but this process doesn't have set is
/// the operator's own misconfiguration — fails closed (every request refused) rather than
/// silently skipping a check an operator believes is active.
#[must_use]
pub fn check_shared_secret(
    self_host: &SelfHostConfig,
    headers: &HashMap<String, String>,
) -> Option<Response> {
    let env_name = self_host
        .shared_secret_env
        .as_ref()
        .filter(|s| !s.is_empty())?;
    let Ok(expected) = std::env::var(env_name) else {
        return Some(err(
            500,
            "internal_error",
            format!("self_host.shared_secret_env names {env_name}, which isn't set"),
        ));
    };
    let offered = headers
        .get(HEADER_SHARED_SECRET)
        .map(String::as_bytes)
        .unwrap_or_default();
    if offered.ct_eq(expected.as_bytes()).into() {
        None
    } else {
        Some(err(
            403,
            "unknown_key",
            "missing or incorrect X-Relay-Shared-Secret",
        ))
    }
}

/// Self-host's optional IP allowlist, the same "defence in depth" stance as the shared
/// secret above. `None` (pass) if the list is empty (empty means unrestricted) or `remote_ip`
/// doesn't even parse — the latter should never happen for a real peer address, and refusing a
/// request over this crate's own inability to parse the socket's own address would be a strange
/// kind of defence in depth.
#[must_use]
pub fn check_ip_allowlist(self_host: &SelfHostConfig, remote_ip: &str) -> Option<Response> {
    if self_host.ip_allowlist.is_empty() {
        return None;
    }
    let Ok(ip) = remote_ip.parse() else {
        return None;
    };
    if crate::cidr::allows(&self_host.ip_allowlist, ip) {
        None
    } else {
        Some(err(
            403,
            "unknown_key",
            "this address is not in ip_allowlist",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::SigningKey;

    fn key(seed: u8) -> VerifyingKey {
        SigningKey::from_bytes(&[seed; 32]).verifying_key()
    }

    fn now() -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs()
    }

    #[test]
    fn a_well_formed_request_authenticates() {
        let store = Store::open_in_memory().unwrap();
        let signing = SigningKey::from_bytes(&[1u8; 32]);
        let req = sign::sign(
            &signing,
            "GET",
            "/v1/pull",
            now(),
            &sign::generate_nonce(),
            b"",
        );
        let headers = HashMap::from([
            (sign::HEADER_KEY.to_string(), req.key_line.clone()),
            (
                sign::HEADER_TIMESTAMP.to_string(),
                req.timestamp.to_string(),
            ),
            (sign::HEADER_NONCE.to_string(), req.nonce.clone()),
            (sign::HEADER_SIGNATURE.to_string(), req.signature.clone()),
        ]);
        let outcome = authenticate(&store, &headers, "GET", "/v1/pull", b"", now()).unwrap();
        assert!(matches!(outcome, AuthOutcome::Ok(k) if k == signing.verifying_key()));
    }

    #[test]
    fn missing_headers_are_bad_signature() {
        let store = Store::open_in_memory().unwrap();
        let outcome = authenticate(&store, &HashMap::new(), "GET", "/v1/pull", b"", now()).unwrap();
        assert!(matches!(outcome, AuthOutcome::BadSignature));
    }

    #[test]
    fn a_stale_timestamp_is_refused_before_the_signature_is_even_checked() {
        let store = Store::open_in_memory().unwrap();
        let signing = SigningKey::from_bytes(&[2u8; 32]);
        let old = now() - 10_000;
        let req = sign::sign(
            &signing,
            "GET",
            "/v1/pull",
            old,
            &sign::generate_nonce(),
            b"",
        );
        let headers = HashMap::from([
            (sign::HEADER_KEY.to_string(), req.key_line),
            (
                sign::HEADER_TIMESTAMP.to_string(),
                req.timestamp.to_string(),
            ),
            (sign::HEADER_NONCE.to_string(), req.nonce),
            (sign::HEADER_SIGNATURE.to_string(), req.signature),
        ]);
        assert!(matches!(
            authenticate(&store, &headers, "GET", "/v1/pull", b"", now()).unwrap(),
            AuthOutcome::StaleTimestamp
        ));
    }

    #[test]
    fn a_replayed_nonce_is_refused_the_second_time() {
        let store = Store::open_in_memory().unwrap();
        let signing = SigningKey::from_bytes(&[3u8; 32]);
        let t = now();
        let req = sign::sign(&signing, "GET", "/v1/pull", t, "fixed-nonce", b"");
        let headers = HashMap::from([
            (sign::HEADER_KEY.to_string(), req.key_line),
            (
                sign::HEADER_TIMESTAMP.to_string(),
                req.timestamp.to_string(),
            ),
            (sign::HEADER_NONCE.to_string(), req.nonce),
            (sign::HEADER_SIGNATURE.to_string(), req.signature),
        ]);
        assert!(matches!(
            authenticate(&store, &headers, "GET", "/v1/pull", b"", t).unwrap(),
            AuthOutcome::Ok(_)
        ));
        assert!(matches!(
            authenticate(&store, &headers, "GET", "/v1/pull", b"", t).unwrap(),
            AuthOutcome::ReplayedNonce
        ));
    }

    #[test]
    fn authorized_keys_reads_the_file_and_rejects_everything_else() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("authorized_keys");
        let k1 = key(1);
        let k2 = key(2);
        std::fs::write(&path, format!("{}\n# comment\n", openssh::encode(&k1))).unwrap();

        let keys = AuthorizedKeys::new(path);
        assert!(keys.is_authorized(&k1));
        assert!(!keys.is_authorized(&k2));
    }

    #[test]
    fn a_missing_authorized_keys_file_authorizes_nobody() {
        let keys = AuthorizedKeys::new(PathBuf::from("/nonexistent/authorized_keys"));
        assert!(!keys.is_authorized(&key(1)));
    }

    fn self_host(shared_secret_env: Option<&str>, ip_allowlist: &[&str]) -> SelfHostConfig {
        SelfHostConfig {
            authorized_keys_path: PathBuf::from("/nonexistent"),
            shared_secret_env: shared_secret_env.map(str::to_string),
            ip_allowlist: ip_allowlist.iter().map(ToString::to_string).collect(),
        }
    }

    /// A unique env var name per test — this is process-global state, and the crate's own test
    /// binary runs tests in parallel threads by default.
    fn set_env(name: &str, value: &str) {
        // SAFETY: test-only, and `name` is unique per call site below.
        unsafe { std::env::set_var(name, value) };
    }

    #[test]
    fn no_shared_secret_env_configured_is_a_pass() {
        let sh = self_host(None, &[]);
        assert!(check_shared_secret(&sh, &HashMap::new()).is_none());
    }

    #[test]
    fn a_matching_shared_secret_passes_and_a_wrong_or_missing_one_is_refused() {
        set_env("ADI_RELAY_TEST_SECRET_A", "s3cr3t");
        let sh = self_host(Some("ADI_RELAY_TEST_SECRET_A"), &[]);

        let right = HashMap::from([(HEADER_SHARED_SECRET.to_string(), "s3cr3t".to_string())]);
        assert!(check_shared_secret(&sh, &right).is_none());

        let wrong = HashMap::from([(HEADER_SHARED_SECRET.to_string(), "nope".to_string())]);
        assert_eq!(check_shared_secret(&sh, &wrong).unwrap().status, 403);

        assert_eq!(
            check_shared_secret(&sh, &HashMap::new()).unwrap().status,
            403
        );
    }

    #[test]
    fn a_shared_secret_env_the_process_never_set_fails_closed() {
        let sh = self_host(Some("ADI_RELAY_TEST_SECRET_UNSET_B"), &[]);
        let refusal = check_shared_secret(&sh, &HashMap::new()).expect("fails closed");
        assert_eq!(refusal.status, 500);
    }

    #[test]
    fn an_empty_ip_allowlist_is_unrestricted() {
        let sh = self_host(None, &[]);
        assert!(check_ip_allowlist(&sh, "203.0.113.1").is_none());
    }

    #[test]
    fn an_ip_allowlist_refuses_an_address_outside_it() {
        let sh = self_host(None, &["10.0.0.0/8"]);
        assert!(check_ip_allowlist(&sh, "10.1.2.3").is_none());
        assert_eq!(check_ip_allowlist(&sh, "203.0.113.1").unwrap().status, 403);
    }
}
