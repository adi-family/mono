//! The backend definition — one flat object holding everything needed to answer a turn.
//!
//! One file per backend, `llm/backends/<id>.toml`, the id taken from the filename the way an
//! agent's name is (there is no `id` field to drift from what the file is called):
//!
//! ```toml
//! label = "Anthropic — my Claude subscription"
//!
//! runtime  = "pty:claude"                          # the existing Backend enum
//! settings = "~/.claude/settings.anthropic.json"   # the credential
//! model    = "claude-opus-5"
//! context_tokens = 200000
//!
//! params = { effort = "high" }     # the dials
//!
//! [[limit_rules]]
//! match  = "(?i)usage limit reached|5-hour limit"
//! class  = "quota"
//! scope  = "model"
//! resume = "from_message"
//! fixed  = "4h"
//!
//! [probe]
//! model  = "claude-haiku-4-5-20251001"
//! prompt = "ok"
//! ```
//!
//! Everything the agent argument structs already carry splits cleanly in two, and the split is the
//! reason this object is flat rather than a login with a configuration on top: the **connection**
//! facts ([`settings`](LlmBackendManifest::settings), `provider`, `base_url`, `api_key_env`) say
//! *who is paying*, and the **dials** ([`params`](LlmBackendManifest::params)) say how hard it
//! thinks. An agent's row may override the dials and the model. It may never override the
//! connection — wanting a different login means wanting a different backend.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use adi_config::{Config, Module};

use crate::backend::Backend;
use crate::error::{Error, Result};
use crate::llm::{BACKENDS_DIR, LLM_MODULE};

/// The argument keys that name *who is paying*. An agent row's overrides may not contain these:
/// changing one would point a row at a different subscription while still calling it the same
/// backend, and the [hold key](crate::llm::holds::HoldKey) — which is derived from exactly these —
/// would then describe a credential nobody is using.
pub const CREDENTIAL_KEYS: [&str; 4] = ["settings", "provider", "base_url", "api_key_env"];

/// The argument key holding the model name, on every backend that has one.
pub const MODEL_KEY: &str = "model";

/// What a matched error means, and therefore what happens next. The classification — not the regex
/// that produced it — is what decides whether a run reroutes silently or stops and asks, so an
/// error nobody wrote a rule for lands in [`Unknown`](LimitClass::Unknown) and is *surfaced*,
/// never quietly routed around.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LimitClass {
    /// The subscription or credit balance is spent. Hold until `T`, move to the next row, tell the
    /// human in one line.
    Quota,
    /// Too many requests too quickly. A short hold, and one in-place retry before moving.
    Rate,
    /// The credential itself is broken — a dead token, a revoked key. Holds the whole login and
    /// **asks**, because waiting out a cooldown would never fix it.
    Auth,
    /// A blip: a 500, a dropped connection. Retried in place; nothing is held.
    Transient,
    /// Nothing matched. No hold, no reroute — the error is shown to the human as it stands.
    #[default]
    Unknown,
}

impl LimitClass {
    /// Whether this class reroutes on its own, with only a notice. The operator's rule: a usage
    /// limit is the machine's problem, anything else is theirs.
    #[must_use]
    pub fn reroutes_silently(self) -> bool {
        matches!(self, Self::Quota | Self::Rate)
    }

    /// Whether this class records a hold at all. `transient` and `unknown` deliberately do not:
    /// holding on an error nobody classified would take a working backend out of every agent's
    /// chain on the strength of a guess.
    #[must_use]
    pub fn holds(self) -> bool {
        matches!(self, Self::Quota | Self::Rate | Self::Auth)
    }
}

/// How wide a hold spreads.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HoldScope {
    /// This login *on this model*. An Opus quota leaves the Sonnet backend on the same
    /// subscription untouched — which is the whole reason the hold key carries a model.
    #[default]
    Model,
    /// This login, whatever the model. A dead token or an unreachable endpoint.
    Login,
}

/// Where the "available again at" time comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Resume {
    /// Parse it out of the provider's own words — "resets at 14:00", "try again in 3 hours".
    /// Falls back to [`fixed`](LimitRule::fixed) when the message says nothing parseable.
    #[default]
    FromMessage,
    /// Read the `Retry-After` header.
    RetryAfter,
    /// Always wait [`fixed`](LimitRule::fixed).
    Fixed,
}

