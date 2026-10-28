//! The lease reference a session carries — `docs/byo-credentials.md` §11.
//!
//! Phase 1 ships this and nothing that acts on it: the reference is computed, written once, and
//! read back, but no turn is actually provisioned with a user's value yet (that is phase 1's own
//! layering item, P1.8, and phases 2–3 beyond it). A session with a lease reference and
//! `[user_secrets] enabled = false` behaves exactly like one with none.

use serde::{Deserialize, Serialize};

/// Whose lease a session's turns look for, and which names they may use from it — resolved once
/// at [`crate::store::SessionStore::create_as`] and never re-derived, the same reason
/// [`crate::RunOverrides`] and a pinned backend chain aren't either: a conversation is answered by
/// whatever *started* it, not by whatever the agent or the server policy says today.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LeaseRef {
    /// The minted user id whose lease this is (`docs/byo-credentials.md` §3.2).
    pub user: String,
    /// `attached ∩ overridable` at creation — the agent's `[[secrets]]` names that were also on
    /// the server's `overridable` list the moment this session opened. A later turn narrows this
    /// further (§5: also against the agent's *current* attachments, the server's *current*
    /// `overridable`, and the live lease's shared names) but never widens it — a name absent here
    /// can never later apply to this session.
    pub names: Vec<String>,
}

/// Where a session's lease asks for a user's **live** value (`docs/byo-credentials.md` §5, §9) —
/// `adi-app`'s own in-memory `Lease` table, once phase 2 (P2.3) builds one. Object-safe on purpose:
/// nothing in this crate may depend on `adi-app`, so the only way the layer below
/// ([`apply`]) and the Test button ([`crate::llm::ondemand::resolve_credential`]) can be exercised
/// before that table exists is against a fake standing in for it — which is exactly what
/// [`crate::Agents::with_lease_values`] lets a test install, and what nothing in production does
/// yet: every `Agents` opened today carries `None` here, so the layer never applies.
pub trait LeaseValues: Send + Sync {
    /// `user`'s live value for `name`, or `None` when their lease isn't live right now or doesn't
    /// currently hold that name.
    fn value(&self, user: &str, name: &str) -> Option<String>;
}

/// Whether `name` is live-supplied by `lease`'s user right now: the intersection
/// `docs/byo-credentials.md` §5 states for the environment layer — `lease.names`, the agent's
/// *current* attachments, the server's *current* `overridable`, and a value the live source
/// actually holds. Each can only narrow, never widen.
fn live_for(
    name: &str,
    lease: &LeaseRef,
    policy: &adi_secrets::UserSecretsPolicy,
    attached: &std::collections::BTreeSet<&str>,
    live: &dyn LeaseValues,
) -> Option<String> {
    if !policy.enabled
        || !attached.contains(name)
        || !policy.overridable.iter().any(|o| o == name)
        || !lease.names.iter().any(|n| n == name)
    {
        return None;
    }
    live.value(&lease.user, name)
}

/// Overlay a session's lease onto an agent's attached-secret environment
/// (`docs/byo-credentials.md` §5, P1.8): a user value replaces the attached secret of the same
/// name, in the same slot the ordering gives a secret — before the database pointer, the
/// workspace vars, and the agent's own `[env]`, every one of which still wins over it exactly as
/// each already wins over a secret. Never adds a name the agent doesn't already attach: `name`
/// has to be in `attachments` first, whether or not the secret itself resolved (a missing or
/// undecryptable secret still leaves the name's *slot* in the ordering for the user's value to
/// fill).
///
/// `lease.names` is only the starting point — pinned at session creation, it can only ever
/// *narrow* from there (§5): the agent's *current* attachments and the server's *current*
/// `overridable` list are both re-read on every call, so an attachment removed mid-conversation,
/// or a name an operator dropped from the policy, stops applying from the very next turn.
pub(crate) fn apply(
    env: &mut Vec<(String, String)>,
    lease: Option<&LeaseRef>,
    config: &adi_config::Config,
    attachments: &[crate::SecretAttachment],
    live: Option<&dyn LeaseValues>,
) {
    let (Some(lease), Some(live)) = (lease, live) else {
        return;
    };
    let policy = adi_secrets::UserSecretsPolicy::load(config);
    if !policy.enabled {
        return;
    }
    let attached: std::collections::BTreeSet<&str> =
        attachments.iter().map(|a| a.name.as_str()).collect();
    for name in &lease.names {
        let Some(value) = live_for(name, lease, &policy, &attached, live) else {
            continue;
        };
        match env.iter_mut().find(|(key, _)| key == name) {
            Some(slot) => slot.1 = value,
            None => env.push((name.clone(), value)),
        }
    }
}

