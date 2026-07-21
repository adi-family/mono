//! Agent manifests, storage, and execution adapters for ADI.
//!
//! ```
//! # let tmp = std::env::temp_dir().join(format!("adi-agents-doctest-{}", std::process::id()));
//! # let _ = std::fs::remove_dir_all(&tmp);
//! use adi_agents::{Agents, AgentManifest};
//! use adi_agents::arguments::TmuxClaudeArguments;
//!
//! # let store = Agents::with_config(adi_config::Config::with_root(&tmp));
//! // In real code: let store = Agents::open();
//! let spec = AgentManifest {
//!     backend: "tmux:claude".into(),
//!     arguments: TmuxClaudeArguments {
//!         model: Some("opus".into()),
//!         ..Default::default()
//!     },
//!     ..Default::default()
//! };
//! let saved = store.save("athz-solver", spec)?;
//! assert_eq!(saved.name, "athz-solver");
//! assert_eq!(saved.manifest.executor(), "tmux");
//! assert!(saved.manifest.created_at > 0);
//!
//! assert_eq!(store.list()?.len(), 1);
//! assert!(store.delete("athz-solver")?);
//! # std::fs::remove_dir_all(&tmp).ok();
//! # Ok::<(), adi_agents::Error>(())
//! ```

mod agent;
pub mod arguments;
mod backend;
mod backends;
mod error;
mod events;
mod run;
pub mod wasm;

use std::path::PathBuf;

use adi_config::{Config, ConfigFile, now_unix};

pub use agent::{
    Agent, AgentManifest, RawAgentArguments, SecretAttachment, StoredAgent, StoredAgentManifest,
    contains_json_null,
};
pub use backend::Backend;
pub use error::{Error, Result};
pub use events::{
    AgentDeleted, AgentRunStarted, AgentRunStopped, AgentSaved, event_catalog, event_types,
};
pub use run::{
    Launch, Peek, RunInfo, capture_pane, is_runnable, running_sessions, send_keys, session_name,
};
pub use wasm::DispatchOutcome;

use agent::validate_name;
use run::{is_running_in, launch_in, peek_in, peek_run_in, runs_in, stop_in, stop_run_in};

const AGENTS_MODULE: &str = "agents";
const WORKFORCE_MODULE: &str = "workforce";
const SESSIONS_MODULE: &str = "sessions";
const MANIFEST_EXT: &str = "toml";

/// An on-disk agent registry.
#[derive(Debug, Clone)]
pub struct Agents {
    config: Config,
}

impl Default for Agents {
    fn default() -> Self {
        Self::open()
    }
}

impl Agents {
    #[must_use]
    pub fn open() -> Self {
        Self {
            config: Config::open(),
        }
    }

    #[must_use]
    pub fn with_config(config: Config) -> Self {
        Self { config }
    }

    #[must_use]
    pub fn config(&self) -> &Config {
        &self.config
    }

    #[must_use]
    pub fn dir(&self) -> PathBuf {
        self.config.module(AGENTS_MODULE).dir().to_path_buf()
    }

    fn agent_file(&self, name: &str) -> ConfigFile<StoredAgentManifest> {
        self.config.module(AGENTS_MODULE).manifest_file(name)
    }

    /// Returns registered agents sorted by name.
    ///
    /// # Errors
    /// Returns store I/O or manifest decoding errors.
    pub fn list(&self) -> Result<Vec<StoredAgent>> {
        let entries = match std::fs::read_dir(self.dir()) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(Error::Io(e)),
        };

