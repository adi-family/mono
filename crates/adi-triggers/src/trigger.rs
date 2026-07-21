//! The on-disk trigger definition ([`TriggerManifest`], serialized as `<name>.toml`) and the
//! name-attached view of a loaded trigger ([`Trigger`]).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

/// The trigger kinds. A kind answers exactly one question — **how the code block gets
/// launched** — so there are only three:
///
/// * [`KIND_WEBHOOK`] — launched by an inbound HTTP call to `/api/hooks/<name>`; the request
///   body becomes the payload. Extra: `secret` (optional shared secret the caller must pass as
///   `?secret=`).
/// * [`KIND_BACKGROUND`] — a long-lived independent process. While the trigger is enabled the
///   supervisor keeps it running, restarting it with backoff if it exits; disabling stops it.
/// * [`KIND_EVENT`] — launched whenever a platform event ([`adi_events`]) whose name matches one
///   of the trigger's [`events`](TriggerManifest::events) patterns is published. The event's
///   payload becomes the payload; its concrete name arrives as `ADI_EVENT`. A one-off fire like a
///   webhook, but the source is the internal event bus rather than an HTTP call.
///
/// Everything a trigger *does* — talk to Telegram, poll on a schedule, react to a push — is the
/// job of its code block, prefilled from a [preset](crate::presets) rather than a kind.
pub const KIND_WEBHOOK: &str = "webhook";
pub const KIND_BACKGROUND: &str = "background";
pub const KIND_EVENT: &str = "event";

/// Kinds this store used to have, now folded into [`KIND_BACKGROUND`]. Kept so manifests written
/// before the collapse keep loading (see [`normalize_kind`]).
const LEGACY_BACKGROUND_KINDS: &[&str] = &["telegram", "cron", "manual"];

/// How a code block is interpreted when it is launched.
///
/// * [`RUNTIME_SH`] — a shell script, run as `sh -c <code>`.
/// * [`RUNTIME_TS`] — TypeScript, written to `triggers/src/<name>.ts` and run with `bun run`.
pub const RUNTIME_SH: &str = "sh";
pub const RUNTIME_TS: &str = "ts";

/// Map a stored `kind` onto one this build understands: the three live kinds pass through, and
/// every retired kind (`telegram`, `cron`, `manual`) reads as [`KIND_BACKGROUND`] — those were
/// always "a code block that isn't a webhook", which is what a background trigger is. An
/// unrecognized kind also reads as background, so a manifest from a newer build still loads
/// rather than vanishing from the list.
#[must_use]
pub fn normalize_kind(kind: &str) -> &str {
    match kind.trim() {
        KIND_WEBHOOK => KIND_WEBHOOK,
        KIND_EVENT => KIND_EVENT,
        k if k == KIND_BACKGROUND || LEGACY_BACKGROUND_KINDS.contains(&k) => KIND_BACKGROUND,
        _ => KIND_BACKGROUND,
    }
}

/// Map a stored `runtime` onto one this build understands, defaulting to [`RUNTIME_SH`] — an
/// empty field is what every manifest written before runtimes existed has.
#[must_use]
pub fn normalize_runtime(runtime: &str) -> &str {
    match runtime.trim() {
        RUNTIME_TS => RUNTIME_TS,
        _ => RUNTIME_SH,
    }
}

