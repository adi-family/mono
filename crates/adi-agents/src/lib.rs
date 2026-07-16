//! adi-agents — reusable agent definitions ([`AgentManifest`]) for the adi platform: a pure
//! library (no CLI, no daemon) over the shared [`adi_config`] store. Each agent is one
//! `<name>.toml` file under `~/.adi/mono/agents/`, holding its backend, system prompt, tool
//! command scope, model/params, and tags.
//!
//! This is the **definition/store layer** of the larger adi-agents orchestration spec
//! (docs/adi-agents.md), plus the first slice of its run layer: [`Agents::run`] launches a
//! tmux-backed agent (`tmux:claude` / `tmux:codex`) detached in a `adi-agent-<name>` tmux
//! session (see [`run`](mod@crate::run) via the re-exports below). The full backend contract,
//! session persistence, and lifecycle hooks from that spec are still future work.
//!
//! ```
//! # let tmp = std::env::temp_dir().join(format!("adi-agents-doctest-{}", std::process::id()));
//! # let _ = std::fs::remove_dir_all(&tmp);
//! use adi_agents::{Agents, AgentManifest};
//!
//! # let store = Agents::with_config(adi_config::Config::with_root(&tmp));
//! // In real code: let store = Agents::open();
//! let mut spec = AgentManifest::default();
//! spec.backend = "tmux:claude".into();
//! spec.model = Some("opus".into());
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
mod error;
mod run;

use std::path::PathBuf;

use adi_config::{Config, ConfigFile};

pub use agent::{Agent, AgentManifest};
pub use error::{Error, Result};
pub use run::{
    Launch, capture_pane, is_runnable, launch, running_sessions, send_keys, session_name, stop,
};

use agent::{now_unix, validate_name};

/// The store module agents live under, and each agent file's extension.
const AGENTS_MODULE: &str = "agents";
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
    fn agent_file(&self, name: &str) -> ConfigFile<AgentManifest> {
        self.config
            .module(AGENTS_MODULE)
            .file(&format!("{name}.{MANIFEST_EXT}"))
    }

    /// Every registered agent, sorted by name. A file without a `.toml` extension is skipped; a
    /// missing `agents/` dir yields an empty list.
    ///
    /// # Errors
    /// [`Error::Io`] on a directory read failure, or [`Error::Config`] if a manifest is invalid TOML.
    pub fn list(&self) -> Result<Vec<Agent>> {
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
    pub fn get(&self, name: &str) -> Result<Option<Agent>> {
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

    /// Create or overwrite an agent definition (an upsert), writing its `<name>.toml`. The store
    /// owns the timestamps: `created_at` is preserved across edits (set once on first save),
    /// `updated_at` is stamped every save — any values in `manifest` are ignored.
    ///
    /// # Errors
    /// [`Error::InvalidName`] for an unsafe name, or [`Error::Config`] on a write failure.
    pub fn save(&self, name: &str, mut manifest: AgentManifest) -> Result<Agent> {
        validate_name(name)?;
        let file = self.agent_file(name);
        let now = now_unix();
        // Preserve the original creation time on edit; stamp a fresh one on first save.
        manifest.created_at = match file.load() {
            Ok(existing) if existing.created_at > 0 => existing.created_at,
            _ => now,
        };
        manifest.updated_at = now;
        file.save(&manifest)?;
        Ok(Agent {
            name: name.to_string(),
            manifest,
        })
    }

    /// Launch a registered agent in its backend (see [`launch`]): the engine CLI starts detached
    /// in a fresh `adi-agent-<name>` tmux session, and the returned [`Launch`] carries the attach
    /// hint. Only tmux executors run today.
    ///
    /// # Errors
    /// [`Error::NotFound`] for an unregistered name, plus everything [`launch`] can return.
    pub fn run(&self, name: &str) -> Result<Launch> {
        let agent = self
            .get(name)?
            .ok_or_else(|| Error::NotFound(name.to_string()))?;
        launch(&agent)
    }

    /// Stop a running agent (see [`stop`]): kill its `adi-agent-<name>` tmux session. Returns
    /// whether a live session was found and killed (idempotent).
    ///
    /// # Errors
    /// [`Error::InvalidName`] for an unsafe name, or [`Error::Tmux`] if the kill fails.
    pub fn stop(&self, name: &str) -> Result<bool> {
        validate_name(name)?;
        stop(name)
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

    fn scratch(tag: &str) -> Agents {
        let root = std::env::temp_dir().join(format!(
            "adi-agents-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id(),
        ));
        let _ = std::fs::remove_dir_all(&root);
        Agents::with_config(Config::with_root(root))
    }

    fn spec(backend: &str) -> AgentManifest {
        AgentManifest {
            backend: backend.into(),
            ..AgentManifest::default()
        }
    }

    #[test]
    fn save_then_get_and_list_round_trip() {
        let store = scratch("crud");
        assert!(store.list().expect("empty list").is_empty());

        let mut m = spec("tmux:claude");
        m.system_prompt = "You are a solver.".into();
        m.model = Some("opus".into());
        m.permission_mode = Some("default".into());
        m.tags = vec!["athz".into()];
        let saved = store.save("athz-solver", m).expect("save");
        assert_eq!(saved.name, "athz-solver");
        assert_eq!(saved.manifest.model.as_deref(), Some("opus"));
        assert!(saved.manifest.created_at > 0);

        let got = store.get("athz-solver").expect("get").expect("present");
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
        edited.temperature = Some(0.2);
        let second = store.save("a", edited).expect("update");
        assert_eq!(second.manifest.backend, "harness:adi");
        assert_eq!(second.manifest.temperature, Some(0.2));
        // Editing keeps the original creation time.
        assert_eq!(second.manifest.created_at, created);
        assert_eq!(store.list().expect("list").len(), 1);
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