        let mut agents = Vec::new();
        for entry in entries {
            let entry = entry?;
            if !entry.file_type()?.is_file() {
                continue;
            }
            let Ok(file_name) = entry.file_name().into_string() else {
                continue;
            };
            let Some(name) = file_name.strip_suffix(&format!(".{MANIFEST_EXT}")) else {
                continue;
            };
            if validate_name(name).is_err() {
                continue;
            }
            agents.push(Agent {
                name: name.to_string(),
                manifest: self.agent_file(name).load()?,
            });
        }
        agents.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(agents)
    }

    /// # Errors
    /// Returns name validation or manifest decoding errors.
    pub fn get(&self, name: &str) -> Result<Option<StoredAgent>> {
        validate_name(name)?;
        let file = self.agent_file(name);
        if !file.exists() {
            return Ok(None);
        }
        Ok(Some(Agent {
            name: name.to_string(),
            manifest: file.load()?,
        }))
    }

    /// # Errors
    /// Returns errors from [`Self::get`] or argument decoding.
    pub fn get_typed<Args: serde::de::DeserializeOwned>(
        &self,
        name: &str,
    ) -> Result<Option<Agent<Args>>> {
        self.get(name)?.map(StoredAgent::into_typed).transpose()
    }

    /// Upserts an agent, preserving `created_at` and stamping `updated_at`.
    ///
    /// # Errors
    /// Returns name, argument, or store errors.
    pub fn save<Args: serde::Serialize>(
        &self,
        name: &str,
        mut manifest: AgentManifest<Args>,
    ) -> Result<Agent<Args>> {
        validate_name(name)?;
        let file = self.agent_file(name);
        let now = now_unix();
        // Preserve the original creation time on edit; stamp a fresh one on first save.
        manifest.created_at = match file.load() {
            Ok(existing) if existing.created_at > 0 => existing.created_at,
            _ => now,
        };
        manifest.updated_at = now;
        let stored = manifest.to_stored()?;
        arguments::validate_builtin(&stored)?;
        file.save(&stored)?;
        self.emit(
            "adi.agents.saved",
            &AgentSaved {
                agent: name.to_string(),
            },
        );
        Ok(Agent {
            name: name.to_string(),
            manifest,
        })
    }

    /// Publish an `adi.agents.*` event onto the shared bus. Best-effort and fire-and-forget: this
    /// registry neither knows nor cares whether anything subscribes, and a spool failure must
    /// never fail the lifecycle action that caused it. Emitted against **this store's** [`Config`],
    /// so a scratch store stays isolated.
    fn emit(&self, event: &str, payload: &impl serde::Serialize) {
        if let Ok(json) = serde_json::to_string(payload) {
            let _ = adi_events::Events::with_config(self.config.clone()).emit(event, json);
        }
    }

    /// Renames an agent's manifest, keeping its contents and `created_at` intact.
    ///
    /// The rename is a plain file move, so a following [`Self::save`] under the new name behaves
    /// like any other edit. Renaming a *running* agent is refused: sessions are keyed by name
    /// (`adi-agent-<name>`, `sessions/<executor>/<name>.pid`), so the live session would be
    /// orphaned beyond the reach of stop.
    ///
    /// # Errors
    /// [`Error::InvalidName`] for either name, [`Error::NotFound`] when `from` isn't registered,
    /// [`Error::Exists`] when `to` is taken, [`Error::AlreadyRunning`] when `from` is live.
    pub fn rename(&self, from: &str, to: &str) -> Result<()> {
        validate_name(from)?;
        validate_name(to)?;
        if from == to {
            return Ok(());
        }
        let agent = self
            .get(from)?
            .ok_or_else(|| Error::NotFound(from.to_string()))?;
        if self.agent_file(to).path().exists() {
            return Err(Error::Exists(to.to_string()));
        }
        if self.is_running(&agent) {
            return Err(Error::AlreadyRunning(from.to_string()));
        }
        std::fs::rename(self.agent_file(from).path(), self.agent_file(to).path()).map_err(Error::Io)
    }

    /// # Errors
    /// Returns [`Error::NotFound`] or backend launch errors.
    pub fn run(&self, name: &str) -> Result<Launch> {
        self.run_with_message(name, "run")
    }

    /// # Errors
    /// Returns [`Error::NotFound`] or backend launch errors.
    pub fn run_with_message(&self, name: &str, message: &str) -> Result<Launch> {
        let agent = self
            .get(name)?
            .ok_or_else(|| Error::NotFound(name.to_string()))?;
        let sessions_dir = self.config.module(SESSIONS_MODULE).dir().to_path_buf();
        // An agent starts in the ADI mono store root (`~/.adi/mono`) by default, not the launching
        // daemon's cwd — so a run kicked off from the app lands in the ADI store, not $HOME. An
        // agent that sets an explicit `working_dir` still overrides this.
        let base_dir = self.config.root().to_path_buf();
        // Materialize this agent's own `.bin` from its enabled tools and prepend it to the run's
        // PATH, so it can invoke exactly those tools by name. Best-effort: a sync failure (or an
        // agent with no tools) just means no extra bin, never a blocked run.
        let bin_dir = adi_tools::Tools::with_config(self.config.clone())
            .sync_agent_bin(&agent.name, &agent.manifest.bin_tools)
            .ok();
        // The run inherits only the secrets explicitly attached to this agent (an allowlist),
        // exported as env vars under their literal names — nothing is pulled in from a scope just
        // for existing. Resolved against this store's Config, so a test store stays isolated;
        // best-effort, so a missing or undecryptable secret is skipped, never a blocked run.
        let secret_env = attached_secret_env(&self.config, &agent.manifest.secrets);
        let launch = launch_in(
            &agent,
            &sessions_dir,
            &base_dir,
            bin_dir.as_deref(),
            message,
            &secret_env,
        )?;
        self.emit(
            "adi.agents.run.started",
            &AgentRunStarted::of(name, message, &launch),
        );
        Ok(launch)
    }

    /// Dispatches a message synchronously to a `wasm:*` agent.
    ///
    /// # Errors
    /// Returns lookup, backend, component loading, or dispatch errors.
    pub fn run_wasm(
        &self,
        name: &str,
        handler: Option<&str>,
        message: &str,
    ) -> Result<DispatchOutcome> {
        let agent = self
            .get(name)?
            .ok_or_else(|| Error::NotFound(name.to_string()))?;
        if !wasm::is_wasm(&agent) {
            return Err(Error::NotRunnable(agent.manifest.backend.to_string()));
        }
        let workforce_dir = self.config.module(WORKFORCE_MODULE).dir().to_path_buf();
        wasm::dispatch(&agent, &workforce_dir, handler, message)
    }

    #[must_use]
    pub fn is_running(&self, agent: &StoredAgent) -> bool {
        let sessions_dir = self.config.module(SESSIONS_MODULE).dir().to_path_buf();
        is_running_in(agent, &sessions_dir)
    }

    /// A read-only live snapshot of an agent for the live view: a tmux pane capture for interactive
    /// backends, or the latest run's log tail for the headless backends.
    #[must_use]
    pub fn peek(&self, agent: &StoredAgent) -> Peek {
        let sessions_dir = self.config.module(SESSIONS_MODULE).dir().to_path_buf();
        peek_in(agent, &sessions_dir)
    }

    /// The run history of a headless agent, newest first (empty for interactive backends, whose
    /// live session is their only "run").
    #[must_use]
    pub fn runs(&self, agent: &StoredAgent) -> Vec<RunInfo> {
        let sessions_dir = self.config.module(SESSIONS_MODULE).dir().to_path_buf();
        runs_in(agent, &sessions_dir)
    }

    /// A read-only snapshot of one specific run of a headless agent (or the tmux pane, for an
    /// interactive backend, where `run_id` is ignored).
    #[must_use]
    pub fn peek_run(&self, agent: &StoredAgent, run_id: &str) -> Peek {
        let sessions_dir = self.config.module(SESSIONS_MODULE).dir().to_path_buf();
        peek_run_in(agent, &sessions_dir, run_id)
    }

    /// Stops one specific run of an agent, returning whether a live run was found.
    ///
    /// # Errors
    /// Returns name validation or backend lifecycle errors.
    pub fn stop_run(&self, name: &str, run_id: &str) -> Result<bool> {
        validate_name(name)?;
        let Some(agent) = self.get(name)? else {
            return Ok(false);
        };
        let sessions_dir = self.config.module(SESSIONS_MODULE).dir().to_path_buf();
        let stopped = stop_run_in(&agent, &sessions_dir, run_id)?;
        if stopped {
            self.emit(
                "adi.agents.run.stopped",
                &AgentRunStopped {
                    agent: name.to_string(),
                    run_id: Some(run_id.to_string()),
                },
            );
        }
        Ok(stopped)
    }

    /// Stops a run, returning whether one was found.
    ///
    /// # Errors
    /// Returns name validation or backend lifecycle errors.
    pub fn stop(&self, name: &str) -> Result<bool> {
        validate_name(name)?;
        let Some(agent) = self.get(name)? else {
            return Ok(false);
        };
        let sessions_dir = self.config.module(SESSIONS_MODULE).dir().to_path_buf();
        let stopped = stop_in(&agent, &sessions_dir)?;
        if stopped {
            self.emit(
                "adi.agents.run.stopped",
                &AgentRunStopped {
                    agent: name.to_string(),
                    run_id: None,
                },
            );
        }
        Ok(stopped)
    }

    /// # Errors
    /// Returns name validation or store errors.
    pub fn delete(&self, name: &str) -> Result<bool> {
        validate_name(name)?;
        let removed = self.config.module(AGENTS_MODULE).remove_manifest(name)?;
        if removed {
            self.emit(
                "adi.agents.deleted",
                &AgentDeleted {
                    agent: name.to_string(),
                },
            );
        }
        Ok(removed)
    }
}