/// One rule matching a backend's way of saying "I am out", and what that means.
///
/// Rules are tried in order and the first match wins, so a specific rule belongs above a general
/// one. A backend with no rules can still fail — it simply never produces anything but
/// [`LimitClass::Unknown`], which is the safe direction: it stops and asks rather than rerouting on
/// a guess.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LimitRule {
    /// A regular expression tested against the error text (and, where there is one, the status
    /// line). Spelled `match` in the file, which is a Rust keyword — hence the rename.
    #[serde(rename = "match")]
    pub pattern: String,
    /// What a match means. See [`LimitClass`].
    #[serde(default)]
    pub class: LimitClass,
    /// How wide the resulting hold spreads. See [`HoldScope`].
    #[serde(default)]
    pub scope: HoldScope,
    /// Where the resume time comes from. See [`Resume`].
    #[serde(default)]
    pub resume: Resume,
    /// The wait used when nothing parseable is found — `"4h"`, `"30m"`, `"90s"`. Absent means
    /// [`DEFAULT_HOLD`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fixed: Option<String>,
}

impl LimitRule {
    /// The fallback wait this rule asks for, in seconds.
    #[must_use]
    pub fn fixed_seconds(&self) -> u64 {
        self.fixed
            .as_deref()
            .and_then(parse_duration)
            .unwrap_or(DEFAULT_HOLD)
    }
}

/// How long a hold lasts when neither the provider's message nor the rule says otherwise. Long
/// enough not to re-trip a five-hour cap every few minutes, short enough that a wrong guess costs
/// one afternoon rather than a day.
pub const DEFAULT_HOLD: u64 = 4 * 60 * 60;

/// The cheap request that asks a held backend whether it is back.
///
/// It exists so that **recovery never costs a chat turn**. A run that discovers a backend has
/// recovered has paid for that discovery with a failed turn; the prober pays instead, with a few
/// tokens on the cheapest model the credential can reach.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Probe {
    /// The model to probe on — cheaper than the backend's own, usually. Absent probes the
    /// backend's [`model`](LlmBackendManifest::model).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// The prompt to send. A couple of tokens; the answer is never read, only the fact of one.
    #[serde(default = "default_probe_prompt")]
    pub prompt: String,
}

impl Default for Probe {
    fn default() -> Self {
        Self {
            model: None,
            prompt: default_probe_prompt(),
        }
    }
}

fn default_probe_prompt() -> String {
    "ok".to_string()
}

/// A backend definition, as stored. The id is the filename, so it cannot drift from what the file
/// is called.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct LlmBackendManifest {
    /// What a human calls it — "Anthropic — my Claude subscription". Free text, shown in the
    /// panel; the id is what everything else refers to.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub label: String,
    /// Which runner answers the turn: the existing [`Backend`] enum (`pty:claude`, `harness:adi`,
    /// …). This sits *above* the runner registry and changes nothing under it.
    pub runtime: Backend,
    /// The model this backend runs. Empty leaves the runtime's own default in place.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub model: String,
    /// How much history this backend can be handed, in tokens. `0` means unknown, and an unknown
    /// window is never used to *refuse* a switch — only a known, too-small one is.
    ///
    /// It is declared here rather than looked up per model because it is the number the
    /// context-shrink warning and the overflow check both read, and a wrong lookup would silently
    /// truncate somebody's conversation.
    #[serde(skip_serializing_if = "is_zero")]
    pub context_tokens: u64,
    /// The engine settings file holding the credential (`~/.claude/settings.glm.json`). One of the
    /// four connection facts; see [`CREDENTIAL_KEYS`].
    #[serde(skip_serializing_if = "Option::is_none")]
    pub settings: Option<String>,
    /// The provider wire for `harness:adi` (`anthropic`, `openai`, `zai`, `ollama`, …).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    /// The API base URL, when it is not the provider's default.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    /// The environment variable the API key is read from.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_key_env: Option<String>,
    /// The dials — `effort`, `reasoning_effort`, `thinking`, `max_tokens`, `sandbox`, and their
    /// neighbours — laid onto the run's arguments as they stand. These are what an agent's row may
    /// override.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub params: BTreeMap<String, serde_json::Value>,
    /// How this backend says "I am out", in order, first match winning. See [`LimitRule`].
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub limit_rules: Vec<LimitRule>,
    /// The cheap request that asks whether it is back. Absent means this backend is never probed,
    /// and a hold on it simply expires.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub probe: Option<Probe>,
    pub created_at: u64,
    pub updated_at: u64,
}

