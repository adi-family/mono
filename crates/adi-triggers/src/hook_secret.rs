//! A webhook trigger's own gate: the shared secret an inbound caller must present (`?secret=`
//! or `X-ADI-Hook-Secret`), kept in [`TriggerManifest::extra`] under one of two keys —
//! [`HOOK_SECRET_EXTRA`] (a plaintext value, inline, kept only for a trigger saved before
//! [`HOOK_SECRET_REF_EXTRA`] existed) or [`HOOK_SECRET_REF_EXTRA`] (the *name* of an
//! [`adi_secrets`] secret, resolved fresh on every request). A reference, once set, always wins
//! over a leftover inline value for the same trigger — see [`reconcile_hook_secret`].

use std::collections::BTreeMap;

use crate::{Trigger, Triggers};

/// The legacy extra key: the hook secret's plaintext value, stored directly in the manifest.
/// Still honored (ADI-MONO-242 keeps old triggers working) but never offered to a new save —
/// every output path redacts it, and `adi-mono triggers migrate-hook-secret` moves it to
/// [`HOOK_SECRET_REF_EXTRA`].
pub const HOOK_SECRET_EXTRA: &str = "secret";

/// The extra key that names an [`adi_secrets`] secret (by key name, not value) to resolve the
/// hook's gate from — project-scoped first, falling back to global, the same precedence
/// [`adi_secrets::Secrets::resolve`] uses. The preferred form: nothing but this name ever
/// touches the manifest, a CLI confirmation line, or the panel.
pub const HOOK_SECRET_REF_EXTRA: &str = "secret_ref";

/// What a webhook trigger's hook secret resolves to, right before a request is gated on it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HookSecret {
    /// Neither extra is set — the hook is open; any caller may fire it.
    Open,
    /// The value to compare a caller's own against (constant-time, by the caller of
    /// [`Triggers::hook_secret`]).
    Value(String),
    /// [`HOOK_SECRET_REF_EXTRA`] names a secret that doesn't resolve in either of the trigger's
    /// scopes — refuse every request rather than falling open.
    Missing,
}

impl Triggers {
    /// Resolve `trigger`'s hook secret. A reference is tried first (project-scoped, then
    /// global — matching [`run_env`](Self::run_env)'s own precedence for a single named
    /// secret), then the legacy inline value, then [`HookSecret::Open`].
    #[must_use]
    pub fn hook_secret(&self, trigger: &Trigger) -> HookSecret {
        if let Some(name) = trigger
            .manifest
            .extra
            .get(HOOK_SECRET_REF_EXTRA)
            .filter(|s| !s.is_empty())
        {
            let secrets = adi_secrets::Secrets::with_config(self.config.clone());
            let project = trigger.manifest.project.as_deref();
            if let Some(p) = project
                && let Ok(Some(value)) = secrets.reveal(Some(p), name)
            {
                return HookSecret::Value(value);
            }
            return match secrets.reveal(None, name) {
                Ok(Some(value)) => HookSecret::Value(value),
                _ => HookSecret::Missing,
            };
        }
        match trigger
            .manifest
            .extra
            .get(HOOK_SECRET_EXTRA)
            .filter(|s| !s.is_empty())
        {
            Some(value) => HookSecret::Value(value.clone()),
            None => HookSecret::Open,
        }
    }
}

/// What `migrate_hook_secret` moved — the secret's own name and scope, never its value, so a
/// caller (the CLI) can report success without ever having the plaintext to accidentally print.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Migrated {
    pub secret_name: String,
    pub project: Option<String>,
}

/// What this crate's own name sanitizer can't fix, or a decision the caller has to make for
/// itself: `migrate_hook_secret`'s own refusals.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MigrateError {
    /// No such trigger.
    NotFound,
    /// The trigger has no inline `secret` to migrate — nothing to do (it's already on a
    /// reference, or it never had one).
    NothingToMigrate,
    /// The target secret name is already set in the trigger's scope — pass a different
    /// `secret_name` rather than silently overwriting an unrelated secret.
    NameTaken(String),
    /// [`adi_secrets`] refused the read or write.
    Secrets(String),
    /// This store refused the manifest write.
    Store(String),
}