/// Resolve an agent's attached-secret allowlist into `(env-var, value)` pairs for a run. Only the
/// listed secrets are decrypted — nothing is inherited from a scope for merely existing. A global
/// attachment resolves ahead of a project-scoped one, so a project secret overrides a global of
/// the same name (matching [`adi_secrets::Secrets::resolve`]'s precedence). Best-effort: a secret
/// that is missing or fails to decrypt is skipped rather than aborting the run. An empty allowlist
/// short-circuits, so a secrets-free agent never touches the master key.
fn attached_secret_env(config: &Config, attachments: &[SecretAttachment]) -> Vec<(String, String)> {
    if attachments.is_empty() {
        return Vec::new();
    }
    let secrets = adi_secrets::Secrets::with_config(config.clone());
    // Stable sort by scope: globals (`false`) before project-scoped (`true`), so the latter win
    // on a name collision when inserted into the map below.
    let mut ordered: Vec<&SecretAttachment> = attachments.iter().collect();
    ordered.sort_by_key(|a| a.project.is_some());
    let mut env: std::collections::BTreeMap<String, String> = std::collections::BTreeMap::new();
    for att in ordered {
        if let Ok(Some(value)) = secrets.reveal(att.project.as_deref(), &att.name) {
            env.insert(att.name.clone(), value);
        }
    }
    env.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
    struct CloudManifest {
        region: String,
        replicas: u32,
    }

    #[derive(Debug, Clone, PartialEq, Default, serde::Serialize, serde::Deserialize)]
    struct TestArguments {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        system_prompt: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        model: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        permission_mode: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        temperature: Option<f64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        resume: Option<bool>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cloud_manifest: Option<CloudManifest>,
    }

    #[derive(Debug, PartialEq, serde::Deserialize)]
    struct PartialArguments {
        system_prompt: String,
        max_turns: u64,
        provider: String,
    }

    fn scratch(tag: &str) -> Agents {
        let root = std::env::temp_dir().join(format!(
            "adi-agents-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id(),
        ));
        let _ = std::fs::remove_dir_all(&root);
        Agents::with_config(Config::with_root(root))
    }

    fn spec(backend: &str) -> AgentManifest<TestArguments> {
        AgentManifest {
            backend: backend.into(),
            ..AgentManifest::default()
        }
    }

    #[test]
    fn save_then_get_and_list_round_trip() {
        let store = scratch("crud");
        assert!(store.list().expect("empty list").is_empty());

        let mut m = spec("cloud:worker");
        m.arguments.system_prompt = Some("You are a solver.".into());
        m.arguments.model = Some("opus".into());
        m.arguments.permission_mode = Some("default".into());
        m.tags = vec!["athz".into()];
        m.project = Some("demo".into());
        m.arguments.resume = Some(true);
        m.arguments.cloud_manifest = Some(CloudManifest {
            region: "eu-west-1".into(),
            replicas: 2,
        });
        let saved = store.save("athz-solver", m).expect("save");
        assert_eq!(saved.name, "athz-solver");
        assert_eq!(saved.manifest.arguments.model.as_deref(), Some("opus"));
        assert_eq!(saved.manifest.project.as_deref(), Some("demo"));
        assert_eq!(saved.manifest.arguments.resume, Some(true));
        assert_eq!(
            saved
                .manifest
                .arguments
                .cloud_manifest
                .as_ref()
                .map(|manifest| manifest.replicas),
            Some(2)
        );
        assert!(saved.manifest.created_at > 0);

        let raw =
            std::fs::read_to_string(store.dir().join("athz-solver.toml")).expect("stored manifest");
        let arguments_section = raw.find("[arguments]").expect("arguments table");
        let adi_fields = &raw[..arguments_section];
        assert!(
            !adi_fields
                .lines()
                .any(|line| line.starts_with("system_prompt ="))
        );
        assert!(!adi_fields.lines().any(|line| line.starts_with("model =")));

        let got = store
            .get_typed::<TestArguments>("athz-solver")
            .expect("get")
            .expect("present");
        assert_eq!(got, saved);
        assert_eq!(store.list().expect("list").len(), 1);
    }

    #[test]
    fn save_is_an_upsert_that_preserves_created_at() {
        let store = scratch("upsert");
        let first = store.save("a", spec("process:codex")).expect("create");
        let created = first.manifest.created_at;
        assert!(created > 0);

        let mut edited = spec("harness:adi");
        edited.arguments.temperature = Some(0.2);
        let second = store.save("a", edited).expect("update");
        assert_eq!(second.manifest.backend, Backend::from("harness:adi"));
        assert_eq!(second.manifest.arguments.temperature, Some(0.2));
        assert_eq!(second.manifest.created_at, created);
        assert_eq!(store.list().expect("list").len(), 1);
    }

    #[test]
    fn partial_toml_manifest_uses_struct_defaults() {
        let store = scratch("partial-default");
        std::fs::create_dir_all(store.dir()).expect("agents dir");
        std::fs::write(
            store.dir().join("partial.toml"),
            "starred = true\n\n[arguments]\nsystem_prompt = \"A prompt\"\nmax_turns = 4\nprovider = \"anthropic\"\n",
        )
        .expect("partial manifest");

        let manifest = store
            .get("partial")
            .expect("get")
            .expect("present")
            .manifest;
        assert!(manifest.starred);
        assert_eq!(manifest.backend, Backend::default());
        let typed = manifest
            .clone()
            .into_typed::<PartialArguments>()
            .expect("typed manifest");
        assert_eq!(typed.arguments.system_prompt, "A prompt");
        assert_eq!(typed.arguments.max_turns, 4);
        assert_eq!(typed.arguments.provider, "anthropic");
        assert!(manifest.tags.is_empty());
        assert_eq!(manifest.project, None);
        assert_eq!(manifest.created_at, 0);
        assert_eq!(manifest.updated_at, 0);
    }

    #[test]
    fn delete_removes_the_agent() {
        let store = scratch("delete");
        store.save("gone", spec("tmux:claude")).expect("create");
        assert!(store.delete("gone").expect("delete"));
        assert!(store.get("gone").expect("get").is_none());
        assert!(!store.delete("gone").expect("delete missing"));
    }

    #[test]
    fn harness_claude_sdk_saves_via_the_raw_ui_path_and_is_runnable() {
        // Mirror what the web app / CLI submit: a raw argument map, with numeric knobs encoded as
        // floats (the form runs every number through `parse::<f64>()`).
        let store = scratch("harness-raw");
        let mut arguments = RawAgentArguments::new();
        arguments.insert("model".into(), "claude-opus-4-8".into());
        arguments.insert("permission_mode".into(), "plan".into());
        arguments.insert("max_turns".into(), serde_json::json!(20.0));
        arguments.insert("tools".into(), "tasks,projects".into());
        let manifest = AgentManifest {
            backend: "harness:claude-sdk".into(),
            arguments,
            ..StoredAgentManifest::default()
        };

        let saved = store.save("planner", manifest).expect("save harness agent");
        assert_eq!(saved.manifest.backend, Backend::HarnessClaudeSdk);

        let stored = store
            .get("planner")
            .expect("get")
            .expect("present")
            .manifest;
        assert!(is_runnable(&stored), "harness:claude-sdk must be runnable");

        let typed = store
            .get_typed::<crate::arguments::HarnessClaudeSdkArguments>("planner")
            .expect("typed get")
            .expect("present");
        assert_eq!(typed.manifest.arguments.max_turns, Some(20));
        assert_eq!(
            typed.manifest.arguments.tools.as_deref(),
            Some("tasks,projects")
        );
    }

    #[test]
    fn harness_adi_is_typed_and_stored_but_not_runnable() {
        let store = scratch("harness-adi-raw");
        let mut arguments = RawAgentArguments::new();
        arguments.insert("provider".into(), "gemini".into());
        arguments.insert("temperature".into(), serde_json::json!(0.7));
        arguments.insert("max_tokens".into(), serde_json::json!(4096.0));
        let manifest = AgentManifest {
            backend: "harness:adi".into(),
            arguments,
            ..StoredAgentManifest::default()
        };

        store
            .save("adi-agent", manifest)
            .expect("save adi harness agent");
        let stored = store
            .get("adi-agent")
            .expect("get")
            .expect("present")
            .manifest;
        assert_eq!(stored.backend, Backend::HarnessAdi);
        assert!(!is_runnable(&stored), "harness:adi is not runnable yet");
    }

    #[test]
    fn built_in_backends_reject_unknown_arguments_on_save() {
        #[derive(Default, serde::Serialize)]
        struct MisspelledCodexArguments {
            max_truns: u64,
        }

        let store = scratch("strict-built-in");
        let manifest = AgentManifest {
            backend: "process:codex".into(),
            arguments: MisspelledCodexArguments { max_truns: 4 },
            ..AgentManifest::default()
        };
        assert!(matches!(
            store.save("typo", manifest),
            Err(Error::Arguments(message)) if message.contains("max_truns")
        ));
    }

    #[test]
    fn invalid_names_never_touch_disk() {
        let store = scratch("invalid");
        assert!(matches!(store.get("../escape"), Err(Error::InvalidName(_))));
        assert!(matches!(
            store.save("a/b", spec("tmux:claude")),
            Err(Error::InvalidName(_))
        ));
        assert!(matches!(store.delete(".."), Err(Error::InvalidName(_))));
    }

    #[test]
    fn rename_moves_the_manifest_and_leaves_no_orphan() {
        let store = scratch("rename");
        let mut m = spec("tmux:claude");
        m.arguments.model = Some("opus".into());
        m.tags = vec!["athz".into()];
        let created = store.save("old", m).expect("save").manifest.created_at;

        store.rename("old", "new").expect("rename");

        assert!(store.get("old").expect("old gone").is_none());
        let moved = store
            .get_typed::<TestArguments>("new")
            .expect("load renamed")
            .expect("renamed agent exists");
        assert_eq!(moved.manifest.arguments.model.as_deref(), Some("opus"));
        assert_eq!(moved.manifest.tags, vec!["athz".to_string()]);
        // The rename is a move, so the agent keeps the age it had before.
        assert_eq!(moved.manifest.created_at, created);
        assert_eq!(store.list().expect("list").len(), 1);
    }

    #[test]
    fn rename_refuses_to_clobber_an_existing_agent() {
        let store = scratch("rename-clash");
        store.save("one", spec("tmux:claude")).expect("save one");
        store.save("two", spec("process:codex")).expect("save two");

        assert!(matches!(
            store.rename("one", "two"),
            Err(Error::Exists(name)) if name == "two"
        ));
        // Both survive untouched.
        let two = store.get("two").expect("get two").expect("two exists");
        assert_eq!(two.manifest.backend, "process:codex".into());
        assert_eq!(store.list().expect("list").len(), 2);
    }

    #[test]
    fn secrets_attachment_round_trips_and_stores_as_array_of_tables() {
        let store = scratch("secret-attach");
        let mut m = spec("process:claude");
        m.secrets = vec![
            SecretAttachment {
                project: None,
                name: "API_KEY".into(),
            },
            SecretAttachment {
                project: Some("proj".into()),
                name: "DB_URL".into(),
            },
        ];
        store.save("a", m).expect("save with secrets");

        let got = store.get("a").expect("get").expect("present").manifest;
        assert_eq!(got.secrets.len(), 2);
        assert_eq!(got.secrets[0].project, None);
        assert_eq!(got.secrets[0].name, "API_KEY");
        assert_eq!(got.secrets[1].project.as_deref(), Some("proj"));
        assert_eq!(got.secrets[1].name, "DB_URL");

        // The attachment list is stored as a valid TOML array-of-tables (proven to round-trip by
        // the load above, since `toml::from_str` parsed it back into the two attachments).
        let raw = std::fs::read_to_string(store.dir().join("a.toml")).expect("stored manifest");
        assert!(raw.contains("[[secrets]]"), "expected array-of-tables in {raw}");
        assert!(raw.contains("name = \"API_KEY\""));
        assert!(raw.contains("project = \"proj\""));
    }

    #[test]
    fn an_agent_with_no_attachments_stores_no_secrets_table() {
        let store = scratch("no-secret-attach");
        store.save("a", spec("process:claude")).expect("save");
        let raw = std::fs::read_to_string(store.dir().join("a.toml")).expect("stored manifest");
        // The empty allowlist is skipped on serialization, so pre-secrets manifests are unchanged.
        assert!(!raw.contains("[[secrets]]"));
    }

    #[test]
    fn only_attached_secrets_are_injected_project_scope_winning() {
        let store = scratch("attached-env");
        let secrets = adi_secrets::Secrets::with_config(store.config().clone());
        secrets.set(None, "GLOBAL_ONLY", "g", None).expect("g");
        secrets.set(None, "SHARED", "global", None).expect("shared-g");
        secrets
            .set(None, "NOT_ATTACHED", "ambient", None)
            .expect("ambient");
        secrets.set(Some("proj"), "PROJ_ONLY", "p", None).expect("p");
        secrets
            .set(Some("proj"), "SHARED", "project", None)
            .expect("shared-p");

        let attachments = vec![
            SecretAttachment {
                project: None,
                name: "GLOBAL_ONLY".into(),
            },
            SecretAttachment {
                project: Some("proj".into()),
                name: "PROJ_ONLY".into(),
            },
            // The same key exists in both scopes; the project one must win.
            SecretAttachment {
                project: None,
                name: "SHARED".into(),
            },
            SecretAttachment {
                project: Some("proj".into()),
                name: "SHARED".into(),
            },
            // A dangling reference is skipped, not fatal.
            SecretAttachment {
                project: None,
                name: "MISSING".into(),
            },
        ];
        let env: std::collections::BTreeMap<String, String> =
            attached_secret_env(store.config(), &attachments)
                .into_iter()
                .collect();

        assert_eq!(env.get("GLOBAL_ONLY").map(String::as_str), Some("g"));
        assert_eq!(env.get("PROJ_ONLY").map(String::as_str), Some("p"));
        assert_eq!(env.get("SHARED").map(String::as_str), Some("project"));
        assert!(!env.contains_key("MISSING"));
        // The allowlist is exclusive: a secret that exists but isn't attached is never injected.
        assert!(!env.contains_key("NOT_ATTACHED"));
    }

    #[test]
    fn an_empty_allowlist_injects_nothing_and_touches_no_key() {
        let store = scratch("empty-allowlist");
        assert!(attached_secret_env(store.config(), &[]).is_empty());
    }

    #[test]
    fn rename_validates_both_names_and_no_ops_on_self() {
        let store = scratch("rename-names");
        store.save("keep", spec("tmux:claude")).expect("save");

        assert!(matches!(
            store.rename("keep", "../escape"),
            Err(Error::InvalidName(_))
        ));
        assert!(matches!(
            store.rename("a/b", "keep"),
            Err(Error::InvalidName(_))
        ));
        assert!(matches!(
            store.rename("ghost", "fresh"),
            Err(Error::NotFound(_))
        ));
        store
            .rename("keep", "keep")
            .expect("self rename is a no-op");
        assert!(store.get("keep").expect("still there").is_some());
    }
}
