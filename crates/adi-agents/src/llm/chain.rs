//! The agent's ordered list of backends, and resolving it into the configurations a run will try.
//!
//! An agent has no model settings of its own. It has one field — an ordered array of rows, each
//! naming a backend and optionally overriding its model and dials:
//!
//! ```toml
//! backends = [
//!   { backend = "anthropic", overrides = { thinking = "high" } },
//!   { backend = "codex" },
//!   { backend = "glm" },
//! ]
//! ```
//!
//! **Row 1 is what a new conversation starts on**; the rest are where it goes when a backend runs
//! out. There is no `default_backend` field, because the default is first in the list.
//!
//! One row per backend. Falling from Opus to Sonnet on one subscription is *two backends* — both
//! naming the same settings file, so both keyed on the same credential — listed one after the
//! other, not one backend listed twice under different model overrides. A repeated base stays legal
//! in a hand-edited file and everything here tolerates it, but it is never offered in the interface
//! and never documented as a way of working.
//!
//! Resolution happens **once**, on the first turn of a session, and the result is pinned to that
//! session. An agent or a backend edited mid-conversation therefore cannot change what a live chat
//! is talking to.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::agent::StoredAgent;
use crate::backend::Backend;
use crate::error::{Error, Result};
use crate::llm::backend::{
    CREDENTIAL_KEYS, LimitRule, LlmBackend, LlmBackendManifest, MODEL_KEY, Probe,
};
use crate::llm::holds::HoldKey;

/// One row of an agent's list: a backend, plus what this agent changes about it.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct AgentBackendEntry {
    /// The id of the backend used as the base. Every row has one.
    pub backend: String,
    /// What this agent changes about it — the model and the dials, in the same flat shape the
    /// agent form already uses. Optional; a row with none is the backend exactly as defined.
    ///
    /// Overrides never touch the runtime or the credential ([`CREDENTIAL_KEYS`]): wanting those
    /// different means wanting a different backend, and a row that quietly repointed a credential
    /// would make the shared hold describe a subscription nobody was using.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub overrides: BTreeMap<String, serde_json::Value>,
}

impl AgentBackendEntry {
    /// A row naming a backend with nothing changed about it.
    #[must_use]
    pub fn new(backend: impl Into<String>) -> Self {
        Self {
            backend: backend.into(),
            overrides: BTreeMap::new(),
        }
    }
}

/// Where a launch wants the chain to begin.
///
/// Rotating rather than truncating is the point: starting at row 3 runs 3, then 1, then 2. A run
/// begun on a second-choice backend still has the whole list behind it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum StartAt {
    /// A backend id — what the picker sends, and what a human types.
    Id(String),
    /// A **1-based** row number, counting the agent's configured list from the top. The escape
    /// hatch for a hand-written manifest that names one backend twice; the picker never sends it.
    Row(usize),
}

/// One row resolved into the concrete configuration a run would use.
///
/// Read back as well as written: a resolved row is what a session pins, so it has to survive a round
/// trip through the record's JSON column. See [`PinnedChain`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ResolvedBackend {
    /// The base backend's id.
    pub backend: String,
    /// Its 1-based position in the agent's *configured* list, kept through rotation so a notice can
    /// say which row it came from.
    pub row: usize,
    /// What a human calls this backend.
    pub label: String,
    /// The runner that answers the turn.
    pub runtime: Backend,
    /// Who is paying — the shared hold key's first half. See [`LlmBackendManifest::credential`].
    pub credential: String,
    /// The model, after this row's overrides. The hold key's second half.
    pub model: String,
    /// How much history this backend can be handed. `0` is unknown.
    pub context_tokens: u64,
    /// The full argument patch this row lays onto the agent: model, connection facts, dials.
    pub arguments: BTreeMap<String, serde_json::Value>,
    /// How this backend says it is out.
    pub limit_rules: Vec<LimitRule>,
    /// The cheap request that asks whether it is back.
    pub probe: Option<Probe>,
    /// Whether a conversation can be moved onto (or off) this row at all — false for the pty
    /// runtimes, which keep no transcript. See [`LlmBackendManifest::is_replayable`].
    pub replayable: bool,
}

