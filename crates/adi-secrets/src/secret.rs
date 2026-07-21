//! The on-disk secret manifest ([`Manifest`], serialized as `<name>.toml`) and the
//! metadata view of a stored secret ([`Secret`]) — which deliberately never carries the value.

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

/// A secret's on-disk manifest: the encrypted value plus metadata. The secret's **key name and
/// scope are its file's location** (`global/<name>.toml` or `projects/<id>/<name>.toml`), not
/// fields here — so this struct holds only ciphertext and descriptive data, never a plaintext
/// value and never the name.
///
/// Unknown fields are ignored so the manifest can gain fields without breaking older stores.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub(crate) struct Manifest {
    /// An optional one-line description of what the secret is for.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// The per-secret `XChaCha20` nonce, base64. Encrypts the value (an access token, for an
    /// OAuth secret).
    pub nonce: String,
    /// The AEAD ciphertext of the value, base64.
    pub ciphertext: String,
    /// When the secret was first set, as Unix epoch seconds.
    #[serde(default)]
    pub created_at: u64,
    /// When the secret's value was last set, as Unix epoch seconds.
    #[serde(default)]
    pub updated_at: u64,
    /// Present only when the value was obtained through an OAuth flow rather than typed. It
    /// records the provider and token lifetime, and carries the (separately-encrypted) refresh
    /// token — so an OAuth secret's value *is* its access token, plus the metadata to show and
    /// refresh it. A plain text secret leaves this `None`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub oauth: Option<OAuthMeta>,
}

impl adi_config::Timestamped for Manifest {
    fn created_at(&self) -> u64 {
        self.created_at
    }
}

/// The OAuth provenance of a secret's value, stored on disk. Everything here except the
/// encrypted `refresh_*` is plaintext metadata; the access token lives in the manifest's own
/// `ciphertext` (it's the secret's value).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct OAuthMeta {
    /// The provider the token came from (`google`, `github`, …).
    pub provider: String,
    /// When the current access token was obtained, Unix epoch seconds.
    pub obtained_at: u64,
    /// When the access token expires, Unix epoch seconds; `None` if the provider gave no expiry.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<u64>,
    /// The granted scopes, as the provider returned them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
    /// The refresh token's own AEAD nonce, base64 — present only when a refresh token was stored.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refresh_nonce: Option<String>,
    /// The AEAD ciphertext of the refresh token, base64.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refresh_ciphertext: Option<String>,
}

/// A stored secret's **metadata** — its scope, name, description, and timestamps. It carries
/// **no value**: the plaintext is only ever produced by an explicit
/// [`Secrets::reveal`](crate::Secrets::reveal), never as a field here, so listing or logging a
/// `Secret` can't leak it. `Serialize` only — it is built from disk, never deserialized.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Secret {
    /// The project this secret is scoped to (its [`adi-projects`](adi_config) id), or `None`
    /// for a global secret — the same `project: Option<String>` convention every entity uses.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    /// The secret's key name — its `<name>.toml` file stem, and the env-var name it injects as.
    pub name: String,
    /// The optional description.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// When the secret was first set, as Unix epoch seconds.
    pub created_at: u64,
    /// When the value was last set, as Unix epoch seconds.
    pub updated_at: u64,
    /// OAuth provenance, if the value came from a provider flow — provider, lifetime, and
    /// whether a refresh token is held. **Never the tokens themselves.** `None` for a plain
    /// text secret.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub oauth: Option<OAuthInfo>,
}

/// The non-secret view of a secret's OAuth provenance — safe to list/serialize. It reports the
/// provider, the token lifetime, and whether a refresh token is held, but never a token value.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct OAuthInfo {
    pub provider: String,
    pub obtained_at: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
    /// Whether a refresh token is stored, so a UI can offer "refresh" vs only "re-authorize".
    pub has_refresh: bool,
}

/// A freshly obtained OAuth token, handed to [`Secrets::set_oauth`](crate::Secrets::set_oauth)
/// to store: the access token becomes the secret's (encrypted) value, the refresh token is
/// encrypted separately, and the rest is metadata.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OAuthToken {
    pub provider: String,
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub expires_at: Option<u64>,
    pub scope: Option<String>,
}

/// Validate a secret **key name**. A secret is delivered to runs as an environment variable,
/// so the name must be a valid env-var identifier: an ASCII letter or `_`, then letters,
/// digits, or `_`. This is stricter than a store path segment (no `.`, `-`, or leading digit),
/// which also makes it a safe filename and blocks path traversal — a security boundary, since
/// names arrive from the CLI and the HTTP API and are joined onto the store path.
pub(crate) fn validate_name(name: &str) -> Result<()> {
    let mut chars = name.chars();
    let valid = matches!(chars.next(), Some(c) if c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_');
    if valid {
        Ok(())
    } else {
        Err(Error::InvalidName(name.to_string()))
    }
}

/// Validate a **project id** used as a secret's scope. Project ids are `adi-projects` ids
/// (generated UUIDs, which contain `-`), so this uses the platform's filesystem-safe name rule
/// rather than the stricter env-identifier rule that key names get.
pub(crate) fn validate_project(id: &str) -> Result<()> {
    adi_config::validate_name(id, Error::InvalidName)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_names_are_env_identifiers() {
        for name in ["API_KEY", "DATABASE_URL", "token2", "_hidden", "X"] {
            assert!(validate_name(name).is_ok(), "{name} should be valid");
        }
    }

    #[test]
    fn invalid_names_are_rejected() {
        // Not env-var identifiers: traversal, separators, dashes/dots, and a leading digit.
        for name in [
            "", ".", "..", "a/b", "a\\b", "with space", "sneaky/../x", "database-url", "a.b", "2fa",
        ] {
            assert!(
                matches!(validate_name(name), Err(Error::InvalidName(_))),
                "{name:?} should be rejected"
            );
        }
    }

    #[test]
    fn project_ids_allow_uuid_dashes_but_never_traversal() {
        assert!(validate_project("3f2504e0-4f89-41d3-9a0c-0305e82c3301").is_ok());
        assert!(validate_project("proj42").is_ok());
        assert!(matches!(validate_project("../x"), Err(Error::InvalidName(_))));
        assert!(matches!(validate_project("a/b"), Err(Error::InvalidName(_))));
    }

    #[test]
    fn the_metadata_view_never_serializes_a_value() {
        // A `Secret` has no value field at all; prove the wire form only carries metadata.
        let secret = Secret {
            project: None,
            name: "API_KEY".to_string(),
            description: Some("demo".to_string()),
            created_at: 1,
            updated_at: 2,
            oauth: None,
        };
        let json = serde_json::to_string(&secret).expect("serialize");
        assert!(json.contains("\"name\":\"API_KEY\""));
        assert!(!json.contains("value"));
        assert!(!json.contains("ciphertext"));
    }
}