#[allow(clippy::trivially_copy_pass_by_ref)] // serde's skip_serializing_if hands us a reference
fn is_zero(value: &u64) -> bool {
    *value == 0
}

impl adi_config::Timestamped for LlmBackendManifest {
    fn created_at(&self) -> u64 {
        self.created_at
    }
}

/// A backend definition paired with its filename-derived id.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct LlmBackend {
    pub id: String,
    pub manifest: LlmBackendManifest,
}

impl LlmBackendManifest {
    /// The key this backend's holds are recorded under: *who is paying*, independent of the model.
    ///
    /// Two backends naming the same settings file are the same subscription and must share a hold —
    /// that is what stops a quota discovered by one agent being rediscovered by fifteen others. The
    /// derivation is deliberately ordered, most specific first, so a backend that names both a
    /// settings file and a provider is keyed on the settings file rather than on the vaguer of the
    /// two.
    #[must_use]
    pub fn credential(&self) -> String {
        if let Some(settings) = clean(self.settings.as_deref()) {
            return format!("settings:{}", expand_home(&settings));
        }
        let parts: Vec<String> = [
            clean(self.provider.as_deref()),
            clean(self.base_url.as_deref()),
            clean(self.api_key_env.as_deref()),
        ]
        .into_iter()
        .flatten()
        .collect();
        if parts.is_empty() {
            // Nothing names a credential, so the runtime's own ambient login is the credential —
            // the logged-in `claude` CLI, say. Every backend on that runtime shares it, which is
            // exactly right: they are one subscription.
            return format!("runtime:{}", self.runtime);
        }
        parts.join("|")
    }

    /// The arguments this backend contributes to a run: its model, its connection facts, and its
    /// dials, in the same flat `key = value` shape the agent form already uses.
    ///
    /// This is the whole of how a backend reaches the runner — there is no second path. A resolved
    /// row lays this onto the agent's own arguments, so everything downstream (each runner's typed
    /// argument struct, the settings-file expansion, the gateway routing) keeps working untouched.
    #[must_use]
    pub fn arguments(&self) -> BTreeMap<String, serde_json::Value> {
        let mut out = BTreeMap::new();
        if !self.model.is_empty() {
            out.insert(MODEL_KEY.to_string(), self.model.clone().into());
        }
        for (key, value) in [
            ("settings", &self.settings),
            ("provider", &self.provider),
            ("base_url", &self.base_url),
            ("api_key_env", &self.api_key_env),
        ] {
            if let Some(value) = clean(value.as_deref()) {
                out.insert(key.to_string(), value.into());
            }
        }
        for (key, value) in &self.params {
            out.insert(key.clone(), value.clone());
        }
        out
    }

    /// Whether this backend's runtime can be handed a conversation that started somewhere else.
    ///
    /// The pty runtimes cannot, and this is a property of the runner rather than of the model: a
    /// pty backend *is* a live terminal session, and ADI keeps no transcript of one
    /// (`runner/pty.rs` writes none, and `.transcript.jsonl` is only ever read). There is therefore
    /// nothing to replay into a second vendor and nothing to replay out of. Requirement 7 — replay
    /// the whole conversation, never truncate it — cannot be met there, so rather than pretend, a
    /// switch touching a pty row stops and asks.
    #[must_use]
    pub fn is_replayable(&self) -> bool {
        self.runtime.executor() != "pty"
    }
}

/// Trim to `None` when a field is absent or blank, so `settings = ""` and no `settings` at all are
/// the same state rather than two.
fn clean(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}

/// Expand a leading `~` / `$HOME` so two spellings of one settings file key the same credential.
/// The run path expands these too (a run is spawned without a shell, so a literal `~/…` would
/// reach the CLI as a filename that does not exist).
fn expand_home(value: &str) -> String {
    let home = adi_config::home();
    let home = home.to_string_lossy();
    if let Some(rest) = value.strip_prefix("~/") {
        return format!("{home}/{rest}");
    }
    if let Some(rest) = value.strip_prefix("$HOME/") {
        return format!("{home}/{rest}");
    }
    value.to_string()
}

/// Read `"4h"` / `"30m"` / `"90s"` / `"3600"` as seconds. `None` when it is not a duration at all,
/// which the caller reads as "use the default" rather than as an error: a typo in a cooldown must
/// not be the reason a limit goes unrecorded.
#[must_use]
pub fn parse_duration(value: &str) -> Option<u64> {
    let value = value.trim();
    if value.is_empty() {
        return None;
    }
    let (digits, multiplier) = match value.chars().last() {
        Some('h' | 'H') => (&value[..value.len() - 1], 3600),
        Some('m' | 'M') => (&value[..value.len() - 1], 60),
        Some('s' | 'S') => (&value[..value.len() - 1], 1),
        _ => (value, 1),
    };
    digits.trim().parse::<u64>().ok().map(|n| n * multiplier)
}

