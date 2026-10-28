//! A user's own BYO-credentials scope, and the server's policy over the whole feature
//! (`docs/byo-credentials.md` §4, §7, ADI-MONO-178 P1.11).
//!
//! ```text
//! GET  /api/secrets/mine            → { names, missing }         — never a value
//! POST /api/secrets/mine/set        → store a value already sealed on the caller's device (§8)
//! POST /api/secrets/mine/remove     → delete one of the caller's own
//! POST /api/secrets/mine/settings   → the caller's own `missing` preference
//! POST /api/secrets/policy          → the server's `[user_secrets]` — admin-only
//! ```
//!
//! `user` throughout is the caller's mesh user scope (`adi_mesh`'s `user_scope`,
//! `docs/byo-credentials.md` §3.2) — computed by the router from the gateway's identity headers
//! alone, never from a request body. There is no `user` field on any request type here for
//! exactly that reason.

use adi_secrets::{Missing, Secrets, UserSecretsPolicy, UserSettings, Wrap, effective_missing};

use crate::types::{
    MySecretDto, MySecretsState, RemoveMySecret, SecretsPolicyState, SetMySecret, SetMySettings,
    SetSecretsPolicy, WrapDto,
};

use super::response::{FromBody, Response, error, ok_json};

fn missing_str(missing: Missing) -> String {
    match missing {
        Missing::Server => "server",
        Missing::Wait => "wait",
        Missing::Fail => "fail",
    }
    .to_string()
}

fn parse_missing(text: &str) -> Option<Missing> {
    match text {
        "server" => Some(Missing::Server),
        "wait" => Some(Missing::Wait),
        "fail" => Some(Missing::Fail),
        _ => None,
    }
}

/// `GET /api/secrets/mine` — this caller's own names, metadata only, and the `missing` that
/// would apply to their next turn. `user: None` (a local caller, or a mesh peer with no user
/// scope) answers an empty list and the server's own `missing` — there is no "mine" to speak of.
#[must_use]
pub fn secrets_mine(store: &Secrets, user: Option<&str>) -> Response {
    let policy = UserSecretsPolicy::load(store.config());
    let Some(user) = user else {
        return ok_json(&MySecretsState {
            names: Vec::new(),
            missing: missing_str(policy.missing),
        });
    };
    let names = match store.list_mine_wrapped(user) {
        Ok(list) => list
            .into_iter()
            .map(|s| MySecretDto {
                name: s.name,
                description: s.description,
                updated_at: s.updated_at,
                nonce: s.nonce,
                ciphertext: s.ciphertext,
                wraps: s
                    .wraps
                    .into_iter()
                    .map(|w| WrapDto {
                        kind: w.kind,
                        nonce: w.nonce,
                        wrapped: w.wrapped,
                    })
                    .collect(),
            })
            .collect(),
        Err(e) => return Response::from(&e),
    };
    let own = UserSettings::load(store.config(), user);
    ok_json(&MySecretsState {
        names,
        missing: missing_str(effective_missing(policy.missing, own.missing)),
    })
}

/// `POST /api/secrets/mine/set` — store a value already sealed on the caller's own device
/// (`docs/byo-credentials.md` §7, §8, ADI-MONO-180 P2.1). The checks run in the order the spec
/// states them, so the *reason* for a refusal is always the first one that actually applies: a
/// caller with no user scope, a disabled policy, a name outside `overridable`, and finally a
/// request with no wrap at all (`wraps` must carry exactly the one device wrap v1 supports,
/// §8: "one DEK per secret, one wrap"). Nothing here decrypts `ciphertext`/`nonce`/`wraps` — the
/// plaintext never reached this process, and this route doesn't make it start now.
#[must_use]
pub fn set_my_secret(store: &Secrets, body: &[u8], user: Option<&str>) -> Response {
    let req = require!(body, SetMySecret);
    let Some(user) = user else {
        return error(400, "a local caller has no user scope to set a value in");
    };
    let policy = UserSecretsPolicy::load(store.config());
    if !policy.enabled {
        return error(400, "the user-secrets feature is off on this server");
    }
    if !policy.overridable.iter().any(|name| name == &req.name) {
        return error(
            400,
            &format!("{} is not on this server's overridable list", req.name),
        );
    }
    if req.wraps.len() != 1 {
        return error(
            400,
            "expected exactly one device wrap in `wraps` (one DEK per secret, one wrap in v1)",
        );
    }
    if req.nonce.is_empty() || req.ciphertext.is_empty() {
        return error(400, "a sealed value needs both `nonce` and `ciphertext`");
    }
    let wraps = req
        .wraps
        .into_iter()
        .map(|w| Wrap {
            kind: w.kind,
            nonce: w.nonce,
            wrapped: w.wrapped,
        })
        .collect();
    match store.set_wrapped(user, &req.name, req.description.as_deref(), req.nonce, req.ciphertext, wraps) {
        Ok(_) => secrets_mine(store, Some(user)),
        Err(e) => Response::from(&e),
    }
}