impl ResolvedBackend {
    /// The key this row's holds are recorded under: the credential, and the model when the hold is
    /// model-scoped.
    #[must_use]
    pub fn hold_key(&self) -> HoldKey {
        HoldKey::new(&self.credential, &self.model)
    }

    /// This row's configuration laid onto an agent — the one and only way a backend reaches the
    /// runner.
    ///
    /// Everything downstream keeps working untouched because the result is an ordinary
    /// [`StoredAgent`]: the runtime lands on `manifest.backend` and the rest on
    /// `manifest.arguments`, exactly where each runner's typed argument struct already looks.
    ///
    /// The agent's own stale model configuration is cleared first, so a manifest that still carries
    /// a `model` from before the migration cannot leak past the row that is supposed to decide it.
    #[must_use]
    pub fn apply(&self, agent: &StoredAgent) -> StoredAgent {
        let mut patched = agent.clone();
        patched.manifest.backend = self.runtime.clone();
        patched.manifest.arguments.remove(MODEL_KEY);
        for key in CREDENTIAL_KEYS {
            patched.manifest.arguments.remove(key);
        }
        for (key, value) in &self.arguments {
            patched.manifest.arguments.insert(key.clone(), value.clone());
        }
        patched
    }

    /// One short phrase naming this row, for a notice or a log line.
    #[must_use]
    pub fn describe(&self) -> String {
        if self.model.is_empty() {
            self.backend.clone()
        } else {
            format!("{} ({})", self.backend, self.model)
        }
    }
}

/// An agent's list, resolved and ready to be pinned to a session.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ResolvedChain {
    /// The rows a run will try, best-first, already rotated by the launch's `start_at`.
    pub entries: Vec<ResolvedBackend>,
    /// Rows that named a backend this store does not have, by id. Kept rather than raised: a
    /// deleted backend should not stop every agent that still names it from running on the rows
    /// that do resolve — but it is reported, because a silently shorter chain is a chain that fails
    /// over less far than its operator believes.
    pub missing: Vec<String>,
}

impl ResolvedChain {
    /// Resolve an agent's configured list against the backend catalog.
    ///
    /// `only` pins the run to a single row with no failover behind it — the explicit opposite of
    /// `start_at`, which rotates and keeps everything.
    ///
    /// # Errors
    /// [`Error::Arguments`] when the agent has no rows at all, when none of them names a backend
    /// that exists, or when `start_at` / `only` names a row that is not in the list.
    pub fn resolve(
        rows: &[AgentBackendEntry],
        catalog: &BTreeMap<String, LlmBackendManifest>,
        start_at: Option<&StartAt>,
        only: Option<&str>,
    ) -> Result<Self> {
        if rows.is_empty() {
            return Err(Error::Arguments(
                "this agent lists no backends, so there is nothing to answer the turn".into(),
            ));
        }

        let mut entries = Vec::new();
        let mut missing = Vec::new();
        for (index, row) in rows.iter().enumerate() {
            let Some(manifest) = catalog.get(&row.backend) else {
                missing.push(row.backend.clone());
                continue;
            };
            entries.push(resolve_row(row, manifest, index + 1));
        }

        if entries.is_empty() {
            return Err(Error::Arguments(format!(
                "none of this agent's backends exist: {}",
                missing.join(", ")
            )));
        }

        if let Some(only) = only {
            let kept = entries
                .iter()
                .find(|entry| entry.backend == only)
                .cloned()
                .ok_or_else(|| {
                    Error::Arguments(format!("`only` names `{only}`, which this agent does not list"))
                })?;
            return Ok(Self {
                entries: vec![kept],
                missing,
            });
        }

        if let Some(start_at) = start_at {
            let at = match start_at {
                StartAt::Id(id) => entries.iter().position(|entry| &entry.backend == id),
                StartAt::Row(row) => entries.iter().position(|entry| entry.row == *row),
            }
            .ok_or_else(|| {
                Error::Arguments(match start_at {
                    StartAt::Id(id) => {
                        format!("`start_at` names `{id}`, which this agent does not list")
                    }
                    StartAt::Row(row) => format!("this agent has no row {row}"),
                })
            })?;
            entries.rotate_left(at);
        }

        Ok(Self { entries, missing })
    }