impl std::fmt::Display for MigrateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFound => write!(f, "no such trigger"),
            Self::NothingToMigrate => write!(f, "this trigger has no inline hook secret to migrate"),
            Self::NameTaken(name) => write!(
                f,
                "a secret named {name} already exists in this trigger's scope — pass --secret-name to pick another"
            ),
            Self::Secrets(e) | Self::Store(e) => write!(f, "{e}"),
        }
    }
}

/// A trigger name folded into a valid [`adi_secrets`] key: uppercased, every character outside
/// `[A-Za-z0-9_]` becomes `_`, and a leading digit (a trigger name may start with one; a secret
/// name may not) gets a `T` prefix. `HOOK_SECRET_<…>` keeps it out of the way of a name the
/// trigger's own code block might otherwise want for itself.
#[must_use]
pub fn default_secret_name(trigger_name: &str) -> String {
    let mut sanitized: String = trigger_name
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_uppercase() } else { '_' })
        .collect();
    if sanitized.starts_with(|c: char| c.is_ascii_digit()) {
        sanitized.insert(0, 'T');
    }
    format!("HOOK_SECRET_{sanitized}")
}

impl Triggers {
    /// Move a trigger's inline `secret` into a real [`adi_secrets`] secret, in the same scope
    /// the trigger already resolves from (its [`project`](crate::TriggerManifest::project), or
    /// global), and switch the manifest to [`HOOK_SECRET_REF_EXTRA`] — the one-shot migration
    /// ADI-MONO-242 ships so a plaintext value never has to be retyped by hand. Never returns or
    /// logs the value: not here, not in the [`Migrated`] it hands back.
    ///
    /// # Errors
    /// [`MigrateError::NotFound`] for an unregistered trigger, [`MigrateError::NothingToMigrate`]
    /// if it has no inline secret, [`MigrateError::NameTaken`] if the target name is already a
    /// secret in scope, or [`MigrateError::Secrets`]/[`MigrateError::Store`] on a read/write
    /// failure.
    pub fn migrate_hook_secret(&self, name: &str, secret_name: Option<&str>) -> Result<Migrated, MigrateError> {
        let trigger = self
            .get(name)
            .map_err(|e| MigrateError::Store(e.to_string()))?
            .ok_or(MigrateError::NotFound)?;
        let value = trigger
            .manifest
            .extra
            .get(HOOK_SECRET_EXTRA)
            .filter(|s| !s.is_empty())
            .cloned()
            .ok_or(MigrateError::NothingToMigrate)?;
        let secret_name = secret_name.map_or_else(|| default_secret_name(name), str::to_string);
        let project = trigger.manifest.project.clone();

        let secrets = adi_secrets::Secrets::with_config(self.config.clone());
        if secrets
            .get(project.as_deref(), &secret_name)
            .map_err(|e| MigrateError::Secrets(e.to_string()))?
            .is_some()
        {
            return Err(MigrateError::NameTaken(secret_name));
        }
        secrets
            .set(
                project.as_deref(),
                &secret_name,
                &value,
                Some(&format!("migrated from trigger {name}'s inline hook secret")),
            )
            .map_err(|e| MigrateError::Secrets(e.to_string()))?;

        let mut manifest = trigger.manifest;
        manifest.extra.remove(HOOK_SECRET_EXTRA);
        manifest.extra.insert(HOOK_SECRET_REF_EXTRA.to_string(), secret_name.clone());
        self.save(name, manifest).map_err(|e| MigrateError::Store(e.to_string()))?;

        Ok(Migrated { secret_name, project })
    }
}

