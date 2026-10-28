//! The "bring your own credentials" policy layer (`docs/byo-credentials.md` §4): the server's
//! `secrets/settings.toml` and each user's own `secrets/users/<u>/.settings.toml`. Nothing here
//! lets a user *store* a value (that is [`crate::Scope::User`] plus phase 2's device wraps) — this
//! is only the two policy files that decide whether a stored value is ever read.
//!
//! ```toml
//! # secrets/settings.toml
//! [user_secrets]
//! enabled     = true
//! overridable = ["CLAUDE_CODE_OAUTH_TOKEN", "ANTHROPIC_API_KEY"]
//! missing     = "server"   # server | wait | fail
//! max_lease   = "72h"      # optional; absent = no cap
//! ```
//!
//! ```toml
//! # secrets/users/<u>/.settings.toml
//! missing = "wait"         # may only be stricter than the server's
//! ```

use serde::{Deserialize, Serialize};

use adi_config::Config;

use crate::error::Result;
use crate::secret::validate_project;
use crate::{SECRETS_MODULE, USERS_DIR};

/// The server policy file, read fresh on every launch and every turn (§4.1) rather than cached:
/// an operator who narrows `overridable` expects the very next turn to obey it.
const POLICY_FILE: &str = "settings.toml";

/// A per-user policy file. The leading dot is the fix for §15 Q1: a secret's key name must start
/// with an ASCII letter or `_` ([`crate::secret::validate_name`]), and a dot is neither, so no
/// secret a user could ever `set --user` can collide with this file — structurally, not by
/// refusing `settings` as an `overridable` name, which an operator could still add by mistake.
const USER_SETTINGS_FILE: &str = ".settings.toml";

/// What a turn does when it would use a user's value for a name and no live lease holds one
/// (`docs/byo-credentials.md` §4.1, §10). Declared `Server < Wait < Fail` on purpose: deriving
/// [`Ord`] off that order is what makes [`effective_missing`] "the stricter of the two" a one-line
/// `max`, and it is also the order the spec states its reason in — `fail` ranks above `wait`
/// because it is the one that also stops work piling up behind a missing value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Missing {
    /// Use the server's attached value for that name; the hold key for the turn is unqualified.
    #[default]
    Server,
    /// Park the turn: it waits on the next unlock.
    Wait,
    /// Refuse the turn and say why.
    Fail,
}

/// The stricter of the server's `missing` and a user's own override (`docs/byo-credentials.md`
/// §4.2). `None` — the user has set no preference — reads as "use the server's outright". A user
/// can tighten (`wait`/`fail` under a server `server`) but never loosen a server that already
/// says `wait` or `fail`, because `max` can only move *up* [`Missing`]'s order.
#[must_use]
pub fn effective_missing(server: Missing, user: Option<Missing>) -> Missing {
    match user {
        Some(user) => server.max(user),
        None => server,
    }
}

/// `secrets/settings.toml`'s `[user_secrets]` table — the one switch that turns the whole feature
/// on, and the one list that says which env-var names a user may ever replace.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct UserSecretsPolicy {
    /// Whether this server takes user values at all. `false` ignores the user layer outright:
    /// refuses every unlock and `secrets/mine/set`, and leaves any value already stored on disk
    /// unused.
    pub enabled: bool,
    /// The env-var names a user may supply. Empty by default, so turning `enabled` on changes
    /// nothing until the operator names something.
    pub overridable: Vec<String>,
    /// What a run does when it would use a user's value and none is live (§10).
    pub missing: Missing,
    /// The longest lease a device may ask for, in the platform's duration spelling (`"72h"`,
    /// `"30m"`) — see [`adi_config::parse_duration`]. Absent means no cap; a device's `"forever"`
    /// then really is forever rather than being clamped to anything.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_lease: Option<String>,
}

impl UserSecretsPolicy {
    /// Read the server's policy, fresh. **A missing file reads as the defaults, silently** — that
    /// is just a server that has never turned this on. **An unreadable file is different**: it is
    /// logged, and reads as the defaults too — which, since [`UserSecretsPolicy::default`] has
    /// `enabled: false`, means the feature is off until the file is fixed. Matches
    /// [`LlmSettings::load`](https://docs.rs/adi-agents) ("a corrupt file reads as its defaults")
    /// and is the decided reading of "closed" (§4.1): when the server can't tell which names a
    /// user may replace, it replaces none.
    #[must_use]
    pub fn load(config: &Config) -> Self {
        let file = config.module(SECRETS_MODULE).file::<Self>(POLICY_FILE);
        match file.load_or_default() {
            Ok(policy) => policy,
            Err(error) => {
                tracing::warn!(
                    path = %file.path().display(),
                    %error,
                    "secrets/settings.toml did not parse — the user-secrets feature reads as disabled until it's fixed"
                );
                Self::default()
            }
        }
    }

