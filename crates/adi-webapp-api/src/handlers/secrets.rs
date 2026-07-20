use adi_secrets::Error as SecretStoreError;
use adi_secrets::{OAuthToken, Secret, Secrets};

use crate::types::{
    OAuthInfoDto, RevealedSecret, SecretDto, SecretRef, SecretsState, SetOAuthSecret, SetSecret,
};

use super::response::{Response, clean, error, ok_json};

/// `GET /api/secrets` — every secret across all scopes, metadata only (never values). Each
/// mutation endpoint below returns a fresh [`SecretsState`], so the client refreshes from one
/// round-trip. The UI filters by `project` (global page vs a project's panel).
#[must_use]
pub fn secrets(store: &Secrets) -> Response {
    match store.list_all() {
        Ok(list) => ok_json(&SecretsState {
            secrets: list.into_iter().map(secret_dto).collect(),
        }),
        Err(e) => Response::from(&e),
    }
}

/// `POST /api/secrets/set` — create or overwrite a secret (value encrypted at rest), then
/// report the fresh list. `project` omitted/blank ⇒ global.
#[must_use]
pub fn set_secret(store: &Secrets, body: &[u8]) -> Response {
    let Some(req) = parse_set(body) else {
        return bad_set();
    };
    let project = clean(req.project);
    match store.set(
        project.as_deref(),
        req.name.trim(),
        &req.value,
        req.description.as_deref(),
    ) {
        Ok(_) => secrets(store),
        Err(e) => Response::from(&e),
    }
}

/// `POST /api/secrets/set-oauth` — store a secret whose value came from an OAuth flow: the
/// access token becomes the (encrypted) value, the refresh token is encrypted separately, and
/// the provider/lifetime/scope are recorded. Then report the fresh list.
#[must_use]
pub fn set_oauth_secret(store: &Secrets, body: &[u8]) -> Response {
    let Some(req) = parse_set_oauth(body) else {
        return bad_set_oauth();
    };
    let project = clean(req.project);
    // The provider gives seconds-to-expiry; stamp the absolute time the store keeps.
    let expires_at = req.expires_in.map(|secs| adi_config::now_unix().saturating_add(secs));
    let token = OAuthToken {
        provider: req.provider.trim().to_string(),
        access_token: req.access_token,
        refresh_token: clean(req.refresh_token),
        expires_at,
        scope: clean(req.scope),
    };
    match store.set_oauth(project.as_deref(), req.name.trim(), &token, req.description.as_deref()) {
        Ok(_) => secrets(store),
        Err(e) => Response::from(&e),
    }
}

/// `POST /api/secrets/remove` — delete a secret from a scope, then report the fresh list.
#[must_use]
pub fn remove_secret(store: &Secrets, body: &[u8]) -> Response {
    let Some(req) = parse_ref(body) else {
        return bad_ref();
    };
    let project = clean(req.project);
    match store.remove(project.as_deref(), req.name.trim()) {
        Ok(_) => secrets(store),
        Err(e) => Response::from(&e),
    }
}

/// `POST /api/secrets/reveal` — the one endpoint that returns a decrypted value. Kept separate
/// from listing so revealing is always a deliberate, single-secret request.
#[must_use]
pub fn reveal_secret(store: &Secrets, body: &[u8]) -> Response {
    let Some(req) = parse_ref(body) else {
        return bad_ref();
    };
    let project = clean(req.project);
    let name = req.name.trim();
    match store.reveal(project.as_deref(), name) {
        Ok(Some(value)) => ok_json(&RevealedSecret {
            project,
            name: name.to_string(),
            value,
        }),
        Ok(None) => error(404, &format!("no such secret: {name}")),
        Err(e) => Response::from(&e),
    }
}

/// Flatten a stored secret into its wire [`SecretDto`] (metadata only — never a token).
fn secret_dto(secret: Secret) -> SecretDto {
    SecretDto {
        project: secret.project,
        name: secret.name,
        description: secret.description,
        created_at: secret.created_at,
        updated_at: secret.updated_at,
        oauth: secret.oauth.map(|o| OAuthInfoDto {
            provider: o.provider,
            obtained_at: o.obtained_at,
            expires_at: o.expires_at,
            scope: o.scope,
            has_refresh: o.has_refresh,
        }),
    }
}

// Map a store error to an HTTP status: bad name → 400, missing → 404, dec/crypt/io → 500.
impl From<&SecretStoreError> for Response {
    fn from(e: &SecretStoreError) -> Self {
        let status = match e {
            SecretStoreError::InvalidName(_) => 400,
            SecretStoreError::NotFound(_) => 404,
            SecretStoreError::Config(_)
            | SecretStoreError::Io(_)
            | SecretStoreError::Crypto(_)
            | SecretStoreError::Decrypt => 500,
        };
        error(status, &e.to_string())
    }
}

fn parse_set(body: &[u8]) -> Option<SetSecret> {
    let req: SetSecret = serde_json::from_slice(body).ok()?;
    (!req.name.trim().is_empty()).then_some(req)
}

fn bad_set() -> Response {
    error(
        400,
        "expected JSON body { \"name\": \"…\", \"value\": \"…\", \"project\"?: \"…\", \"description\"?: \"…\" }",
    )
}

fn parse_set_oauth(body: &[u8]) -> Option<SetOAuthSecret> {
    let req: SetOAuthSecret = serde_json::from_slice(body).ok()?;
    (!req.name.trim().is_empty() && !req.provider.trim().is_empty() && !req.access_token.is_empty())
        .then_some(req)
}

fn bad_set_oauth() -> Response {
    error(
        400,
        "expected JSON body { \"name\": \"…\", \"provider\": \"…\", \"access_token\": \"…\", \"project\"?, \"refresh_token\"?, \"expires_in\"?, \"scope\"?, \"description\"? }",
    )
}

fn parse_ref(body: &[u8]) -> Option<SecretRef> {
    let req: SecretRef = serde_json::from_slice(body).ok()?;
    (!req.name.trim().is_empty()).then_some(req)
}

fn bad_ref() -> Response {
    error(400, "expected JSON body { \"name\": \"…\", \"project\"?: \"…\" }")
}