/// Reconcile an edited trigger's hook-secret extras against what it had before the save — the
/// save path's own chokepoint for the one setting a caller might not have seen to round-trip
/// (the panel never receives the inline value at all, by design; see
/// [`HOOK_SECRET_EXTRA`]'s own doc).
///
/// * `clear` — an explicit "remove the secret" action (the panel's own button): wipe both keys
///   regardless of anything else.
/// * Otherwise, whichever of the two keys `extra` sets explicitly wins outright, and the other
///   is dropped — setting a reference retires a stale inline value rather than leaving it to
///   linger unused on disk.
/// * If `extra` sets neither, the caller touched nothing about the secret (the common case for
///   the panel resaving an unrelated field): carry both forward from `previous_extra` unchanged,
///   the same way `adi_relay::register::keep_unchanged_endpoint` carries `relay_endpoint`
///   forward for a save that never touched the relay section either.
pub fn reconcile_hook_secret(previous_extra: Option<&BTreeMap<String, String>>, clear: bool, extra: &mut BTreeMap<String, String>) {
    if clear {
        extra.remove(HOOK_SECRET_EXTRA);
        extra.remove(HOOK_SECRET_REF_EXTRA);
        return;
    }
    let has_ref = extra.get(HOOK_SECRET_REF_EXTRA).is_some_and(|s| !s.is_empty());
    let has_inline = extra.get(HOOK_SECRET_EXTRA).is_some_and(|s| !s.is_empty());
    if has_ref {
        extra.remove(HOOK_SECRET_EXTRA);
        return;
    }
    if has_inline {
        return;
    }
    if let Some(prev) = previous_extra {
        if let Some(v) = prev.get(HOOK_SECRET_EXTRA).filter(|s| !s.is_empty()) {
            extra.insert(HOOK_SECRET_EXTRA.to_string(), v.clone());
        }
        if let Some(v) = prev.get(HOOK_SECRET_REF_EXTRA).filter(|s| !s.is_empty()) {
            extra.insert(HOOK_SECRET_REF_EXTRA.to_string(), v.clone());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::trigger::{KIND_WEBHOOK, TriggerManifest};
    use adi_config::Config;

    fn store(tag: &str) -> Triggers {
        let root = std::env::temp_dir().join(format!(
            "adi-triggers-hooksecret-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id(),
        ));
        let _ = std::fs::remove_dir_all(&root);
        Triggers::with_config(Config::with_root(root))
    }

    fn webhook(extra: &[(&str, &str)], project: Option<&str>) -> Trigger {
        Trigger {
            name: "hook".to_string(),
            manifest: TriggerManifest {
                kind: KIND_WEBHOOK.to_string(),
                code: "true".to_string(),
                project: project.map(str::to_string),
                extra: extra.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect(),
                ..TriggerManifest::default()
            },
        }
    }

    #[test]
    fn no_extra_is_open() {
        let store = store("open");
        assert_eq!(store.hook_secret(&webhook(&[], None)), HookSecret::Open);
    }

    #[test]
    fn inline_legacy_value_is_honored() {
        let store = store("inline");
        assert_eq!(
            store.hook_secret(&webhook(&[(HOOK_SECRET_EXTRA, "s3cr3t")], None)),
            HookSecret::Value("s3cr3t".to_string())
        );
    }

    #[test]
    fn a_ref_to_a_global_secret_resolves() {
        let store = store("ref-global");
        adi_secrets::Secrets::with_config(store.config().clone())
            .set(None, "MY_HOOK", "from-global", None)
            .expect("set");
        let t = webhook(&[(HOOK_SECRET_REF_EXTRA, "MY_HOOK")], None);
        assert_eq!(store.hook_secret(&t), HookSecret::Value("from-global".to_string()));
    }

    /// A project-scoped secret of the same name overrides the global one — the same precedence
    /// `run_env`/`adi_secrets::resolve` use everywhere else.
    #[test]
    fn a_project_scoped_secret_overrides_global_of_the_same_name() {
        let store = store("ref-project-override");
        let secrets = adi_secrets::Secrets::with_config(store.config().clone());
        secrets.set(None, "MY_HOOK", "global-value", None).expect("set global");
        secrets.set(Some("proj"), "MY_HOOK", "project-value", None).expect("set project");
        let t = webhook(&[(HOOK_SECRET_REF_EXTRA, "MY_HOOK")], Some("proj"));
        assert_eq!(store.hook_secret(&t), HookSecret::Value("project-value".to_string()));

        // A sibling project with no override of its own still falls back to global.
        let other = webhook(&[(HOOK_SECRET_REF_EXTRA, "MY_HOOK")], Some("other"));
        assert_eq!(store.hook_secret(&other), HookSecret::Value("global-value".to_string()));
    }

    /// A reference naming a secret that isn't set anywhere must never fall open.
    #[test]
    fn a_ref_to_a_missing_secret_is_missing_not_open() {
        let store = store("ref-missing");
        let t = webhook(&[(HOOK_SECRET_REF_EXTRA, "NO_SUCH_SECRET")], None);
        assert_eq!(store.hook_secret(&t), HookSecret::Missing);
    }

    /// A reference wins over a leftover inline value on the same trigger.
    #[test]
    fn a_ref_wins_over_an_inline_value_set_on_the_same_trigger() {
        let store = store("ref-wins");
        adi_secrets::Secrets::with_config(store.config().clone())
            .set(None, "MY_HOOK", "ref-value", None)
            .expect("set");
        let t = webhook(
            &[(HOOK_SECRET_EXTRA, "inline-value"), (HOOK_SECRET_REF_EXTRA, "MY_HOOK")],
            None,
        );
        assert_eq!(store.hook_secret(&t), HookSecret::Value("ref-value".to_string()));
    }

    #[test]
    fn reconcile_clears_both_keys_when_asked_explicitly() {
        let mut extra = BTreeMap::from([(HOOK_SECRET_EXTRA.to_string(), "s3".to_string())]);
        reconcile_hook_secret(None, true, &mut extra);
        assert!(extra.is_empty());
    }

    /// An explicit new reference retires a stale inline value rather than leaving it on disk.
    #[test]
    fn reconcile_lets_an_explicit_ref_retire_a_stale_inline_value() {
        let previous = BTreeMap::from([(HOOK_SECRET_EXTRA.to_string(), "old-inline".to_string())]);
        let mut extra = BTreeMap::from([(HOOK_SECRET_REF_EXTRA.to_string(), "NEW_NAME".to_string())]);
        reconcile_hook_secret(Some(&previous), false, &mut extra);
        assert_eq!(extra.get(HOOK_SECRET_EXTRA), None);
        assert_eq!(extra.get(HOOK_SECRET_REF_EXTRA).map(String::as_str), Some("NEW_NAME"));
    }

    /// A save that touches neither hook-secret key (the panel resaving some other field) keeps
    /// whatever the trigger already had — the inline value here, since the panel never saw it to
    /// round-trip.
    #[test]
    fn reconcile_carries_forward_an_untouched_inline_value() {
        let previous = BTreeMap::from([(HOOK_SECRET_EXTRA.to_string(), "kept".to_string())]);
        let mut extra = BTreeMap::from([("description".to_string(), "unrelated".to_string())]);
        reconcile_hook_secret(Some(&previous), false, &mut extra);
        assert_eq!(extra.get(HOOK_SECRET_EXTRA).map(String::as_str), Some("kept"));
    }

    /// Same, but for a reference — a resave that touches neither key keeps the ref too.
    #[test]
    fn reconcile_carries_forward_an_untouched_ref() {
        let previous = BTreeMap::from([(HOOK_SECRET_REF_EXTRA.to_string(), "KEPT".to_string())]);
        let mut extra = BTreeMap::new();
        reconcile_hook_secret(Some(&previous), false, &mut extra);
        assert_eq!(extra.get(HOOK_SECRET_REF_EXTRA).map(String::as_str), Some("KEPT"));
    }
}