/// `POST /api/secrets/mine/remove` — works from phase 1: deletes one of the caller's own, if any.
#[must_use]
pub fn remove_my_secret(store: &Secrets, body: &[u8], user: Option<&str>) -> Response {
    let req = require!(body, RemoveMySecret);
    let Some(user) = user else {
        return error(400, "a local caller has no user scope to remove a value from");
    };
    match store.remove_mine(user, &req.name) {
        Ok(_) => secrets_mine(store, Some(user)),
        Err(e) => Response::from(&e),
    }
}

/// `POST /api/secrets/mine/settings` — a user's own `missing` preference. Works from phase 1;
/// an unrecognised `missing` string is a 400, and `null`/absent clears the preference back to
/// "no preference".
#[must_use]
pub fn set_my_settings(store: &Secrets, body: &[u8], user: Option<&str>) -> Response {
    let req = require!(body, SetMySettings);
    let Some(user) = user else {
        return error(400, "a local caller has no user scope with its own settings");
    };
    let missing = match req.missing.as_deref() {
        None => None,
        Some(text) => match parse_missing(text) {
            Some(missing) => Some(missing),
            None => return error(400, &format!("unrecognised missing value: {text}")),
        },
    };
    match (UserSettings { missing }).save(store.config(), user) {
        Ok(()) => secrets_mine(store, Some(user)),
        Err(e) => Response::from(&e),
    }
}

/// `GET /api/secrets/policy` — the server's own `[user_secrets]`, exactly as stored.
#[must_use]
pub fn secrets_policy(store: &Secrets) -> Response {
    ok_json(&policy_dto(&UserSecretsPolicy::load(store.config())))
}

/// `POST /api/secrets/policy` — replace the server's `[user_secrets]`. Admin-only (the member
/// gate's list, `docs/byo-credentials.md` §4.1).
#[must_use]
pub fn set_secrets_policy(store: &Secrets, body: &[u8]) -> Response {
    let req = require!(body, SetSecretsPolicy);
    let Some(missing) = parse_missing(req.missing.trim()) else {
        return error(400, &format!("unrecognised missing value: {}", req.missing));
    };
    let policy = UserSecretsPolicy {
        enabled: req.enabled,
        overridable: req
            .overridable
            .iter()
            .map(|name| name.trim().to_string())
            .filter(|name| !name.is_empty())
            .collect(),
        missing,
        max_lease: req
            .max_lease
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(ToString::to_string),
    };
    match policy.save(store.config()) {
        Ok(()) => ok_json(&policy_dto(&policy)),
        Err(e) => Response::from(&e),
    }
}

fn policy_dto(policy: &UserSecretsPolicy) -> SecretsPolicyState {
    SecretsPolicyState {
        enabled: policy.enabled,
        overridable: policy.overridable.clone(),
        missing: missing_str(policy.missing),
        max_lease: policy.max_lease.clone(),
    }
}

impl FromBody for SetMySecret {
    const EXPECTED: &'static str = "expected JSON body { \"name\": \"…\", \"description\"?, \"ciphertext\"?, \"nonce\"?, \"wraps\"? }";

    fn is_complete(&self) -> bool {
        !self.name.trim().is_empty()
    }
}

impl FromBody for RemoveMySecret {
    const EXPECTED: &'static str = "expected JSON body { \"name\": \"…\" }";

    fn is_complete(&self) -> bool {
        !self.name.trim().is_empty()
    }
}

impl FromBody for SetMySettings {
    const EXPECTED: &'static str = "expected JSON body { \"missing\"?: \"server\"|\"wait\"|\"fail\" }";

    fn is_complete(&self) -> bool {
        true
    }
}

impl FromBody for SetSecretsPolicy {
    const EXPECTED: &'static str = "expected JSON body { \"enabled\": bool, \"overridable\": [\"…\"], \"missing\": \"server\"|\"wait\"|\"fail\", \"max_lease\"? }";

