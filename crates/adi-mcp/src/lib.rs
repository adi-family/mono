//! adi-mcp — register and read *MCP server definitions*: what an ADI agent needs to reach one
//! more Model Context Protocol server than the one ADI already gives every Claude-backed run of
//! its own (see `adi_agents::backends::mcp`). A pure library (no CLI, no daemon), mirroring
//! [`adi_agents::llm::LlmBackends`] closely enough that the two command groups read the same way:
//! `list` / `show` / `save <id>` / `rm <id>`, a whole-object save rather than a patch.
//!
//! Each server is one file, `mcp/<id>.toml` under the store, named by its filename the way a
//! tool or an LLM backend is (no `id` field to drift from what the file is called) — global, or
//! filed under a project with [`Manifest::project`], the same choice a tool makes.
//!
//! **A definition never stores a secret's value, only the secret's name it needs** — see
//! [`Value`]. Resolving that reference against a specific agent's attached secrets is
//! `adi_agents`' job (the same allowlist its own `secrets` field already uses), not this crate's:
//! an MCP server definition has no agent of its own and no idea which agent's launch is reading
//! it.
//!
//! ```
//! # let tmp = std::env::temp_dir().join(format!("adi-mcp-doctest-{}", std::process::id()));
//! # let _ = std::fs::remove_dir_all(&tmp);
//! use adi_mcp::{Manifest, McpServers, Transport, Value};
//! use std::collections::BTreeMap;
//!
//! # let store = McpServers::with_config(adi_config::Config::with_root(&tmp));
//! // In real code: let store = McpServers::open();
//! let manifest = Manifest::new(
//!     "n8n",
//!     Transport::Http {
//!         url: "http://n8n.adi/mcp/".into(),
//!         headers: BTreeMap::from([("Authorization".to_string(), Value::secret("N8N_TOKEN"))]),
//!     },
//! );
//! let server = store.save("n8n", manifest)?;
//! assert_eq!(server.manifest.transport.required_secrets().len(), 1);
//! # std::fs::remove_dir_all(&tmp).ok();
//! # Ok::<(), adi_mcp::Error>(())
//! ```

mod error;
mod transport;

pub use error::{Error, Result};
pub use transport::{Transport, Value};

use serde::{Deserialize, Serialize};

use adi_config::{Config, Module};

/// The store module MCP server definitions live under: `~/.adi/mono/mcp/<id>.toml`.
const MODULE: &str = "mcp";

/// One MCP server's metadata manifest — the `<id>.toml` file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    /// A human-facing display name. Defaults to the id when created without one.
    #[serde(default)]
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// The project this server is filed under (its id), or `None` for a global server.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    pub transport: Transport,
    #[serde(default)]
    pub created_at: u64,
    #[serde(default)]
    pub updated_at: u64,
}

impl Manifest {
    /// A new, unstamped manifest with `name` as both its display name and (by convention) the id
    /// it is about to be saved under — `save` stamps the timestamps, so this never does.
    #[must_use]
    pub fn new(name: impl Into<String>, transport: Transport) -> Self {
        Self {
            name: name.into(),
            description: None,
            project: None,
            transport,
            created_at: 0,
            updated_at: 0,
        }
    }

    /// Builder-style: file this server under a project.
    #[must_use]
    pub fn with_project(mut self, project: impl Into<String>) -> Self {
        self.project = Some(project.into());
        self
    }

    /// Builder-style: attach a one-line description.
    #[must_use]
    pub fn with_description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }
}

/// A registered MCP server definition: its id (the filename stem) plus its loaded [`Manifest`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct McpServer {
    pub id: String,
    pub manifest: Manifest,
}

impl McpServer {
    /// The display name, falling back to the id when the manifest's name is blank.
    #[must_use]
    pub fn display_name(&self) -> &str {
        if self.manifest.name.trim().is_empty() {
            &self.id
        } else {
            &self.manifest.name
        }
    }
}

/// Validate a server id: a single, filesystem-safe path segment — the same security boundary
/// `adi_tools` enforces, since ids arrive from the CLI and the HTTP API and are joined onto the
/// store path.
fn validate_id(id: &str) -> Result<()> {
    adi_config::validate_name(id, Error::InvalidId)
}