/// A reusable trigger definition: *how* it launches (the kind), *what language* its code block
/// is (the runtime), and *what runs* (the code block itself). It says nothing about any live
/// process — the store is pure data; launching is [`crate::fire`] and [`crate::Supervisor`].
///
/// Unknown fields are ignored so the manifest can gain fields without breaking older stores.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TriggerManifest {
    /// How this trigger launches: `webhook` | `background`. Normalized on load.
    pub kind: String,
    /// The language of [`code`](Self::code): `sh` | `ts`. Normalized on load.
    #[serde(default)]
    pub runtime: String,
    /// The code block launched when the trigger fires — a shell script (`sh -c`) or a
    /// TypeScript module (`bun run`), spawned detached with `ADI_TRIGGER`/`ADI_TRIGGER_KIND`,
    /// every [extra](Self::extra) as `ADI_<KEY>`, and `ADI_PAYLOAD_FILE` when the event carries
    /// a payload.
    #[serde(default)]
    pub code: String,
    /// The [preset](crate::presets) this trigger was prefilled from, if any. Pure provenance:
    /// it tells the editor which named settings the code block expects, so reopening a trigger
    /// shows the same fields the preset offered. Never affects launching.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preset: Option<String>,
    /// A free-form one-line description shown in lists.
    #[serde(default)]
    pub description: String,
    /// Whether the trigger may launch. For a webhook that gates the endpoint; for a background
    /// trigger it *is* the on/off switch — the supervisor runs exactly the enabled ones. A
    /// disabled trigger keeps its definition and can still be fired by hand (an explicit user
    /// action).
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    /// The project this trigger is filed under (its [`adi-projects`] id), or `None` for a
    /// global trigger. Pure metadata: it scopes where the trigger shows up (a project's detail
    /// page), not what it may do.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    /// Named settings the code block reads, exported into its environment as `ADI_<KEY>`
    /// (uppercased). Which keys matter is the preset's business — `secret` is the one the
    /// platform itself reads, to guard a webhook.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub extra: BTreeMap<String, String>,
    /// For an [event](KIND_EVENT) trigger only: the event-name patterns it subscribes to, matched
    /// segment-by-segment ([`adi_events::matches`]) — `adi.tasks.*` (one segment) or `adi.tasks.**`
    /// (the tail). Any match fires the code block with the event's payload as `ADI_PAYLOAD` and its
    /// concrete name as `ADI_EVENT`. Ignored for the other kinds.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub events: Vec<String>,
    /// Restrict which projects may fire this trigger — an allowlist of [`adi-projects`] ids read
    /// from the *fire's payload*. Empty (the default) is unrestricted: the trigger fires for every
    /// project, exactly as before this field existed. When non-empty, an external source fires the
    /// trigger only if the event/request payload names a `project` in this list (see
    /// [`allows_project`](Self::allows_project) and [`payload_project`](crate::payload_project)) —
    /// a payload naming no project, or one outside the list, is skipped. A manual ▶ Fire is an
    /// explicit user action and bypasses it, as it does `enabled`. Distinct from
    /// [`project`](Self::project), which only files *where the trigger is shown*, never gating it.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub trigger_on: Vec<String>,
    /// When the definition was created, as Unix epoch seconds.
    #[serde(default)]
    pub created_at: u64,
    /// When the definition was last saved, as Unix epoch seconds.
    #[serde(default)]
    pub updated_at: u64,
}

impl adi_config::Timestamped for TriggerManifest {
    fn created_at(&self) -> u64 {
        self.created_at
    }
}

impl TriggerManifest {
    /// Fold the stored `kind`/`runtime` onto the values this build understands. Applied on every
    /// load, so the rest of the crate — and every caller — only ever sees a live kind and runtime.
    pub(crate) fn normalize(&mut self) {
        self.kind = normalize_kind(&self.kind).to_string();
        self.runtime = normalize_runtime(&self.runtime).to_string();
    }

    /// Whether this is a supervised long-lived trigger.
    #[must_use]
    pub fn is_background(&self) -> bool {
        normalize_kind(&self.kind) == KIND_BACKGROUND
    }

    /// Whether this is an event-driven trigger (fired by the event dispatcher on a matching
    /// [`adi_events`] publication).
    #[must_use]
    pub fn is_event(&self) -> bool {
        normalize_kind(&self.kind) == KIND_EVENT
    }

    /// Whether this trigger may fire for `project` — the project an external source read from the
    /// fire's payload (via [`payload_project`](crate::payload_project)). An empty
    /// [`trigger_on`](Self::trigger_on) allowlist admits every project (the trigger is
    /// unrestricted); otherwise the project must be present and listed. A `None` project (the
    /// payload named none) never satisfies a non-empty allowlist — a restricted trigger stays
    /// silent rather than firing for an unattributed event.
    #[must_use]
    pub fn allows_project(&self, project: Option<&str>) -> bool {
        self.trigger_on.is_empty()
            || project.is_some_and(|p| self.trigger_on.iter().any(|allowed| allowed == p))
    }
}

