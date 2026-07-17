//! adi-agents — reusable agent definitions ([`AgentManifest`]) for the adi platform: a pure
//! library (no CLI, no daemon) over the shared [`adi_config`] store. Each agent is one
//! `<name>.toml` file under `~/.adi/mono/agents/`, holding ADI control metadata plus a strictly
//! typed backend argument object.
//!
//! This is the **definition/store layer** of the larger adi-agents orchestration spec
//! (docs/adi-agents.md), plus the first slice of its run layer: [`Agents::run`] launches a
//! tmux-backed agent in an `adi-agent-<name>` session or a headless `process:*` agent in the
//! background (see [`run`](mod@crate::run) via the re-exports below). The full backend contract
//! and lifecycle hooks from that spec are still future work.
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
mod backends;
mod error;
mod run;
pub mod wasm;

use std::path::PathBuf;

use adi_config::{Config, ConfigFile};

pub use agent::{Agent, AgentManifest, RawAgentArguments, StoredAgent, StoredAgentManifest};
pub use error::{Error, Result};
pub use run::{
    Launch, capture_pane, is_runnable, launch, running_sessions, send_keys, session_name, stop,
};
pub use wasm::DispatchOutcome;

use agent::{now_unix, validate_name};
use run::{is_running_in, launch_in, stop_in};

/// The store module agents live under, and each agent file's extension.
const AGENTS_MODULE: &str = "agents";
/// The module dir wasm employees are installed under (`~/.adi/mono/workforce`).
const WORKFORCE_MODULE: &str = "workforce";
/// Runtime state for launched agents (`~/.adi/mono/sessions`).
const SESSIONS_MODULE: &str = "sessions";
const MANIFEST_EXT: &str = "toml";