    /// Persist the policy — `adi-mono secrets policy --enable …` / `POST /api/secrets/policy`.
    ///
    /// # Errors
    /// [`Error::Config`] if the file cannot be written.
    pub fn save(&self, config: &Config) -> Result<()> {
        config
            .module(SECRETS_MODULE)
            .file(POLICY_FILE)
            .save(self)?;
        Ok(())
    }

    /// [`max_lease`](Self::max_lease) parsed to seconds, or `None` when it is absent or not a
    /// duration this platform's spelling recognizes — the same "unreadable means no cap" choice
    /// [`adi_config::parse_duration`]'s callers already make elsewhere.
    #[must_use]
    pub fn max_lease_secs(&self) -> Option<u64> {
        self.max_lease.as_deref().and_then(adi_config::parse_duration)
    }
}

/// `secrets/users/<u>/.settings.toml` — one user's own, strictly-tightening-only override of
/// [`UserSecretsPolicy::missing`] (§4.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct UserSettings {
    /// This user's own `missing`, combined with the server's through [`effective_missing`].
    /// `None` is "no preference": the server's own setting governs outright.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub missing: Option<Missing>,
}

impl UserSettings {
    /// Read one user's own policy, fresh. **A missing file reads as `UserSettings::default()`**
    /// (no preference: the server's `missing` applies outright) — that is just a user who has
    /// never set one. **An unreadable file is read the same way, but logged** (§15 Q15): the
    /// user's leased work defers to the server's policy rather than stopping, because a typo in a
    /// file this server's panel doesn't even expose yet must not be the reason a user's runs wait
    /// or fail on their behalf. An invalid user id reads the same way too — `load` never panics or
    /// propagates an error, since every caller on the launch path needs a plain value.
    #[must_use]
    pub fn load(config: &Config, user: &str) -> Self {
        let Ok(file) = Self::file(config, user) else {
            return Self::default();
        };
        match file.load_or_default() {
            Ok(settings) => settings,
            Err(error) => {
                tracing::warn!(
                    user,
                    path = %file.path().display(),
                    %error,
                    "a user's .settings.toml did not parse — the server's own policy applies until it's fixed"
                );
                Self::default()
            }
        }
    }

    /// Persist one user's own policy — `adi-mono secrets policy --user … --missing wait` /
    /// `POST /api/secrets/mine/settings`.
    ///
    /// # Errors
    /// [`Error::InvalidName`] for an unsafe user id, or [`Error::Config`] on a write failure.
    pub fn save(&self, config: &Config, user: &str) -> Result<()> {
        Self::file(config, user)?.save(self)?;
        Ok(())
    }