/// The user qualifier `docs/byo-credentials.md` §6 means by "a user value supplied this turn's
/// credential": `Some(lease.user)` when `name` — the one env var that decides a backend's
/// `credential()` — is itself live-supplied right now, under the same four-way check [`apply`]
/// makes per name. `None` whenever there is no lease, no live source, or `name` fails any part of
/// that check — including a settings-file backend, which has no `name` to ask about at all
/// (`credential_env_name` answers `None` for one, and the caller never calls this then).
pub(crate) fn qualifier_for(
    name: &str,
    lease: Option<&LeaseRef>,
    config: &adi_config::Config,
    attachments: &[crate::SecretAttachment],
    live: Option<&dyn LeaseValues>,
) -> Option<String> {
    let lease = lease?;
    let live = live?;
    let policy = adi_secrets::UserSecretsPolicy::load(config);
    let attached: std::collections::BTreeSet<&str> =
        attachments.iter().map(|a| a.name.as_str()).collect();
    live_for(name, lease, &policy, &attached, live).map(|_| lease.user.clone())
}

/// Whether a turn may start at all, for a session whose lease is `lease`
/// (`docs/byo-credentials.md` §10, ADI-MONO-181 P2.7) — asked *before* a turn starts, never while
/// building its environment. `Ready` covers every session with no lease, every lease whose
/// narrowed names (the same four-way check [`apply`] makes, minus the agent's current
/// `[env]`, which has no bearing on whether a *secret* slot is filled) already have a live value,
/// and a missing value under `missing = "server"` — in that last case the turn starts exactly as
/// [`apply`] already leaves it, on the server's own value, with nothing to gate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Readiness {
    Ready,
    /// Park the turn: `missing = "wait"`, and `names` have no live value right now.
    Wait { user: String, names: Vec<String> },
    /// Refuse the turn and say why: `missing = "fail"`, and `names` have no live value right now.
    Fail { user: String, names: Vec<String> },
}

pub(crate) fn readiness(
    lease: Option<&LeaseRef>,
    config: &adi_config::Config,
    attachments: &[crate::SecretAttachment],
    live: Option<&dyn LeaseValues>,
) -> Readiness {
    let Some(lease) = lease else {
        return Readiness::Ready;
    };
    let policy = adi_secrets::UserSecretsPolicy::load(config);
    if !policy.enabled {
        return Readiness::Ready;
    }
    let attached: std::collections::BTreeSet<&str> =
        attachments.iter().map(|a| a.name.as_str()).collect();
    // The names this turn would actually ask the lease for, narrowed exactly as `apply` narrows
    // them, that have no live value right now.
    let missing: Vec<String> = lease
        .names
        .iter()
        .filter(|name| {
            attached.contains(name.as_str())
                && policy.overridable.iter().any(|o| o == *name)
                && live.is_none_or(|live| live.value(&lease.user, name).is_none())
        })
        .cloned()
        .collect();
    if missing.is_empty() {
        return Readiness::Ready;
    }
    let user_settings = adi_secrets::UserSettings::load(config, &lease.user);
    match adi_secrets::effective_missing(policy.missing, user_settings.missing) {
        adi_secrets::Missing::Server => Readiness::Ready,
        adi_secrets::Missing::Wait => Readiness::Wait { user: lease.user.clone(), names: missing },
        adi_secrets::Missing::Fail => Readiness::Fail { user: lease.user.clone(), names: missing },
    }
}