/// The MCP server registry: lists, reads, and edits the per-server manifests under the `mcp`
/// module dir. Cheap to clone; all state is on disk.
#[derive(Debug, Clone)]
pub struct McpServers {
    config: Config,
}

impl Default for McpServers {
    fn default() -> Self {
        Self::open()
    }
}

impl McpServers {
    /// Open the registry backed by the standard store (`~/.adi/mono`, honoring `$ADI_DIR`).
    #[must_use]
    pub fn open() -> Self {
        Self { config: Config::open() }
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

    fn module(&self) -> Module {
        self.config.module(MODULE)
    }

    /// The `mcp` directory: `~/.adi/mono/mcp`.
    #[must_use]
    pub fn dir(&self) -> std::path::PathBuf {
        self.module().dir().to_path_buf()
    }

    /// Every registered server, sorted by id. A store nothing has written to yet is an empty
    /// list, not an error.
    ///
    /// # Errors
    /// [`Error::Config`] on a directory read failure or an unreadable definition.
    pub fn list(&self) -> Result<Vec<McpServer>> {
        let entries = match std::fs::read_dir(self.dir()) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(adi_config::Error::Io(e).into()),
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
            if let Some(server) = self.get(id)? {
                out.push(server);
            }
        }
        out.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(out)
    }

    /// One server by id, or `None` when there is no such definition.
    ///
    /// # Errors
    /// [`Error::InvalidId`] for an unsafe id, or [`Error::Config`] if the definition exists but
    /// cannot be read.
    pub fn get(&self, id: &str) -> Result<Option<McpServer>> {
        validate_id(id)?;
        let file = self.module().manifest_file::<Manifest>(id);
        if !file.exists() {
            return Ok(None);
        }
        Ok(Some(McpServer {
            id: id.to_string(),
            manifest: file.load()?,
        }))
    }

    /// Create or replace a server definition under `id`, stamping the timestamps the same way
    /// [`adi_agents::llm::LlmBackends::save`] does: `created_at` kept from whatever already stood
    /// at `id`, `updated_at` always fresh.
    ///
    /// # Errors
    /// [`Error::InvalidId`] for an unsafe id, or [`Error::Config`] if it cannot be written.
    pub fn save(&self, id: &str, mut manifest: Manifest) -> Result<McpServer> {
        validate_id(id)?;
        let now = adi_config::now_unix();
        let existing = self.get(id)?;
        manifest.created_at = existing.as_ref().map_or(now, |server| server.manifest.created_at);
        manifest.updated_at = now;
        if manifest.name.trim().is_empty() {
            manifest.name = id.to_string();
        }
        self.module().manifest_file(id).save(&manifest)?;
        Ok(McpServer { id: id.to_string(), manifest })
    }

    /// Permanently delete a server definition. Returns `false` if it wasn't there.
    ///
    /// Unlike a tool, there is no soft delete: a server definition carries no script and no
    /// `.bin` shim to keep working while disabled, so the one state worth having is "exists" —
    /// the agent attachments that name a deleted id are not followed here, the same tolerance
    /// `adi_agents` already has for a dangling tool id or secret attachment (ADI-MONO-175's own
    /// doc comment).
    ///
    /// # Errors
    /// [`Error::InvalidId`] for an unsafe id, or [`Error::Config`] on a removal failure.
    pub fn delete(&self, id: &str) -> Result<bool> {
        validate_id(id)?;
        Ok(self.module().remove_manifest(id)?)
    }