    /// The row a new conversation starts on.
    #[must_use]
    pub fn first(&self) -> &ResolvedBackend {
        &self.entries[0]
    }

    /// Fasten this resolution to a conversation, on its first row.
    ///
    /// Row 1 and not a choice: `start_at` has already rotated the list, so the row a launch asked to
    /// begin on *is* the first entry, with the rest still behind it.
    #[must_use]
    pub fn pin(self) -> PinnedChain {
        PinnedChain {
            entries: self.entries,
            missing: self.missing,
            at: 0,
        }
    }

    /// The rows whose context window is smaller than one earlier in the list.
    ///
    /// A legitimate configuration and a trap, so this is what the agent form **warns** about rather
    /// than refusing: full replay is only safe while the conversation still fits, and a chain that
    /// steps down in context is a chain whose later switch may not.
    ///
    /// A backend declaring `context_tokens = 0` — unknown — is never warned about. An unknown
    /// window is not evidence of a small one, and a warning nobody can act on is noise.
    ///
    /// Each warning is `(the row it is about, the sentence)`, so a form can put it against that
    /// row rather than in a list at the bottom that nobody connects to anything.
    #[must_use]
    pub fn context_shrink_warnings(&self) -> Vec<(String, String)> {
        let mut warnings = Vec::new();
        let mut largest = 0_u64;
        let mut largest_name = String::new();
        for entry in &self.entries {
            if entry.context_tokens == 0 {
                continue;
            }
            if largest > 0 && entry.context_tokens < largest {
                warnings.push((
                    entry.backend.clone(),
                    format!(
                        "{} holds {} tokens, less than {} above it ({}) — a switch may not fit",
                        entry.backend, entry.context_tokens, largest_name, largest
                    ),
                ));
            }
            if entry.context_tokens > largest {
                largest = entry.context_tokens;
                largest_name.clone_from(&entry.backend);
            }
        }
        warnings
    }

    /// The rows that cannot take a conversation handed to them, by id.
    ///
    /// A pty row can only ever be the row a conversation *starts* on and never leaves. Listing one
    /// beneath another row is a chain that cannot fail over, and the form says so.
    #[must_use]
    pub fn unreplayable(&self) -> Vec<String> {
        self.entries
            .iter()
            .filter(|entry| !entry.replayable)
            .map(|entry| entry.backend.clone())
            .collect()
    }

    /// Every distinct hold key in this chain — what a run asks the hold store about in one query
    /// rather than one per row.
    #[must_use]
    pub fn hold_keys(&self) -> BTreeSet<HoldKey> {
        self.entries.iter().map(ResolvedBackend::hold_key).collect()
    }
}

/// A resolved chain fastened to one conversation, and the row that conversation is on now.
///
/// This is what a session stores, and it is the whole of requirement 5. The list is resolved on the
/// first turn and never resolved again, so editing the agent — or a backend it names, or deleting
/// one out from under it — cannot change what a live chat is talking to. Only [`at`](Self::at)
/// moves, and it moves only when the row it names is forced out.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct PinnedChain {
    /// The rows this conversation may answer on, in the order it will try them — already rotated by
    /// the launch's `start_at`, so index 0 is where it began.
    pub entries: Vec<ResolvedBackend>,
    /// Rows that named a backend the store did not have at the moment of pinning, by id. Kept so a
    /// chat can say why it has fewer places to fall than its agent lists.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub missing: Vec<String>,
    /// Index into [`entries`](Self::entries) of the row answering right now.
    ///
    /// Deliberately **not** serialized with the rest. The list and the position have different
    /// lifetimes — the list is written once, the position moves on every failover — so they are
    /// stored apart, and writing the position into the list's blob as well would be a second copy of
    /// it that the first switch makes wrong. See [`SessionRecord::chain`](crate::SessionRecord).
    #[serde(skip)]
    pub at: usize,
}