/// Whether `user`'s lease currently holds a value for `name` the server's policy still allows —
/// the Test button's own check (`docs/byo-credentials.md` §5, P1.10), which has no agent and so no
/// attachment allowlist to intersect against the way [`apply`]'s per-name check does.
pub(crate) fn live_value_for_asker(
    config: &adi_config::Config,
    user: &str,
    live: &dyn LeaseValues,
    name: &str,
) -> Option<String> {
    let policy = adi_secrets::UserSecretsPolicy::load(config);
    if !policy.enabled || !policy.overridable.iter().any(|o| o == name) {
        return None;
    }
    live.value(user, name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SecretAttachment;

    fn scratch(tag: &str) -> adi_config::Config {
        let root = std::env::temp_dir().join(format!(
            "adi-agents-lease-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id(),
        ));
        let _ = std::fs::remove_dir_all(&root);
        adi_config::Config::with_root(root)
    }

    fn enable(config: &adi_config::Config, overridable: &[&str]) {
        adi_secrets::UserSecretsPolicy {
            enabled: true,
            overridable: overridable.iter().map(ToString::to_string).collect(),
            ..adi_secrets::UserSecretsPolicy::default()
        }
        .save(config)
        .expect("save policy");
    }

    fn attachment(name: &str) -> SecretAttachment {
        SecretAttachment {
            project: None,
            name: name.to_string(),
        }
    }

    /// An in-memory stand-in for `adi-app`'s own `Lease` table (phase 2) — a fixed map of
    /// `user -> name -> value`, which is all [`apply`]/[`qualifier_for`] need to be exercised
    /// ahead of phase 2 (`docs/byo-credentials.md` §5, P1.8).
    #[derive(Debug, Default)]
    struct FakeLease(std::collections::BTreeMap<(String, String), String>);

    impl FakeLease {
        fn with(mut self, user: &str, name: &str, value: &str) -> Self {
            self.0
                .insert((user.to_string(), name.to_string()), value.to_string());
            self
        }
    }

    impl LeaseValues for FakeLease {
        fn value(&self, user: &str, name: &str) -> Option<String> {
            self.0.get(&(user.to_string(), name.to_string())).cloned()
        }
    }

    #[test]
    fn a_live_user_value_replaces_the_attached_secret_of_the_same_name() {
        let config = scratch("apply-replace");
        enable(&config, &["CLAUDE_CODE_OAUTH_TOKEN"]);
        let lease = LeaseRef {
            user: "u-aaa".into(),
            names: vec!["CLAUDE_CODE_OAUTH_TOKEN".into()],
        };
        let attachments = vec![attachment("CLAUDE_CODE_OAUTH_TOKEN")];
        let live = FakeLease::default().with("u-aaa", "CLAUDE_CODE_OAUTH_TOKEN", "sk-mine");
        let mut env = vec![("CLAUDE_CODE_OAUTH_TOKEN".to_string(), "sk-server".to_string())];

        apply(&mut env, Some(&lease), &config, &attachments, Some(&live));

        assert_eq!(
            env,
            vec![("CLAUDE_CODE_OAUTH_TOKEN".to_string(), "sk-mine".to_string())]
        );
    }

    /// A name the agent doesn't attach never appears, even with a live value and a lease that
    /// names it — a user value replaces a secret an agent already attaches and never adds one
    /// (§2, §5).
    #[test]
    fn a_name_the_agent_does_not_attach_is_never_added() {
        let config = scratch("apply-no-add");
        enable(&config, &["CLAUDE_CODE_OAUTH_TOKEN"]);
        let lease = LeaseRef {
            user: "u-aaa".into(),
            names: vec!["CLAUDE_CODE_OAUTH_TOKEN".into()],
        };
        let live = FakeLease::default().with("u-aaa", "CLAUDE_CODE_OAUTH_TOKEN", "sk-mine");
        let mut env = Vec::new();

        apply(&mut env, Some(&lease), &config, &[], Some(&live));

        assert!(env.is_empty());
    }

    /// A name the agent attaches but the secret itself failed to resolve still gets the user's
    /// value: the allowlist is what decides the slot, not whether the server's own secret
    /// happened to be there.
    #[test]
    fn a_missing_secret_still_gets_the_users_value_in_its_slot() {
        let config = scratch("apply-missing-secret");
        enable(&config, &["CLAUDE_CODE_OAUTH_TOKEN"]);
        let lease = LeaseRef {
            user: "u-aaa".into(),
            names: vec!["CLAUDE_CODE_OAUTH_TOKEN".into()],
        };
        let attachments = vec![attachment("CLAUDE_CODE_OAUTH_TOKEN")];
        let live = FakeLease::default().with("u-aaa", "CLAUDE_CODE_OAUTH_TOKEN", "sk-mine");
        let mut env = Vec::new();

        apply(&mut env, Some(&lease), &config, &attachments, Some(&live));

        assert_eq!(
            env,
            vec![("CLAUDE_CODE_OAUTH_TOKEN".to_string(), "sk-mine".to_string())]
        );
    }

    /// The four narrowing checks, one at a time: a name dropped from the server's `overridable`
    /// list, a name the live lease has nothing for, and `[user_secrets] enabled = false` each stop
    /// the overlay on their own — §5's "each can only narrow", and the default-off behaviour every
    /// existing agent must keep.
    #[test]
    fn each_narrowing_check_stops_the_overlay_alone() {
        let attachments = vec![attachment("CLAUDE_CODE_OAUTH_TOKEN")];
        let lease = LeaseRef {
            user: "u-aaa".into(),
            names: vec!["CLAUDE_CODE_OAUTH_TOKEN".into()],
        };
        let live = FakeLease::default().with("u-aaa", "CLAUDE_CODE_OAUTH_TOKEN", "sk-mine");

        // Not on the server's overridable list.
        let config = scratch("apply-narrow-overridable");
        enable(&config, &[]);
        let mut env = vec![("CLAUDE_CODE_OAUTH_TOKEN".to_string(), "sk-server".to_string())];
        apply(&mut env, Some(&lease), &config, &attachments, Some(&live));
        assert_eq!(env[0].1, "sk-server");

        // The feature is off.
        let config = scratch("apply-narrow-disabled");
        let mut env = vec![("CLAUDE_CODE_OAUTH_TOKEN".to_string(), "sk-server".to_string())];
        apply(&mut env, Some(&lease), &config, &attachments, Some(&live));
        assert_eq!(env[0].1, "sk-server");

        // The live lease has nothing for this name.
        let config = scratch("apply-narrow-no-live-value");
        enable(&config, &["CLAUDE_CODE_OAUTH_TOKEN"]);
        let empty_live = FakeLease::default();
        let mut env = vec![("CLAUDE_CODE_OAUTH_TOKEN".to_string(), "sk-server".to_string())];
        apply(&mut env, Some(&lease), &config, &attachments, Some(&empty_live));
        assert_eq!(env[0].1, "sk-server");

        // No live source at all — every `Agents` nothing has called `with_lease_values` on.
        let config = scratch("apply-narrow-no-source");
        enable(&config, &["CLAUDE_CODE_OAUTH_TOKEN"]);
        let mut env = vec![("CLAUDE_CODE_OAUTH_TOKEN".to_string(), "sk-server".to_string())];
        apply(&mut env, Some(&lease), &config, &attachments, None);
        assert_eq!(env[0].1, "sk-server");

        // No lease at all — the ordinary case for every run today.
        let mut env = vec![("CLAUDE_CODE_OAUTH_TOKEN".to_string(), "sk-server".to_string())];
        apply(&mut env, None, &config, &attachments, Some(&live));
        assert_eq!(env[0].1, "sk-server");
    }

    /// A name the agent stopped attaching mid-conversation stops getting the user's value from the
    /// very next call — the attachment list is re-read fresh, never trusted from `lease.names`
    /// alone (§5: "a name the agent stopped attaching mid-conversation stops getting the user's
    /// value").
    #[test]
    fn dropping_the_attachment_mid_conversation_stops_the_overlay() {
        let config = scratch("apply-narrow-dropped-attachment");
        enable(&config, &["CLAUDE_CODE_OAUTH_TOKEN"]);
        let lease = LeaseRef {
            user: "u-aaa".into(),
            names: vec!["CLAUDE_CODE_OAUTH_TOKEN".into()],
        };
        let live = FakeLease::default().with("u-aaa", "CLAUDE_CODE_OAUTH_TOKEN", "sk-mine");

        let mut env = vec![("CLAUDE_CODE_OAUTH_TOKEN".to_string(), "sk-server".to_string())];
        apply(&mut env, Some(&lease), &config, &[attachment("CLAUDE_CODE_OAUTH_TOKEN")], Some(&live));
        assert_eq!(env[0].1, "sk-mine", "applies while the agent still attaches it");

        let mut env = vec![("CLAUDE_CODE_OAUTH_TOKEN".to_string(), "sk-server".to_string())];
        apply(&mut env, Some(&lease), &config, &[], Some(&live));
        assert_eq!(env[0].1, "sk-server", "stops the moment the attachment is gone");
    }

    #[test]
    fn qualifier_for_names_the_lease_user_only_when_its_value_is_live() {
        let config = scratch("qualifier-live");
        enable(&config, &["CLAUDE_CODE_OAUTH_TOKEN"]);
        let lease = LeaseRef {
            user: "u-aaa".into(),
            names: vec!["CLAUDE_CODE_OAUTH_TOKEN".into()],
        };
        let attachments = vec![attachment("CLAUDE_CODE_OAUTH_TOKEN")];
        let live = FakeLease::default().with("u-aaa", "CLAUDE_CODE_OAUTH_TOKEN", "sk-mine");

        assert_eq!(
            qualifier_for(
                "CLAUDE_CODE_OAUTH_TOKEN",
                Some(&lease),
                &config,
                &attachments,
                Some(&live)
            ),
            Some("u-aaa".to_string())
        );
        assert_eq!(
            qualifier_for("CLAUDE_CODE_OAUTH_TOKEN", None, &config, &attachments, Some(&live)),
            None,
            "no lease, no qualifier — the server's own login"
        );
        assert_eq!(
            qualifier_for(
                "ANTHROPIC_API_KEY",
                Some(&lease),
                &config,
                &attachments,
                Some(&live)
            ),
            None,
            "a different name was never supplied by this lease"
        );
    }

    // -- readiness (ADI-MONO-181 P2.7) --------------------------------------------------------

    #[test]
    fn readiness_is_ready_with_no_lease_or_a_fully_live_one() {
        let config = scratch("readiness-ready");
        enable(&config, &["K"]);
        let attachments = vec![attachment("K")];
        let live = FakeLease::default().with("u-aaa", "K", "v");
        assert_eq!(readiness(None, &config, &attachments, Some(&live)), Readiness::Ready);

        let lease = LeaseRef { user: "u-aaa".into(), names: vec!["K".into()] };
        assert_eq!(
            readiness(Some(&lease), &config, &attachments, Some(&live)),
            Readiness::Ready,
            "a live value for every narrowed name needs no gate"
        );
    }

    #[test]
    fn readiness_is_ready_under_missing_server_however_little_is_live() {
        let config = scratch("readiness-server");
        enable(&config, &["K"]); // `missing` defaults to `server`
        let lease = LeaseRef { user: "u-aaa".into(), names: vec!["K".into()] };
        let empty_live = FakeLease::default();
        assert_eq!(
            readiness(Some(&lease), &config, &[attachment("K")], Some(&empty_live)),
            Readiness::Ready,
            "server is apply's own fallback, nothing to gate"
        );
    }

    #[test]
    fn readiness_waits_or_fails_on_the_effective_missing_only_for_names_actually_missing() {
        let config = scratch("readiness-wait-fail");
        adi_secrets::UserSecretsPolicy {
            enabled: true,
            overridable: vec!["K".to_string()],
            missing: adi_secrets::Missing::Wait,
            ..adi_secrets::UserSecretsPolicy::default()
        }
        .save(&config)
        .expect("save policy");
        let lease = LeaseRef { user: "u-aaa".into(), names: vec!["K".into()] };
        let empty_live = FakeLease::default();
        assert_eq!(
            readiness(Some(&lease), &config, &[attachment("K")], Some(&empty_live)),
            Readiness::Wait { user: "u-aaa".to_string(), names: vec!["K".to_string()] }
        );

        adi_secrets::UserSettings { missing: Some(adi_secrets::Missing::Fail) }
            .save(&config, "u-aaa")
            .expect("save user settings");
        assert_eq!(
            readiness(Some(&lease), &config, &[attachment("K")], Some(&empty_live)),
            Readiness::Fail { user: "u-aaa".to_string(), names: vec!["K".to_string()] },
            "a user's own setting may tighten the server's wait into a fail"
        );
    }

    #[test]
    fn readiness_ignores_a_name_the_narrowing_already_drops() {
        let config = scratch("readiness-narrowed-away");
        adi_secrets::UserSecretsPolicy {
            enabled: true,
            overridable: vec!["K".to_string()],
            missing: adi_secrets::Missing::Fail,
            ..adi_secrets::UserSecretsPolicy::default()
        }
        .save(&config)
        .expect("save policy");
        let lease = LeaseRef { user: "u-aaa".into(), names: vec!["K".into()] };
        let empty_live = FakeLease::default();
        // The agent no longer attaches K — `apply` would leave the server's value alone, so there
        // is nothing for this turn to wait or fail on either.
        assert_eq!(
            readiness(Some(&lease), &config, &[], Some(&empty_live)),
            Readiness::Ready
        );
    }

    #[test]
    fn live_value_for_asker_needs_no_agent_only_the_server_policy() {
        let config = scratch("asker-value");
        enable(&config, &["CLAUDE_CODE_OAUTH_TOKEN"]);
        let live = FakeLease::default().with("u-aaa", "CLAUDE_CODE_OAUTH_TOKEN", "sk-mine");

        assert_eq!(
            live_value_for_asker(&config, "u-aaa", &live, "CLAUDE_CODE_OAUTH_TOKEN"),
            Some("sk-mine".to_string())
        );
        assert_eq!(
            live_value_for_asker(&config, "u-aaa", &live, "ANTHROPIC_API_KEY"),
            None,
            "not on the server's overridable list"
        );
        assert_eq!(
            live_value_for_asker(&config, "u-bbb", &live, "CLAUDE_CODE_OAUTH_TOKEN"),
            None,
            "a different user's lease has nothing for this name"
        );
    }
}