/// The agents registry: lists, reads, and mutates the per-agent manifests under the `agents`
/// module dir. Cheap to clone; all state is on disk.
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
    /// Open the registry backed by the standard store (`~/.adi/mono`, honoring `$ADI_DIR`).
    #[must_use]
    pub fn open() -> Self {
        Self {
            config: Config::open(),
        }
    }

    /// Open the registry backed by a caller-supplied [`Config`] — for tests or alternate installs.
    #[must_use]
    pub fn with_config(config: Config) -> Self {
        Self { config }
    }

    /// The store this registry reads from.
    #[must_use]
    pub fn config(&self) -> &Config {
        &self.config
    }

    /// The `agents` directory: `~/.adi/mono/agents`.
    #[must_use]
    pub fn dir(&self) -> PathBuf {
        self.config.module(AGENTS_MODULE).dir().to_path_buf()
    }

    /// The manifest file handle for `name`, at `agents/<name>.toml` (touches no disk).
    fn agent_file(&self, name: &str) -> ConfigFile<StoredAgentManifest> {
        self.config
            .module(AGENTS_MODULE)
            .file(&format!("{name}.{MANIFEST_EXT}"))
    }

    /// Every registered agent, sorted by name. A file without a `.toml` extension is skipped; a
    /// missing `agents/` dir yields an empty list.
    ///
    /// # Errors
    /// [`Error::Io`] on a directory read failure, or [`Error::Config`] if a manifest is invalid TOML.
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

    /// The agent with this name, or `None` if it isn't registered.
    ///
    /// # Errors
    /// [`Error::InvalidName`] for an unsafe name, or [`Error::Config`] if the manifest is invalid TOML.
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

    /// The agent with this name decoded using a backend's strict argument type.
    ///
    /// # Errors
    /// Returns the same errors as [`Self::get`], plus [`Error::Arguments`] when the stored
    /// argument object does not match `Args`.
    pub fn get_typed<Args: serde::de::DeserializeOwned>(
        &self,
        name: &str,
    ) -> Result<Option<Agent<Args>>> {
        self.get(name)?.map(StoredAgent::into_typed).transpose()
    }

    /// Create or overwrite an agent definition (an upsert), writing its `<name>.toml`. The store
    /// owns the timestamps: `created_at` is preserved across edits (set once on first save),
    /// `updated_at` is stamped every save — any values in `manifest` are ignored.
    ///
    /// # Errors
    /// [`Error::InvalidName`] for an unsafe name, or [`Error::Config`] on a write failure.
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
        Ok(Agent {
            name: name.to_string(),
            manifest,
        })
    }

    /// Launch a registered agent in its backend with the default `run` message.
    ///
    /// # Errors
    /// [`Error::NotFound`] for an unregistered name, plus everything [`launch`] can return.
    pub fn run(&self, name: &str) -> Result<Launch> {
        self.run_with_message(name, "run")
    }

    /// Launch a registered agent, passing `message` to headless process backends. Interactive
    /// tmux backends start without consuming it.
    ///
    /// # Errors
    /// [`Error::NotFound`] for an unregistered name, plus everything [`launch`] can return.
    pub fn run_with_message(&self, name: &str, message: &str) -> Result<Launch> {
        let agent = self
            .get(name)?
            .ok_or_else(|| Error::NotFound(name.to_string()))?;
        let sessions_dir = self.config.module(SESSIONS_MODULE).dir().to_path_buf();
        launch_in(&agent, &sessions_dir, message)
    }

    /// Run a `wasm:*` agent: a synchronous one-shot dispatch of `message` into the compiled
    /// component named by the manifest's `arguments.wasm` (see [`wasm::dispatch`]). Employees are
    /// installed — and their logs live — under the `workforce` module dir.
    ///
    /// # Errors
    /// [`Error::NotFound`] for an unregistered name, [`Error::NotRunnable`] for a non-wasm
    /// backend, plus everything [`wasm::dispatch`] can return.
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
            return Err(Error::NotRunnable(agent.manifest.backend));
        }
        let workforce_dir = self.config.module(WORKFORCE_MODULE).dir().to_path_buf();
        wasm::dispatch(&agent, &workforce_dir, handler, message)
    }

    /// Whether an agent currently has a live tmux session or detached process.
    #[must_use]
    pub fn is_running(&self, agent: &StoredAgent) -> bool {
        let sessions_dir = self.config.module(SESSIONS_MODULE).dir().to_path_buf();
        is_running_in(agent, &sessions_dir)
    }

    /// Stop a running agent using its executor's lifecycle. Returns whether a live run was found
    /// and asked to stop (idempotent).
    ///
    /// # Errors
    /// [`Error::InvalidName`] for an unsafe name, or an executor-specific lifecycle error.
    pub fn stop(&self, name: &str) -> Result<bool> {
        validate_name(name)?;
        let Some(agent) = self.get(name)? else {
            return Ok(false);
        };
        let sessions_dir = self.config.module(SESSIONS_MODULE).dir().to_path_buf();
        stop_in(&agent, &sessions_dir)
    }

    /// Delete an agent definition. Returns `false` if it wasn't registered.
    ///
    /// # Errors
    /// [`Error::InvalidName`] for an unsafe name, or [`Error::Config`] on a removal failure.
    pub fn delete(&self, name: &str) -> Result<bool> {
        validate_name(name)?;
        Ok(self
            .config
            .module(AGENTS_MODULE)
            .remove_raw(&format!("{name}.{MANIFEST_EXT}"))?)
    }
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
    struct LegacyPartialArguments {
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
        assert_eq!(second.manifest.backend, "harness:adi");
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
            "starred = true\nsystem_prompt = \"Legacy prompt\"\nmax_turns = 4\n\n[extra]\nprovider = \"anthropic\"\n",
        )
        .expect("partial manifest");

        let manifest = store
            .get("partial")
            .expect("get")
            .expect("present")
            .manifest;
        assert!(manifest.starred);
        assert_eq!(manifest.backend, "");
        let typed = manifest
            .clone()
            .into_typed::<LegacyPartialArguments>()
            .expect("typed legacy manifest");
        assert_eq!(typed.arguments.system_prompt, "Legacy prompt");
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
}