impl PinnedChain {
    /// The row answering this conversation right now.
    ///
    /// `None` when the pin is empty, or when its position no longer names a row — which a record
    /// written by a newer build can produce, and which is a thing to fail on rather than to answer
    /// with whatever happens to be at index 0.
    #[must_use]
    pub fn current(&self) -> Option<&ResolvedBackend> {
        self.entries.get(self.at)
    }

    /// Where `backend` sits in this pin, if it is in it at all.
    #[must_use]
    pub fn position(&self, backend: &str) -> Option<usize> {
        self.entries.iter().position(|entry| entry.backend == backend)
    }

    /// The best row this conversation could move to: the earliest one in its own order that is
    /// neither the row it is on nor one the caller calls unavailable.
    ///
    /// It scans from the **top**, not from `at + 1`, and that is deliberate. A conversation never
    /// climbs back on its own — nothing calls this until the row it is on has run out — but once it
    /// is forced to move, the best row it can still reach is the one to move to, even if that row is
    /// above it and has recovered since. Scanning downward instead would send a chat that fell to
    /// row 3 on to row 4 while row 1 sat idle and working.
    ///
    /// `unavailable` is the caller's question, not this module's: it knows about the shared holds,
    /// and about whether a transcript exists that a row would have to be able to take. It is `FnMut`
    /// so the caller can collect *why* each row was passed over as it goes — a "nowhere left to go"
    /// message that cannot name what it tried is one nobody can act on.
    #[must_use]
    pub fn next_available(&self, mut unavailable: impl FnMut(&ResolvedBackend) -> bool) -> Option<usize> {
        self.entries
            .iter()
            .enumerate()
            .find(|(index, entry)| *index != self.at && !unavailable(entry))
            .map(|(index, _)| index)
    }

    /// Point this conversation at another row. `false` — and no move — when there is no such row, so
    /// a miscounted index cannot leave a chat pointed at nothing.
    pub fn move_to(&mut self, index: usize) -> bool {
        if index >= self.entries.len() {
            return false;
        }
        self.at = index;
        true
    }
}

/// Lay one row's overrides over its base backend.
///
/// Precedence, low to high: the backend's own fields, then the row's overrides. The launch's own
/// overrides ride on top later, through the existing [`RunOverrides`](crate::RunOverrides)
/// mechanism, which is applied to the patched agent — so all three layers land in the same place
/// and in the right order without this function knowing about the third.
fn resolve_row(row: &AgentBackendEntry, manifest: &LlmBackendManifest, position: usize) -> ResolvedBackend {
    let mut arguments = manifest.arguments();
    for (key, value) in &row.overrides {
        // A row may not repoint the credential. Dropped rather than raised: an operator hand-editing
        // a manifest gets the backend they named, which is the safe reading, and the save path
        // refuses the same key with an explanation.
        if CREDENTIAL_KEYS.contains(&key.as_str()) {
            continue;
        }
        arguments.insert(key.clone(), value.clone());
    }
    let model = arguments
        .get(MODEL_KEY)
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .to_string();
    ResolvedBackend {
        backend: row.backend.clone(),
        row: position,
        label: manifest.label.clone(),
        runtime: manifest.runtime.clone(),
        credential: manifest.credential(),
        model,
        context_tokens: manifest.context_tokens,
        arguments,
        limit_rules: manifest.limit_rules.clone(),
        probe: manifest.probe.clone(),
        replayable: manifest.is_replayable(),
    }
}

/// Reject an agent's list before it is stored.
///
/// # Errors
/// [`Error::Arguments`] for a row with no backend, or a row overriding a credential key.
pub fn validate_rows(rows: &[AgentBackendEntry]) -> Result<()> {
    for row in rows {
        if row.backend.trim().is_empty() {
            return Err(Error::Arguments("a backend row needs a backend id".into()));
        }
        for key in CREDENTIAL_KEYS {
            if row.overrides.contains_key(key) {
                return Err(Error::Arguments(format!(
                    "row `{}` overrides `{key}`, which names the login — \
                     a different credential means a different backend",
                    row.backend
                )));
            }
        }
    }
    Ok(())
}

