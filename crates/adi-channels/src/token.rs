//! Where a node token (`docs/channels.md` §1) lives: one per `(node, provider)`, shared across
//! every connection this node holds on that provider, in `adi-secrets`' reserved-scope mechanism —
//! the same one a fleet node's own password uses (`adi-app/src/viewer.rs`'s `CREDENTIAL_SCOPE`).
//! `"channels-nodes"` is never a real `adi-projects` id, so [`adi_secrets::Secrets::resolve`] —
//! the path that fills an agent run's environment — never surfaces it: this is metadata the router
//! client reads, never a secret an agent is handed.

use adi_secrets::Secrets;

use crate::error::Result;

const SCOPE: &str = "channels-nodes";

/// Store (or replace) the node token for `provider`.
///
/// # Errors
/// Whatever `adi_secrets::Secrets::set` returns — a crypto or write failure.
pub fn save(secrets: &Secrets, provider: &str, token: &str) -> Result<()> {
    secrets.set(Some(SCOPE), provider, token, Some("channel-router node token"))?;
    Ok(())
}

/// The node token for `provider`, if this node has registered one.
///
/// # Errors
/// Whatever `adi_secrets::Secrets::reveal` returns — a crypto or read failure.
pub fn load(secrets: &Secrets, provider: &str) -> Result<Option<String>> {
    Ok(secrets.reveal(Some(SCOPE), provider)?)
}

/// Drop the stored token for `provider` — called once nothing on this node still needs it
/// (§1 "Revocation": "a node holds one token per provider, shared across every connection").
///
/// # Errors
/// Whatever `adi_secrets::Secrets::remove` returns.
pub fn remove(secrets: &Secrets, provider: &str) -> Result<bool> {
    Ok(secrets.remove(Some(SCOPE), provider)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> Secrets {
        let root = std::env::temp_dir().join(format!(
            "adi-channels-token-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id(),
        ));
        let _ = std::fs::remove_dir_all(&root);
        Secrets::with_config(adi_config::Config::with_root(root))
    }

    #[test]
    fn save_load_and_remove_round_trip() {
        let secrets = scratch("roundtrip");
        assert_eq!(load(&secrets, "telegram").unwrap(), None);
        save(&secrets, "telegram", "tok_abc").unwrap();
        assert_eq!(load(&secrets, "telegram").unwrap(), Some("tok_abc".into()));
        assert!(remove(&secrets, "telegram").unwrap());
        assert_eq!(load(&secrets, "telegram").unwrap(), None);
    }

    /// The whole reason for the reserved scope: the token must never show up where a run's
    /// environment is resolved.
    #[test]
    fn the_token_never_reaches_a_runs_resolved_environment() {
        let secrets = scratch("isolated");
        save(&secrets, "telegram", "tok_secret").unwrap();
        for scope in [None, Some("some-project")] {
            let env = secrets.resolve(scope).unwrap();
            assert!(!env.values().any(|v| v == "tok_secret"));
        }
    }
}