impl Default for TriggerManifest {
    /// An empty manifest that is nonetheless *enabled* — a freshly defined trigger should launch
    /// without an extra toggle step, matching the form/CLI defaults.
    fn default() -> Self {
        Self {
            kind: String::new(),
            runtime: RUNTIME_SH.to_string(),
            code: String::new(),
            preset: None,
            description: String::new(),
            enabled: true,
            project: None,
            extra: BTreeMap::new(),
            events: Vec::new(),
            trigger_on: Vec::new(),
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

/// The `project` an event or webhook payload names, if any: the top-level `"project"` string of a
/// JSON body — the field every task/agent event payload carries (a task's `project`, flattened
/// into its event view). Returns `None` for a non-JSON body, a missing / null / non-string
/// `project`, or a blank one: a body that names no project. This is the value
/// [`TriggerManifest::allows_project`] is checked against when a [`trigger_on`](TriggerManifest::trigger_on)
/// allowlist gates a fire.
#[must_use]
pub fn payload_project(payload: &[u8]) -> Option<String> {
    let value: serde_json::Value = serde_json::from_slice(payload).ok()?;
    let project = value.get("project")?.as_str()?.trim();
    (!project.is_empty()).then(|| project.to_string())
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

/// Validate a trigger name before it is joined onto the store path as `<name>.toml`, mapping a
/// rejection onto [`Error::InvalidName`]. The rule matters here because names also appear in the
/// *public webhook URL path*.
pub(crate) fn validate_name(name: &str) -> Result<()> {
    adi_config::validate_name(name, Error::InvalidName)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_default_manifest_is_enabled_and_shell_flavored() {
        let m = TriggerManifest::default();
        assert!(m.enabled);
        assert_eq!(m.runtime, RUNTIME_SH);
    }

    /// The kind collapse: the two live kinds survive, and every retired kind lands on
    /// `background` so an existing store keeps listing.
    #[test]
    fn retired_kinds_normalize_onto_background() {
        assert_eq!(normalize_kind(KIND_WEBHOOK), KIND_WEBHOOK);
        assert_eq!(normalize_kind(KIND_BACKGROUND), KIND_BACKGROUND);
        assert_eq!(normalize_kind(KIND_EVENT), KIND_EVENT);
        for legacy in ["telegram", "cron", "manual", "something-newer"] {
            assert_eq!(normalize_kind(legacy), KIND_BACKGROUND, "{legacy}");
        }
    }

    #[test]
    fn an_unset_runtime_reads_as_shell() {
        assert_eq!(normalize_runtime(""), RUNTIME_SH);
        assert_eq!(normalize_runtime("sh"), RUNTIME_SH);
        assert_eq!(normalize_runtime("ts"), RUNTIME_TS);
        assert_eq!(normalize_runtime("python"), RUNTIME_SH);
    }

    /// An empty allowlist admits every project; a populated one admits only its members, and never
    /// an unattributed (`None`) fire.
    #[test]
    fn allows_project_gates_on_the_allowlist() {
        let mut m = TriggerManifest::default();
        // Unrestricted: everything passes, including a payload that named no project.
        assert!(m.allows_project(Some("alpha")));
        assert!(m.allows_project(None));

        m.trigger_on = vec!["alpha".into(), "beta".into()];
        assert!(m.allows_project(Some("alpha")));
        assert!(m.allows_project(Some("beta")));
        assert!(!m.allows_project(Some("gamma")));
        // A restricted trigger stays silent for an event that named no project.
        assert!(!m.allows_project(None));
    }

    /// `payload_project` pulls the top-level `project` string out of a JSON body, and yields `None`
    /// for anything that names no usable project.
    #[test]
    fn payload_project_reads_the_top_level_project() {
        assert_eq!(
            payload_project(br#"{"id":"t1","project":"alpha","title":"x"}"#).as_deref(),
            Some("alpha")
        );
        assert_eq!(payload_project(br#"{"id":"t1"}"#), None);
        assert_eq!(payload_project(br#"{"project":null}"#), None);
        assert_eq!(payload_project(br#"{"project":"  "}"#), None);
        assert_eq!(payload_project(br#"{"project":42}"#), None);
        assert_eq!(payload_project(b"not json"), None);
        assert_eq!(payload_project(b""), None);
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