    fn file(config: &Config, user: &str) -> Result<adi_config::ConfigFile<Self>> {
        validate_project(user)?;
        Ok(config
            .module(SECRETS_MODULE)
            .file(&format!("{USERS_DIR}/{user}/{USER_SETTINGS_FILE}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> Config {
        let root = std::env::temp_dir().join(format!(
            "adi-secrets-settings-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id(),
        ));
        let _ = std::fs::remove_dir_all(&root);
        Config::with_root(root)
    }

    // -- Missing's order and effective_missing --------------------------------------------

    #[test]
    fn missing_is_ordered_server_then_wait_then_fail() {
        assert!(Missing::Server < Missing::Wait);
        assert!(Missing::Wait < Missing::Fail);
        assert!(Missing::Server < Missing::Fail);
    }

    #[test]
    fn effective_missing_is_the_stricter_of_the_two_and_never_loosens_the_server() {
        assert_eq!(effective_missing(Missing::Server, None), Missing::Server);
        assert_eq!(
            effective_missing(Missing::Server, Some(Missing::Wait)),
            Missing::Wait,
            "a user may tighten a lenient server"
        );
        assert_eq!(
            effective_missing(Missing::Fail, Some(Missing::Server)),
            Missing::Fail,
            "a user may never loosen a strict server"
        );
        assert_eq!(
            effective_missing(Missing::Wait, Some(Missing::Fail)),
            Missing::Fail,
            "fail outranks wait in both directions"
        );
    }

    // -- the server policy -----------------------------------------------------------------

    #[test]
    fn a_fresh_store_has_the_feature_off_with_empty_overridable() {
        let config = scratch("fresh-policy");
        let policy = UserSecretsPolicy::load(&config);
        assert!(!policy.enabled);
        assert!(policy.overridable.is_empty());
        assert_eq!(policy.missing, Missing::Server);
        assert_eq!(policy.max_lease, None);
        assert!(
            !config
                .module(SECRETS_MODULE)
                .file::<UserSecretsPolicy>(POLICY_FILE)
                .exists(),
            "reading a missing policy must not materialize one — every launch and turn reads it"
        );
    }

    #[test]
    fn the_policy_round_trips_and_parses_max_lease() {
        let config = scratch("policy-roundtrip");
        let policy = UserSecretsPolicy {
            enabled: true,
            overridable: vec!["CLAUDE_CODE_OAUTH_TOKEN".to_string()],
            missing: Missing::Fail,
            max_lease: Some("72h".to_string()),
        };
        policy.save(&config).expect("save");

        let read = UserSecretsPolicy::load(&config);
        assert_eq!(read, policy);
        assert_eq!(read.max_lease_secs(), Some(72 * 3600));
    }

    #[test]
    fn a_corrupt_policy_reads_as_disabled_defaults() {
        let config = scratch("policy-corrupt");
        let module = config.module(SECRETS_MODULE);
        module.ensure_dir().expect("mkdir");
        std::fs::write(module.dir().join(POLICY_FILE), "not = [ toml").expect("write garbage");

        let policy = UserSecretsPolicy::load(&config);
        assert_eq!(policy, UserSecretsPolicy::default());
        assert!(!policy.enabled, "an unreadable policy fails closed");
    }

    // -- the per-user settings file ----------------------------------------------------------

    #[test]
    fn a_users_settings_file_lives_beside_their_secrets_with_a_leading_dot() {
        let config = scratch("user-settings-path");
        let file = UserSettings::file(&config, "u-aaa").expect("file");
        assert!(
            file.path().ends_with("secrets/users/u-aaa/.settings.toml"),
            "{}",
            file.path().display()
        );

        // §15 Q1: a secret literally named `settings` lives at a different path, so the two can
        // never collide.
        let secrets = crate::Secrets::with_config(config);
        let secret_file = secrets
            .manifest_file(crate::Scope::User("u-aaa"), "settings")
            .expect("manifest file");
        assert_ne!(secret_file.path(), file.path());
        assert!(secret_file.path().ends_with("users/u-aaa/settings.toml"));
    }

    #[test]
    fn a_missing_users_settings_file_has_no_preference() {
        let config = scratch("user-settings-missing");
        assert_eq!(UserSettings::load(&config, "u-aaa"), UserSettings::default());
        assert_eq!(UserSettings::load(&config, "u-aaa").missing, None);
    }

    #[test]
    fn a_users_settings_file_round_trips() {
        let config = scratch("user-settings-roundtrip");
        let settings = UserSettings {
            missing: Some(Missing::Wait),
        };
        settings.save(&config, "u-aaa").expect("save");
        assert_eq!(UserSettings::load(&config, "u-aaa"), settings);

        // A different user's file is untouched.
        assert_eq!(
            UserSettings::load(&config, "u-bbb"),
            UserSettings::default()
        );
    }

    #[test]
    fn a_corrupt_users_settings_file_defers_to_the_server_policy() {
        let config = scratch("user-settings-corrupt");
        let dir = config.module(SECRETS_MODULE).dir().join(USERS_DIR).join("u-aaa");
        std::fs::create_dir_all(&dir).expect("mkdir");
        std::fs::write(dir.join(USER_SETTINGS_FILE), "not = [ toml").expect("write garbage");

        let settings = UserSettings::load(&config, "u-aaa");
        assert_eq!(settings.missing, None, "§15 Q15: unreadable reads as no preference");
    }

    #[test]
    fn an_invalid_user_id_reads_as_no_preference_rather_than_panicking() {
        let config = scratch("user-settings-invalid-id");
        assert_eq!(
            UserSettings::load(&config, "../escape"),
            UserSettings::default()
        );
    }
}