/// The on-disk store of backend definitions: `llm/backends/<id>.toml`.
#[derive(Debug, Clone)]
pub struct LlmBackends {
    config: Config,
}

impl LlmBackends {
    /// Open the store over a config root.
    #[must_use]
    pub fn with_config(config: Config) -> Self {
        Self { config }
    }

    fn module(&self) -> Module {
        self.config
            .module(&format!("{LLM_MODULE}/{BACKENDS_DIR}"))
    }

    /// Where the definitions live.
    #[must_use]
    pub fn dir(&self) -> std::path::PathBuf {
        self.module().dir().to_path_buf()
    }

    /// Every backend, by id. A store nothing has written to yet is an empty list, not an error.
    ///
    /// # Errors
    /// [`Error::Io`] on a directory read failure, or [`Error::Config`] on an unreadable definition.
    pub fn list(&self) -> Result<Vec<LlmBackend>> {
        let entries = match std::fs::read_dir(self.dir()) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(Error::Io(e)),
        };
        let mut out = Vec::new();
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some(adi_config::MANIFEST_EXT) {
                continue;
            }
            let Some(id) = path.file_stem().and_then(|s| s.to_str()) else {
                continue;
            };
            if let Some(backend) = self.get(id)? {
                out.push(backend);
            }
        }
        out.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(out)
    }

    /// One backend by id, or `None` when there is no such definition.
    ///
    /// # Errors
    /// [`Error::Config`] when the definition exists but cannot be read.
    pub fn get(&self, id: &str) -> Result<Option<LlmBackend>> {
        crate::agent::validate_name(id)?;
        let file = self.module().manifest_file::<LlmBackendManifest>(id);
        if !file.exists() {
            return Ok(None);
        }
        Ok(Some(LlmBackend {
            id: id.to_string(),
            manifest: file.load()?,
        }))
    }

    /// Create or replace a backend definition, stamping the timestamps.
    ///
    /// # Errors
    /// [`Error::InvalidName`] for an id that cannot be a filename, [`Error::Arguments`] when the
    /// definition is self-contradictory, or [`Error::Config`] when it cannot be written.
    pub fn save(&self, id: &str, mut manifest: LlmBackendManifest) -> Result<LlmBackend> {
        crate::agent::validate_name(id)?;
        validate(&manifest)?;
        let now = adi_config::now_unix();
        let existing = self.get(id)?;
        manifest.created_at = existing
            .as_ref()
            .map_or(now, |backend| backend.manifest.created_at);
        manifest.updated_at = now;
        self.module().manifest_file(id).save(&manifest)?;
        Ok(LlmBackend {
            id: id.to_string(),
            manifest,
        })
    }

    /// Delete a backend definition, returning whether it existed.
    ///
    /// The caller is expected to have checked which agents name it first — this store knows
    /// nothing about agents, deliberately, because a backend that knew its users would be a login
    /// record and the design has none.
    ///
    /// # Errors
    /// [`Error::Config`] on a removal failure other than not-found.
    pub fn delete(&self, id: &str) -> Result<bool> {
        crate::agent::validate_name(id)?;
        Ok(self.module().remove_manifest(id)?)
    }
}