    fn is_complete(&self) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    fn scratch(tag: &str) -> Secrets {
        let root = std::env::temp_dir().join(format!(
            "adi-webapp-api-secrets-mine-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id(),
        ));
        let _ = std::fs::remove_dir_all(&root);
        Secrets::with_config(adi_config::Config::with_root(root))
    }

    fn enable(store: &Secrets, overridable: &[&str]) {
        UserSecretsPolicy {
            enabled: true,
            overridable: overridable.iter().map(|s| (*s).to_string()).collect(),
            ..UserSecretsPolicy::default()
        }
        .save(store.config())
        .expect("save policy");
    }

    #[test]
    fn a_local_caller_sees_an_empty_list_and_the_servers_own_missing() {
        let store = scratch("local-caller");
        enable(&store, &["CLAUDE_CODE_OAUTH_TOKEN"]);

        let Response { status, body } = secrets_mine(&store, None);
        assert_eq!(status, 200);
        let v: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["names"], serde_json::json!([]));
        assert_eq!(v["missing"], "server");
    }

    /// `GET /mine` now carries the sealed `nonce`/`ciphertext`/`wraps` on purpose (§9 step 2: a
    /// device's own unlock path reads its wraps straight off this route) — what it must never
    /// carry, still, is the plaintext underneath them.
    #[test]
    fn get_mine_carries_the_wraps_but_never_the_plaintext() {
        let store = scratch("never-reveals");
        let dek = adi_secrets::wrap::generate_dek();
        let (nonce, ciphertext) =
            adi_secrets::wrap::seal_value(&dek, "u-aaa", "CLAUDE_CODE_OAUTH_TOKEN", b"sk-mine").expect("seal");
        store
            .set_wrapped(
                "u-aaa",
                "CLAUDE_CODE_OAUTH_TOKEN",
                None,
                nonce,
                ciphertext,
                vec![adi_secrets::Wrap {
                    kind: "device-key-v1".to_string(),
                    nonce: "wn".to_string(),
                    wrapped: "wc".to_string(),
                }],
            )
            .expect("set_wrapped");

        let Response { status, body } = secrets_mine(&store, Some("u-aaa"));
        assert_eq!(status, 200);
        assert!(!body.contains("sk-mine"), "{body}");
        let v: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["names"][0]["name"], "CLAUDE_CODE_OAUTH_TOKEN");
        assert!(v["names"][0]["ciphertext"].as_str().is_some_and(|s| !s.is_empty()));
        assert_eq!(v["names"][0]["wraps"][0]["kind"], "device-key-v1");
    }

    #[test]
    fn remove_mine_deletes_only_the_callers_own() {
        let store = scratch("remove-mine");
        set_scoped_for_test(&store, "u-aaa", "K");
        set_scoped_for_test(&store, "u-bbb", "K");

        let Response { status, .. } =
            remove_my_secret(&store, br#"{"name":"K"}"#, Some("u-aaa"));
        assert_eq!(status, 200);
        assert!(store.list_mine("u-aaa").unwrap().is_empty());
        assert_eq!(store.list_mine("u-bbb").unwrap().len(), 1);

        assert_eq!(
            remove_my_secret(&store, br#"{"name":"K"}"#, None).status,
            400,
            "a local caller has no scope to remove from"
        );
    }

    #[test]
    fn set_my_settings_round_trips_and_clears() {
        let store = scratch("my-settings");

        let Response { status, .. } =
            set_my_settings(&store, br#"{"missing":"wait"}"#, Some("u-aaa"));
        assert_eq!(status, 200);
        assert_eq!(UserSettings::load(store.config(), "u-aaa").missing, Some(Missing::Wait));

        let Response { status, .. } = set_my_settings(&store, b"{}", Some("u-aaa"));
        assert_eq!(status, 200);
        assert_eq!(UserSettings::load(store.config(), "u-aaa").missing, None);

        assert_eq!(
            set_my_settings(&store, br#"{"missing":"nonsense"}"#, Some("u-aaa")).status,
            400
        );
        assert_eq!(
            set_my_settings(&store, br#"{"missing":"wait"}"#, None).status,
            400
        );
    }

    /// Every path refuses, as documented, but each for its own stated reason — and only the
    /// fully-valid, fully-wrapped request actually stores anything (ADI-MONO-180 P2.1: `set` no
    /// longer stops at a blanket refusal once it has a wrap to store).
    #[test]
    fn set_my_secret_refuses_in_order_then_succeeds_with_a_wrap() {
        let store = scratch("set-refuses");

        assert_eq!(
            set_my_secret(&store, br#"{"name":"CLAUDE_CODE_OAUTH_TOKEN"}"#, None).status,
            400,
            "no user scope"
        );

        let Response { status, body } =
            set_my_secret(&store, br#"{"name":"CLAUDE_CODE_OAUTH_TOKEN"}"#, Some("u-aaa"));
        assert_eq!(status, 400, "{body}");
        assert!(body.contains("off"), "{body}");

        enable(&store, &["ANTHROPIC_API_KEY"]);
        let Response { status, body } =
            set_my_secret(&store, br#"{"name":"CLAUDE_CODE_OAUTH_TOKEN"}"#, Some("u-aaa"));
        assert_eq!(status, 400, "{body}");
        assert!(body.contains("overridable"), "{body}");

        let Response { status, body } = set_my_secret(
            &store,
            br#"{"name":"ANTHROPIC_API_KEY","nonce":"n","ciphertext":"c","wraps":[]}"#,
            Some("u-aaa"),
        );
        assert_eq!(status, 400, "{body}");
        assert!(body.contains("exactly one"), "{body}");

        let req = br#"{"name":"ANTHROPIC_API_KEY","nonce":"n","ciphertext":"c",
            "wraps":[{"kind":"device-key-v1","nonce":"wn","wrapped":"wc"}]}"#;
        let Response { status, body } = set_my_secret(&store, req, Some("u-aaa"));
        assert_eq!(status, 200, "{body}");

        let names = store.list_mine("u-aaa").expect("list");
        assert_eq!(names.len(), 1);
        assert_eq!(names[0].name, "ANTHROPIC_API_KEY");
    }

    #[test]
    fn set_my_secret_refuses_a_request_missing_the_sealed_value() {
        let store = scratch("set-no-value");
        enable(&store, &["ANTHROPIC_API_KEY"]);
        let req = br#"{"name":"ANTHROPIC_API_KEY",
            "wraps":[{"kind":"device-key-v1","nonce":"wn","wrapped":"wc"}]}"#;
        let Response { status, body } = set_my_secret(&store, req, Some("u-aaa"));
        assert_eq!(status, 400, "{body}");
        assert!(body.contains("nonce") || body.contains("ciphertext"), "{body}");
    }

    #[test]
    fn secrets_policy_round_trips_over_the_wire() {
        let store = scratch("policy-round-trip");

        let Response { status, body } = secrets_policy(&store);
        assert_eq!(status, 200);
        let v: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["enabled"], false);

        let req = br#"{"enabled":true,"overridable":["CLAUDE_CODE_OAUTH_TOKEN"," ","ANTHROPIC_API_KEY"],"missing":"fail","max_lease":"72h"}"#;
        let Response { status, body } = set_secrets_policy(&store, req);
        assert_eq!(status, 200, "{body}");
        let v: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["enabled"], true);
        assert_eq!(
            v["overridable"],
            serde_json::json!(["CLAUDE_CODE_OAUTH_TOKEN", "ANTHROPIC_API_KEY"]),
            "a blank entry is dropped: {body}"
        );
        assert_eq!(v["missing"], "fail");
        assert_eq!(v["max_lease"], "72h");

        let read_back = UserSecretsPolicy::load(store.config());
        assert!(read_back.enabled);
        assert_eq!(read_back.missing, Missing::Fail);

        assert_eq!(
            set_secrets_policy(&store, br#"{"enabled":true,"missing":"nonsense"}"#).status,
            400
        );
    }

    /// Writes a manifest file directly where `list_mine`/`remove_mine` expect to find one, in the
    /// on-disk shape `docs/byo-credentials.md` §8 documents — `Secrets` has no public way to
    /// *write* a value into a user's scope yet (§7: phase 1 refuses `set --user` everywhere), and
    /// `list_mine` never decrypts, so a placeholder `nonce`/`ciphertext` is enough to prove
    /// listing and removal act on what is actually on disk.
    fn set_scoped_for_test(store: &Secrets, user: &str, name: &str) {
        let dir = store.dir().join("users").join(user);
        std::fs::create_dir_all(&dir).expect("mkdir");
        std::fs::write(
            dir.join(format!("{name}.toml")),
            "nonce = \"AA==\"\nciphertext = \"AA==\"\ncreated_at = 1\nupdated_at = 2\n",
        )
        .expect("write a placeholder manifest");
    }
}