/// The catalog shape [`ResolvedChain::resolve`] reads, built from a store listing.
#[must_use]
pub fn catalog(backends: Vec<LlmBackend>) -> BTreeMap<String, LlmBackendManifest> {
    backends
        .into_iter()
        .map(|backend| (backend.id, backend.manifest))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::{Agent, AgentManifest};

    fn catalog_of(entries: &[(&str, LlmBackendManifest)]) -> BTreeMap<String, LlmBackendManifest> {
        entries
            .iter()
            .map(|(id, manifest)| ((*id).to_string(), manifest.clone()))
            .collect()
    }

    fn backend(runtime: Backend, model: &str, context: u64) -> LlmBackendManifest {
        LlmBackendManifest {
            runtime,
            model: model.into(),
            context_tokens: context,
            settings: Some("~/.claude/settings.json".into()),
            ..Default::default()
        }
    }

    fn three() -> BTreeMap<String, LlmBackendManifest> {
        catalog_of(&[
            ("anthropic", backend(Backend::HarnessClaudeSdk, "claude-opus-5", 200_000)),
            (
                "codex",
                LlmBackendManifest {
                    runtime: Backend::ProcessCodex,
                    model: "gpt-5".into(),
                    context_tokens: 400_000,
                    settings: None,
                    provider: Some("openai".into()),
                    ..Default::default()
                },
            ),
            (
                "glm",
                LlmBackendManifest {
                    runtime: Backend::HarnessAdi,
                    model: "glm-5.3".into(),
                    context_tokens: 128_000,
                    settings: None,
                    provider: Some("zai".into()),
                    ..Default::default()
                },
            ),
        ])
    }

    fn rows() -> Vec<AgentBackendEntry> {
        vec![
            AgentBackendEntry {
                backend: "anthropic".into(),
                overrides: [("thinking".to_string(), serde_json::json!("high"))]
                    .into_iter()
                    .collect(),
            },
            AgentBackendEntry::new("codex"),
            AgentBackendEntry::new("glm"),
        ]
    }

    #[test]
    fn a_chain_resolves_in_configured_order() {
        let chain = ResolvedChain::resolve(&rows(), &three(), None, None).expect("resolve");
        let ids: Vec<&str> = chain.entries.iter().map(|e| e.backend.as_str()).collect();
        assert_eq!(ids, ["anthropic", "codex", "glm"]);
        assert_eq!(chain.first().backend, "anthropic");
        assert!(chain.missing.is_empty());
    }

    /// Row 1's overrides are laid over the backend's own fields, and nothing else is.
    #[test]
    fn a_row_override_beats_the_backend_and_leaves_its_neighbours_alone() {
        let chain = ResolvedChain::resolve(&rows(), &three(), None, None).expect("resolve");
        assert_eq!(
            chain.entries[0].arguments["thinking"],
            serde_json::json!("high")
        );
        assert_eq!(chain.entries[0].model, "claude-opus-5");
        assert!(!chain.entries[1].arguments.contains_key("thinking"));
    }

    /// Picking row 3 means 3, then 1, then 2 — the rest still follows behind it.
    #[test]
    fn start_at_rotates_and_keeps_everything_behind_it() {
        let chain = ResolvedChain::resolve(
            &rows(),
            &three(),
            Some(&StartAt::Id("glm".into())),
            None,
        )
        .expect("resolve");
        let ids: Vec<&str> = chain.entries.iter().map(|e| e.backend.as_str()).collect();
        assert_eq!(ids, ["glm", "anthropic", "codex"]);
    }

    #[test]
    fn start_at_also_takes_a_one_based_row_number() {
        let chain =
            ResolvedChain::resolve(&rows(), &three(), Some(&StartAt::Row(2)), None).expect("resolve");
        let ids: Vec<&str> = chain.entries.iter().map(|e| e.backend.as_str()).collect();
        assert_eq!(ids, ["codex", "glm", "anthropic"]);
    }

    /// The row number survives rotation, so a notice can say where a backend sits in the agent's
    /// own list rather than where it landed in this run's order.
    #[test]
    fn the_configured_row_number_survives_rotation() {
        let chain = ResolvedChain::resolve(&rows(), &three(), Some(&StartAt::Row(3)), None)
            .expect("resolve");
        assert_eq!(chain.entries[0].backend, "glm");
        assert_eq!(chain.entries[0].row, 3);
        assert_eq!(chain.entries[1].row, 1);
    }

    #[test]
    fn only_pins_to_one_row_with_nothing_behind_it() {
        let chain =
            ResolvedChain::resolve(&rows(), &three(), None, Some("codex")).expect("resolve");
        assert_eq!(chain.entries.len(), 1);
        assert_eq!(chain.entries[0].backend, "codex");
    }

    #[test]
    fn start_at_naming_something_unlisted_is_an_error() {
        let err = ResolvedChain::resolve(
            &rows(),
            &three(),
            Some(&StartAt::Id("nope".into())),
            None,
        )
        .expect_err("refused");
        assert!(err.to_string().contains("does not list"), "{err}");
    }

    /// A deleted backend shortens the chain rather than stopping the agent — but it is reported,
    /// because a chain that fails over less far than its operator believes is worth knowing about.
    #[test]
    fn a_row_naming_a_deleted_backend_is_skipped_and_reported() {
        let mut rows = rows();
        rows.insert(1, AgentBackendEntry::new("ghost"));
        let chain = ResolvedChain::resolve(&rows, &three(), None, None).expect("resolve");
        let ids: Vec<&str> = chain.entries.iter().map(|e| e.backend.as_str()).collect();
        assert_eq!(ids, ["anthropic", "codex", "glm"]);
        assert_eq!(chain.missing, vec!["ghost".to_string()]);
    }

    #[test]
    fn an_agent_with_no_rows_or_no_resolvable_rows_is_an_error() {
        assert!(ResolvedChain::resolve(&[], &three(), None, None).is_err());
        let ghost = vec![AgentBackendEntry::new("ghost")];
        assert!(ResolvedChain::resolve(&ghost, &three(), None, None).is_err());
    }

    /// One backend listed twice under different models stays legal in a hand-edited file, and the
    /// resolver must not break on it — it is only absent from the interface and the docs.
    #[test]
    fn a_hand_written_repeat_of_one_base_still_resolves() {
        let rows = vec![
            AgentBackendEntry::new("anthropic"),
            AgentBackendEntry {
                backend: "anthropic".into(),
                overrides: [("model".to_string(), serde_json::json!("claude-sonnet-5"))]
                    .into_iter()
                    .collect(),
            },
        ];
        let chain = ResolvedChain::resolve(&rows, &three(), None, None).expect("resolve");
        assert_eq!(chain.entries.len(), 2);
        assert_eq!(chain.entries[0].model, "claude-opus-5");
        assert_eq!(chain.entries[1].model, "claude-sonnet-5");
        // Same subscription, different model — so the two hold keys differ in the model alone.
        assert_eq!(chain.entries[0].credential, chain.entries[1].credential);
        assert_ne!(chain.entries[0].hold_key(), chain.entries[1].hold_key());
        // A bare id resolves to the first row using it; a row number picks the other.
        let by_row = ResolvedChain::resolve(&rows, &three(), Some(&StartAt::Row(2)), None)
            .expect("resolve");
        assert_eq!(by_row.first().model, "claude-sonnet-5");
    }

    /// An override that repointed the credential would make the shared hold describe a
    /// subscription nobody was using, so it is refused on the way in and ignored on the way out.
    #[test]
    fn a_row_may_not_repoint_the_credential() {
        let rows = vec![AgentBackendEntry {
            backend: "anthropic".into(),
            overrides: [("settings".to_string(), serde_json::json!("~/.claude/other.json"))]
                .into_iter()
                .collect(),
        }];
        assert!(validate_rows(&rows).is_err());

        let chain = ResolvedChain::resolve(&rows, &three(), None, None).expect("resolve");
        assert_eq!(
            chain.entries[0].arguments["settings"],
            serde_json::json!("~/.claude/settings.json"),
            "the backend's own credential wins"
        );
    }

    #[test]
    fn applying_a_row_patches_the_runtime_and_the_arguments() {
        let chain = ResolvedChain::resolve(&rows(), &three(), None, None).expect("resolve");
        let agent = Agent {
            name: "adi-agent".to_string(),
            manifest: AgentManifest {
                backend: Backend::HarnessAdi,
                arguments: [
                    ("system_prompt".to_string(), serde_json::json!("Be useful")),
                    ("model".to_string(), serde_json::json!("stale-model")),
                ]
                .into_iter()
                .collect(),
                ..Default::default()
            },
        };

        let run = chain.first().apply(&agent);
        assert_eq!(run.manifest.backend, Backend::HarnessClaudeSdk);
        assert_eq!(run.manifest.arguments["model"], serde_json::json!("claude-opus-5"));
        assert_eq!(run.manifest.arguments["thinking"], serde_json::json!("high"));
        assert_eq!(
            run.manifest.arguments["system_prompt"],
            serde_json::json!("Be useful"),
            "the agent's identity is re-applied unchanged"
        );
    }

    /// A manifest still carrying model configuration from before the migration must not leak past
    /// the row that is supposed to decide it.
    #[test]
    fn a_stale_credential_on_the_agent_is_cleared_by_the_row() {
        let catalog = catalog_of(&[(
            "codex",
            LlmBackendManifest {
                runtime: Backend::ProcessCodex,
                provider: Some("openai".into()),
                ..Default::default()
            },
        )]);
        let chain = ResolvedChain::resolve(&[AgentBackendEntry::new("codex")], &catalog, None, None)
            .expect("resolve");
        let agent = Agent {
            name: "old".to_string(),
            manifest: AgentManifest {
                arguments: [
                    ("settings".to_string(), serde_json::json!("~/.claude/settings.glm.json")),
                    ("base_url".to_string(), serde_json::json!("https://api.z.ai")),
                ]
                .into_iter()
                .collect(),
                ..Default::default()
            },
        };
        let run = chain.first().apply(&agent);
        assert!(!run.manifest.arguments.contains_key("settings"));
        assert!(!run.manifest.arguments.contains_key("base_url"));
        assert_eq!(run.manifest.arguments["provider"], serde_json::json!("openai"));
    }

    #[test]
    fn a_chain_that_steps_down_in_context_warns_once_per_smaller_row() {
        let chain = ResolvedChain::resolve(&rows(), &three(), None, None).expect("resolve");
        // anthropic 200k, codex 400k, glm 128k — only glm is smaller than something above it.
        let warnings = chain.context_shrink_warnings();
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert_eq!(warnings[0].0, "glm");
        assert!(warnings[0].1.contains("glm"), "{warnings:?}");
    }

    #[test]
    fn a_chain_that_only_grows_warns_about_nothing() {
        let rows = vec![AgentBackendEntry::new("glm"), AgentBackendEntry::new("codex")];
        let chain = ResolvedChain::resolve(&rows, &three(), None, None).expect("resolve");
        assert!(chain.context_shrink_warnings().is_empty());
    }

    /// An unknown window is not evidence of a small one.
    #[test]
    fn an_unknown_context_size_never_warns() {
        let catalog = catalog_of(&[
            ("big", backend(Backend::HarnessAdi, "m", 400_000)),
            ("unknown", backend(Backend::HarnessAdi, "m", 0)),
        ]);
        let rows = vec![AgentBackendEntry::new("big"), AgentBackendEntry::new("unknown")];
        let chain = ResolvedChain::resolve(&rows, &catalog, None, None).expect("resolve");
        assert!(chain.context_shrink_warnings().is_empty());
    }

    /// A pty row cannot be handed a conversation, so a chain containing one is reported as unable
    /// to fail over through it.
    #[test]
    fn pty_rows_are_reported_as_unreplayable() {
        let catalog = catalog_of(&[
            ("pty", backend(Backend::PtyClaude, "claude-opus-5", 200_000)),
            ("harness", backend(Backend::HarnessAdi, "glm-5.3", 128_000)),
        ]);
        let rows = vec![AgentBackendEntry::new("pty"), AgentBackendEntry::new("harness")];
        let chain = ResolvedChain::resolve(&rows, &catalog, None, None).expect("resolve");
        assert_eq!(chain.unreplayable(), vec!["pty".to_string()]);
    }

    #[test]
    fn the_hold_keys_of_a_chain_are_collected_once() {
        let chain = ResolvedChain::resolve(&rows(), &three(), None, None).expect("resolve");
        assert_eq!(chain.hold_keys().len(), 3);
    }

    #[test]
    fn a_row_describes_itself_with_its_model() {
        let chain = ResolvedChain::resolve(&rows(), &three(), None, None).expect("resolve");
        assert_eq!(chain.first().describe(), "anthropic (claude-opus-5)");
    }

    /// A launch that starts on row 3 still pins the whole list, and pins it *starting* there — the
    /// rotation has already happened, so the pin's first entry is what the conversation opens on.
    #[test]
    fn pinning_starts_a_conversation_on_the_row_it_was_launched_at() {
        let chain = ResolvedChain::resolve(&rows(), &three(), Some(&StartAt::Id("glm".into())), None)
            .expect("resolve");
        let pinned = chain.pin();
        assert_eq!(pinned.at, 0);
        assert_eq!(pinned.current().map(|row| row.backend.as_str()), Some("glm"));
        assert_eq!(pinned.entries.len(), 3, "the rest of the list is still behind it");
        assert_eq!(pinned.entries[0].row, 3, "and each row remembers where it was configured");
    }

    /// The move is *best-first*, not next-down. Nothing calls this until the current row has run
    /// out, and at that moment the best row still reachable is the one to take — even one above,
    /// which a chat that fell to row 3 would otherwise never come back to.
    #[test]
    fn the_next_row_is_the_best_available_one_and_never_the_one_in_use() {
        let mut pinned = ResolvedChain::resolve(&rows(), &three(), None, None)
            .expect("resolve")
            .pin();

        // Row 1 is spent: the chat falls to row 2.
        let next = pinned
            .next_available(|entry| entry.backend == "anthropic")
            .expect("a row is left");
        assert!(pinned.move_to(next));
        assert_eq!(pinned.current().map(|row| row.backend.as_str()), Some("codex"));

        // Row 2 runs out too, and row 1 has recovered in the meantime — that is the row to take.
        let next = pinned.next_available(|entry| entry.backend == "codex").expect("a row is left");
        assert!(pinned.move_to(next));
        assert_eq!(
            pinned.current().map(|row| row.backend.as_str()),
            Some("anthropic"),
            "a forced move re-picks the best row, not the next one down",
        );

        assert_eq!(pinned.position("glm"), Some(2));
        assert!(!pinned.move_to(9), "a row that is not there is not moved to");
        assert_eq!(pinned.current().map(|row| row.backend.as_str()), Some("anthropic"));
    }

    /// Every row held is the case that has to stop and ask rather than pick something.
    #[test]
    fn a_chain_with_nothing_available_offers_no_row() {
        let pinned = ResolvedChain::resolve(&rows(), &three(), None, None)
            .expect("resolve")
            .pin();
        assert_eq!(pinned.next_available(|_| true), None);
    }

    /// The position is the session record's column, not part of the pinned list — one truth each,
    /// so a switch cannot leave the blob disagreeing with the row a conversation is actually on.
    #[test]
    fn a_pinned_chain_serializes_its_list_without_its_position() {
        let mut pinned = ResolvedChain::resolve(&rows(), &three(), None, None)
            .expect("resolve")
            .pin();
        pinned.move_to(2);

        let json = serde_json::to_value(&pinned).expect("serialize");
        assert!(json.get("at").is_none(), "{json}");

        let back: PinnedChain = serde_json::from_value(json).expect("deserialize");
        assert_eq!(back.entries, pinned.entries, "the list survives the round trip");
        assert_eq!(back.at, 0, "and the position comes back from its own column");
    }

    #[test]
    fn rows_round_trip_through_toml_on_an_agent() {
        let text = toml::to_string_pretty(&serde_json::json!({ "backends": rows() }))
            .expect("toml");
        let back: BTreeMap<String, Vec<AgentBackendEntry>> = toml::from_str(&text).expect("parse");
        assert_eq!(back["backends"], rows());
    }
}