/// Reject a definition that cannot mean what it says. Kept small: this is configuration, and a
/// store that argues with the operator about their own dials is worse than one that lets an odd
/// setting through.
fn validate(manifest: &LlmBackendManifest) -> Result<()> {
    if manifest.runtime.to_string().is_empty() {
        return Err(Error::Arguments(
            "a backend needs a runtime (pty:claude, harness:adi, …)".into(),
        ));
    }
    for rule in &manifest.limit_rules {
        regex::Regex::new(&rule.pattern)
            .map_err(|e| Error::Arguments(format!("limit rule `{}`: {e}", rule.pattern)))?;
    }
    for key in CREDENTIAL_KEYS {
        if manifest.params.contains_key(key) {
            return Err(Error::Arguments(format!(
                "`{key}` is a credential, not a dial — set it as a field, not in params"
            )));
        }
    }
    if manifest.params.contains_key(MODEL_KEY) {
        return Err(Error::Arguments(
            "`model` is a field of its own, not a dial in params".into(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> LlmBackends {
        let root = std::env::temp_dir().join(format!(
            "adi-agents-llm-backends-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id(),
        ));
        let _ = std::fs::remove_dir_all(&root);
        LlmBackends::with_config(Config::with_root(root))
    }

    fn anthropic() -> LlmBackendManifest {
        LlmBackendManifest {
            label: "Anthropic — my Claude subscription".into(),
            runtime: Backend::PtyClaude,
            model: "claude-opus-5".into(),
            context_tokens: 200_000,
            settings: Some("~/.claude/settings.anthropic.json".into()),
            params: [("effort".to_string(), serde_json::json!("high"))]
                .into_iter()
                .collect(),
            ..Default::default()
        }
    }

    #[test]
    fn a_definition_round_trips_through_the_store() {
        let store = scratch("round-trip");
        store.save("anthropic", anthropic()).expect("save");

        let read = store.get("anthropic").expect("get").expect("present");
        assert_eq!(read.id, "anthropic");
        assert_eq!(read.manifest.model, "claude-opus-5");
        assert_eq!(read.manifest.runtime, Backend::PtyClaude);
        assert_eq!(read.manifest.context_tokens, 200_000);
        assert_eq!(
            read.manifest.params.get("effort"),
            Some(&serde_json::json!("high"))
        );
        assert!(read.manifest.created_at > 0, "timestamps are stamped");
    }

    /// The id is the filename and nothing else, so a definition cannot disagree with what it is
    /// called — the drift an `id` field inside the file would invite.
    #[test]
    fn the_file_carries_no_id_of_its_own() {
        let text = toml::to_string_pretty(&anthropic()).expect("toml");
        assert!(!text.contains("id ="), "{text}");
    }

    #[test]
    fn listing_is_sorted_and_an_empty_store_is_not_an_error() {
        let store = scratch("list");
        assert!(store.list().expect("empty list").is_empty());

        store.save("glm", LlmBackendManifest {
            runtime: Backend::HarnessAdi,
            provider: Some("zai".into()),
            ..Default::default()
        })
        .expect("save glm");
        store.save("anthropic", anthropic()).expect("save anthropic");

        let ids: Vec<String> = store.list().expect("list").into_iter().map(|b| b.id).collect();
        assert_eq!(ids, vec!["anthropic".to_string(), "glm".to_string()]);
    }

    #[test]
    fn saving_twice_keeps_the_original_created_at() {
        let store = scratch("created-at");
        let first = store.save("anthropic", anthropic()).expect("save");
        let again = store.save("anthropic", anthropic()).expect("save again");
        assert_eq!(again.manifest.created_at, first.manifest.created_at);
    }

    #[test]
    fn deleting_reports_whether_it_was_there() {
        let store = scratch("delete");
        store.save("anthropic", anthropic()).expect("save");
        assert!(store.delete("anthropic").expect("delete"));
        assert!(!store.delete("anthropic").expect("delete again"));
        assert!(store.get("anthropic").expect("get").is_none());
    }

    /// Two backends naming the same settings file are one subscription, and must share a hold —
    /// otherwise a quota found by the Opus row is rediscovered by the Sonnet row beside it.
    #[test]
    fn the_same_settings_file_is_the_same_credential() {
        let opus = anthropic();
        let sonnet = LlmBackendManifest {
            model: "claude-sonnet-5".into(),
            ..anthropic()
        };
        assert_eq!(opus.credential(), sonnet.credential());

        let glm = LlmBackendManifest {
            runtime: Backend::HarnessAdi,
            settings: None,
            provider: Some("zai".into()),
            api_key_env: Some("Z_AI_API_KEY".into()),
            ..Default::default()
        };
        assert_ne!(glm.credential(), opus.credential());
    }

    /// `~/x` and `$HOME/x` are the same file, so they must not key two different credentials.
    #[test]
    fn home_is_expanded_before_keying_a_credential() {
        let tilde = anthropic();
        let dollar = LlmBackendManifest {
            settings: Some("$HOME/.claude/settings.anthropic.json".into()),
            ..anthropic()
        };
        assert_eq!(tilde.credential(), dollar.credential());
        assert!(!tilde.credential().contains('~'), "{}", tilde.credential());
    }

    /// A backend naming no credential at all is keyed on its runtime — the ambient logged-in CLI —
    /// so two such backends on one runtime still share a subscription.
    #[test]
    fn a_backend_with_no_credential_keys_on_its_runtime() {
        let bare = LlmBackendManifest {
            runtime: Backend::PtyCodex,
            ..Default::default()
        };
        assert_eq!(bare.credential(), "runtime:pty:codex");
    }

    #[test]
    fn arguments_carry_the_model_the_connection_and_the_dials() {
        let arguments = anthropic().arguments();
        assert_eq!(arguments["model"], serde_json::json!("claude-opus-5"));
        assert_eq!(
            arguments["settings"],
            serde_json::json!("~/.claude/settings.anthropic.json")
        );
        assert_eq!(arguments["effort"], serde_json::json!("high"));
        assert!(!arguments.contains_key("provider"), "absent stays absent");
    }

    /// The pty runtimes keep no transcript, so nothing can be replayed into or out of one.
    #[test]
    fn only_non_pty_runtimes_are_replayable() {
        for runtime in [Backend::PtyClaude, Backend::PtyCodex] {
            let manifest = LlmBackendManifest {
                runtime,
                ..Default::default()
            };
            assert!(!manifest.is_replayable(), "{} keeps no transcript", manifest.runtime);
        }
        for runtime in [
            Backend::HarnessAdi,
            Backend::HarnessClaudeSdk,
            Backend::ProcessClaude,
            Backend::ProcessCodex,
        ] {
            let manifest = LlmBackendManifest {
                runtime,
                ..Default::default()
            };
            assert!(manifest.is_replayable(), "{} has a transcript", manifest.runtime);
        }
    }

    #[test]
    fn a_credential_in_params_is_refused() {
        let store = scratch("validate-credential");
        let mut manifest = anthropic();
        manifest
            .params
            .insert("base_url".into(), serde_json::json!("https://example.test"));
        let err = store.save("anthropic", manifest).expect_err("refused");
        assert!(err.to_string().contains("credential"), "{err}");
    }

    #[test]
    fn a_model_in_params_is_refused() {
        let store = scratch("validate-model");
        let mut manifest = anthropic();
        manifest
            .params
            .insert("model".into(), serde_json::json!("sonnet"));
        assert!(store.save("anthropic", manifest).is_err());
    }

    #[test]
    fn an_unparseable_limit_rule_is_refused_at_save_time() {
        let store = scratch("validate-regex");
        let mut manifest = anthropic();
        manifest.limit_rules.push(LimitRule {
            pattern: "([unclosed".into(),
            class: LimitClass::Quota,
            scope: HoldScope::Model,
            resume: Resume::FromMessage,
            fixed: None,
        });
        let err = store.save("anthropic", manifest).expect_err("refused");
        assert!(err.to_string().contains("limit rule"), "{err}");
    }

    #[test]
    fn a_backend_with_no_runtime_is_refused() {
        let store = scratch("validate-runtime");
        let manifest = LlmBackendManifest {
            model: "x".into(),
            ..Default::default()
        };
        assert!(store.save("nowhere", manifest).is_err());
    }

    #[test]
    fn durations_read_in_the_units_people_write() {
        assert_eq!(parse_duration("4h"), Some(14_400));
        assert_eq!(parse_duration("30m"), Some(1_800));
        assert_eq!(parse_duration("90s"), Some(90));
        assert_eq!(parse_duration(" 45 "), Some(45));
        assert_eq!(parse_duration("soon"), None);
        assert_eq!(parse_duration(""), None);
    }

    /// A typo in a cooldown must not be the reason a limit goes unrecorded — an unreadable `fixed`
    /// falls back to the default rather than to nothing.
    #[test]
    fn an_unreadable_fixed_falls_back_to_the_default_hold() {
        let rule = LimitRule {
            pattern: "x".into(),
            class: LimitClass::Quota,
            scope: HoldScope::Model,
            resume: Resume::Fixed,
            fixed: Some("whenever".into()),
        };
        assert_eq!(rule.fixed_seconds(), DEFAULT_HOLD);
    }

    #[test]
    fn classes_split_into_reroute_and_ask() {
        assert!(LimitClass::Quota.reroutes_silently());
        assert!(LimitClass::Rate.reroutes_silently());
        assert!(!LimitClass::Auth.reroutes_silently());
        assert!(!LimitClass::Unknown.reroutes_silently());

        assert!(LimitClass::Auth.holds());
        assert!(!LimitClass::Transient.holds(), "a blip holds nothing");
        assert!(
            !LimitClass::Unknown.holds(),
            "an unclassified error must not take a backend out of every chain on a guess"
        );
    }
}
