//! The on-disk trigger definition ([`TriggerManifest`], serialized as `<name>.toml`) and the
//! name-attached view of a loaded trigger ([`Trigger`]).

use std::collections::BTreeMap;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

/// The well-known trigger kinds. `kind` is stored as a free string (so new kinds don't break
/// older stores), but these are the ones the platform understands today:
///
/// * [`KIND_WEBHOOK`] — fired by an HTTP call to the app's `/api/hooks/<name>` endpoint; the
///   request body becomes the payload. Extras: `secret` (optional shared secret the caller must
///   pass as a `?secret=` query parameter).
/// * [`KIND_TELEGRAM`] — fired by a Telegram bot update. The listener runtime is future work;
///   the definition (extras: `token_env`, `chat_id`) is stored now and can be fired manually.
/// * [`KIND_CRON`] — fired on a schedule (extra: `schedule`). The scheduler runtime is future
///   work; stored now, manually firable.
/// * [`KIND_MANUAL`] — fired only by hand (UI button / CLI / API).
pub const KIND_WEBHOOK: &str = "webhook";
pub const KIND_TELEGRAM: &str = "telegram";
pub const KIND_CRON: &str = "cron";
pub const KIND_MANUAL: &str = "manual";

/// A reusable trigger definition: *what* fires it (the kind plus its kind-specific extras) and
/// *what runs* when it fires (the shell code block). It says nothing about any live listener —
/// the store is pure data; firing spawns the code block detached (see [`crate::fire`]).
///
/// Unknown fields are ignored so the manifest can gain fields without breaking older stores.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TriggerManifest {
    /// The event source that fires this trigger: `webhook` | `telegram` | `cron` | `manual`.
    pub kind: String,
    /// The code block run when the trigger fires — a shell script executed as `sh -c <code>`,
    /// detached, with `ADI_TRIGGER`/`ADI_TRIGGER_KIND` (and `ADI_PAYLOAD_FILE` when the event
    /// carries a payload) in its environment.
    #[serde(default)]
    pub code: String,
    /// A free-form one-line description shown in lists.
    #[serde(default)]
    pub description: String,
    /// Whether the trigger's event source may fire it. A disabled trigger keeps its definition
    /// and can still be fired manually (an explicit user action), but its external source — the
    /// webhook endpoint, a future listener — refuses.
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    /// The project this trigger is filed under (its [`adi-projects`] id), or `None` for a
    /// global trigger. Pure metadata: it scopes where the trigger shows up (a project's detail
    /// page), not what it may do.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    /// Kind-specific settings not promoted to first-class manifest properties (`secret`,
    /// `schedule`, `token_env`, `chat_id`, …).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub extra: BTreeMap<String, String>,
    /// When the definition was created, as Unix epoch seconds.
    #[serde(default)]
    pub created_at: u64,
    /// When the definition was last saved, as Unix epoch seconds.
    #[serde(default)]
    pub updated_at: u64,
}

impl Default for TriggerManifest {
    /// An empty manifest that is nonetheless *enabled* — a freshly defined trigger should fire
    /// without an extra toggle step, matching the form/CLI defaults.
    fn default() -> Self {
        Self {
            kind: String::new(),
            code: String::new(),
            description: String::new(),
            enabled: true,
            project: None,
            extra: BTreeMap::new(),
            created_at: 0,
            updated_at: 0,
        }
    }
}

/// serde default for [`TriggerManifest::enabled`] — an omitted flag reads as enabled, so
/// manifests written before the flag existed keep firing.
fn default_enabled() -> bool {
    true
}

/// A registered trigger: its name (the file stem under `triggers/`) plus its loaded
/// [`TriggerManifest`]. The name is not stored in the file — it *is* the file. `Serialize` so
/// the CLI/API can emit it; built from disk, never deserialized, so no `Deserialize`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Trigger {
    /// The trigger name — its `<name>.toml` file stem under `~/.adi/mono/triggers/`.
    pub name: String,
    /// The parsed manifest.
    pub manifest: TriggerManifest,
}

/// The current time as Unix epoch seconds (0 if the clock predates the epoch).
#[must_use]
pub(crate) fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Validate a trigger name: a single, filesystem-safe path segment. This is a security boundary —
/// names arrive from the CLI, the HTTP API, *and the public webhook URL path*, and are joined
/// onto the store path as `<name>.toml`, so anything with a separator or `.`/`..` must be
/// rejected.
pub(crate) fn validate_name(name: &str) -> Result<()> {
    let ok = !name.is_empty()
        && name != "."
        && name != ".."
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'));
    if ok {
        Ok(())
    } else {
        Err(Error::InvalidName(name.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_default_manifest_is_enabled() {
        assert!(TriggerManifest::default().enabled);
    }

    #[test]
    fn valid_and_invalid_names() {
        for name in ["deploy-hook", "notify", "hook_2", "a.b"] {
            assert!(validate_name(name).is_ok(), "{name} should be valid");
        }
        for name in ["", ".", "..", "a/b", "a\\b", "with space"] {
            assert!(
                matches!(validate_name(name), Err(Error::InvalidName(_))),
                "{name:?} should be rejected"
            );
        }
    }
}