    /// Follow a project rename into this registry: every server filed under `from` is re-filed
    /// under `to`. Returns how many manifests changed.
    ///
    /// # Errors
    /// [`Error::Config`] if a manifest can't be written, plus everything [`list`](Self::list) can
    /// return.
    pub fn rename_project(&self, from: &str, to: &str) -> Result<usize> {
        let mut changed = 0;
        for mut server in self.list()? {
            if server.manifest.project.as_deref() != Some(from) {
                continue;
            }
            server.manifest.project = Some(to.to_string());
            self.module().manifest_file(&server.id).save(&server.manifest)?;
            changed += 1;
        }
        Ok(changed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn scratch(tag: &str) -> McpServers {
        let dir = std::env::temp_dir().join(format!(
            "adi-mcp-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id(),
        ));
        let _ = std::fs::remove_dir_all(&dir);
        McpServers::with_config(Config::with_root(dir))
    }

    fn http(url: &str) -> Transport {
        Transport::Http { url: url.to_string(), headers: BTreeMap::new() }
    }

    #[test]
    fn save_then_get_round_trips_and_stamps_timestamps() {
        let store = scratch("round-trip");
        let saved = store.save("n8n", Manifest::new("n8n", http("http://n8n.adi/mcp/"))).expect("save");
        assert!(saved.manifest.created_at > 0);
        assert_eq!(saved.manifest.created_at, saved.manifest.updated_at);

        let got = store.get("n8n").expect("get").expect("exists");
        assert_eq!(got, saved);
    }

    #[test]
    fn a_blank_name_falls_back_to_the_id() {
        let store = scratch("blank-name");
        let saved = store
            .save("n8n", Manifest::new("", http("http://n8n.adi/mcp/")))
            .expect("save");
        assert_eq!(saved.manifest.name, "n8n");
        assert_eq!(saved.display_name(), "n8n");
    }

    #[test]
    fn re_saving_keeps_created_at_but_moves_updated_at() {
        let store = scratch("re-save");
        let first = store.save("n8n", Manifest::new("n8n", http("http://n8n.adi/mcp/"))).expect("save");
        std::thread::sleep(std::time::Duration::from_millis(1100));
        let second = store
            .save("n8n", Manifest::new("n8n", http("http://n8n.adi/other")))
            .expect("re-save");
        assert_eq!(second.manifest.created_at, first.manifest.created_at);
        assert!(second.manifest.updated_at >= first.manifest.updated_at);
        assert!(matches!(second.manifest.transport, Transport::Http { ref url, .. } if url == "http://n8n.adi/other"));
    }

    #[test]
    fn get_and_delete_report_absence_rather_than_failing() {
        let store = scratch("absence");
        assert_eq!(store.get("nope").expect("get"), None);
        assert!(!store.delete("nope").expect("delete"));
    }

    #[test]
    fn delete_removes_it_and_list_reflects_the_change() {
        let store = scratch("delete");
        store.save("a", Manifest::new("a", http("http://a"))).expect("save a");
        store.save("b", Manifest::new("b", http("http://b"))).expect("save b");
        assert_eq!(store.list().expect("list").len(), 2);

        assert!(store.delete("a").expect("delete"));
        let ids: Vec<String> = store.list().expect("list").into_iter().map(|s| s.id).collect();
        assert_eq!(ids, ["b"]);
    }

    #[test]
    fn an_unsafe_id_is_rejected_before_touching_disk() {
        let store = scratch("unsafe-id");
        for id in ["", ".", "..", "a/b", "sneaky/../x"] {
            assert!(matches!(store.get(id), Err(Error::InvalidId(_))), "{id:?}");
            assert!(
                matches!(store.save(id, Manifest::new("x", http("http://x"))), Err(Error::InvalidId(_))),
                "{id:?}"
            );
        }
    }

    #[test]
    fn rename_project_refiles_only_the_matching_servers() {
        let store = scratch("rename-project");
        store
            .save("mine", Manifest::new("mine", http("http://x")).with_project("acme"))
            .expect("save");
        store.save("global", Manifest::new("global", http("http://y"))).expect("save");

        let changed = store.rename_project("acme", "acme-2").expect("rename");
        assert_eq!(changed, 1);
        assert_eq!(
            store.get("mine").expect("get").expect("exists").manifest.project,
            Some("acme-2".to_string())
        );
        assert_eq!(store.get("global").expect("get").expect("exists").manifest.project, None);
    }

    #[test]
    fn list_is_empty_for_a_fresh_store_rather_than_erroring() {
        let store = scratch("fresh");
        assert_eq!(store.list().expect("list"), Vec::new());
    }
}
