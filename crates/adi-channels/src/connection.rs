//! The connection model (`docs/channels.md` §5): one `(provider, routing key)` bound to a
//! [`Target`] and an [`Allowlist`], mirrored on the node so it can route an inbound message without
//! a round trip to the router for every one. Each connection is one `<id>.toml` under
//! `~/.adi/mono/channels/`.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use adi_config::{Config, ConfigFile, Timestamped, now_unix};
use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

const CHANNELS_MODULE: &str = "channels";

/// What a connection dispatches an inbound message to. An enum now, for one variant built today —
/// see `docs/channels.md` §5/§6: `trigger` and `app_route` are named so a bundle-installed channel
/// never has to widen this type later, only add handling for a variant already on the wire. A
/// connection whose target this node doesn't recognize (an older build, reading a store a newer one
/// wrote) deserializes to [`Target::Unknown`] rather than failing — the same tolerance
/// `docs/marketplace-bundles.md` already asks of an unrecognized element `kind`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Target {
    Agent { agent: String },
    Trigger { trigger: String },
    AppRoute { app: String, route: String },
    /// A variant newer than this build knows, kept rather than refused so the rest of the
    /// connection (its allowlist, its thread map) still reads. Dispatch skips it.
    #[serde(other)]
    Unknown,
}

/// Who may talk to the target through this connection (`docs/channels.md` §5). Default is the
/// tightest: only whoever completed the link.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Allowlist {
    #[default]
    OwnerOnly,
    List { sender_ids: Vec<String> },
    Open,
}

impl Allowlist {
    /// Whether `sender_id` may talk to this connection's target. `owner` is the sender id that
    /// completed the link — the one person [`OwnerOnly`](Self::OwnerOnly) means.
    #[must_use]
    pub fn allows(&self, sender_id: &str, owner: &str) -> bool {
        match self {
            Self::OwnerOnly => !owner.is_empty() && sender_id == owner,
            Self::List { sender_ids } => sender_ids.iter().any(|id| id == sender_id),
            Self::Open => true,
        }
    }
}

/// The on-disk shape of one connection. Unknown fields are ignored so it can gain fields without
/// breaking an older store.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConnectionManifest {
    pub provider: String,
    /// Chat id, team id, … — empty until the install/link flow binds it (`docs/channels.md` §1).
    #[serde(default)]
    pub routing_key: String,
    pub target: Target,
    #[serde(default)]
    pub allowlist: Allowlist,
    /// The provider's own sender id for whoever ran `connect`/completed the link — what
    /// [`Allowlist::OwnerOnly`] means. Empty until linked.
    #[serde(default)]
    pub owner_sender_id: String,
    /// Provider thread/chat id -> ADI run id, the one state a connection keeps past the link
    /// (`docs/channels.md` §5).
    #[serde(default)]
    pub threads: BTreeMap<String, String>,
    /// Stop delivering to the target without tearing down the link (§7: "Pause" is a flag here,
    /// not a new endpoint or CLI verb).
    #[serde(default)]
    pub paused: bool,
    /// Whether the install/link flow has bound a `routing_key` yet. A connection a `connect` just
    /// created is unlinked (§7: the CLI/panel wait on this).
    #[serde(default)]
    pub linked: bool,
    #[serde(default)]
    pub created_at: u64,
    #[serde(default)]
    pub updated_at: u64,
}

impl Timestamped for ConnectionManifest {
    fn created_at(&self) -> u64 {
        self.created_at
    }
}

impl ConnectionManifest {
    fn new(provider: String, target: Target) -> Self {
        Self {
            provider,
            routing_key: String::new(),
            target,
            allowlist: Allowlist::default(),
            owner_sender_id: String::new(),
            threads: BTreeMap::new(),
            paused: false,
            linked: false,
            created_at: 0,
            updated_at: 0,
        }
    }
}

/// A connection, named by the id minted at creation.
#[derive(Debug, Clone, PartialEq)]
pub struct Connection {
    pub id: String,
    pub manifest: ConnectionManifest,
}

/// Disambiguates two ids minted within the same millisecond by one process.
static ID_SEQ: AtomicU64 = AtomicU64::new(0);

/// A unique, time-sortable connection id: `<unix_millis>-<seq>` — the same scheme
/// `adi-agents`' session ids use, for the same reason (lexicographic = newest-first).
fn new_id() -> String {
    let ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let seq = ID_SEQ.fetch_add(1, Ordering::Relaxed);
    format!("{ms:013}-{seq:04}")
}

fn validate_id(id: &str) -> Result<()> {
    adi_config::validate_name(id, Error::InvalidId)
}

/// The connection registry: creates, lists, mutates, and maps the connections this node holds.
/// Cheap to clone; all state is on disk.
#[derive(Debug, Clone)]
pub struct Connections {
    config: Config,
}

impl Default for Connections {
    fn default() -> Self {
        Self::open()
    }
}

impl Connections {
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
        self.config.module(CHANNELS_MODULE).dir().to_path_buf()
    }

    fn file(&self, id: &str) -> ConfigFile<ConnectionManifest> {
        self.config.module(CHANNELS_MODULE).manifest_file(id)
    }

    /// Create a new, unlinked connection for `provider` pointed at `target`, under a locally
    /// minted id. Only for a connection with no router counterpart (tests, and any future
    /// standalone target this crate dispatches to without going through the router at all) — the
    /// router-backed path is [`create_with_id`](Self::create_with_id).
    ///
    /// # Errors
    /// [`Error::Config`] on a write failure.
    pub fn create(&self, provider: &str, target: Target) -> Result<Connection> {
        self.create_with_id(&new_id(), provider, target)
    }

    /// Create a new, unlinked connection under `id` — the router's own `connection` id, minted by
    /// `POST /register` in the same call that creates its counterpart there
    /// (`apps/channel-router`'s `do.ts`: `register` mints the id, this node's connect flow is
    /// handed it back and must use it verbatim, since every later frame and `/send` call names
    /// the connection by that id). The install/link flow ([`mark_linked`](Self::mark_linked)) is
    /// what makes it live.
    ///
    /// # Errors
    /// [`Error::InvalidId`] for an unsafe id, or [`Error::Config`] on a write failure.
    pub fn create_with_id(&self, id: &str, provider: &str, target: Target) -> Result<Connection> {
        validate_id(id)?;
        let file = self.file(id);
        let now = now_unix();
        let mut manifest = ConnectionManifest::new(provider.to_string(), target);
        manifest.created_at = now;
        manifest.updated_at = now;
        file.save(&manifest)?;
        Ok(Connection {
            id: id.to_string(),
            manifest,
        })
    }

    /// Every connection, sorted by id (so newest-created sorts last).
    ///
    /// # Errors
    /// [`Error::Config`] on a directory or manifest read failure.
    pub fn list(&self) -> Result<Vec<Connection>> {
        let entries = match std::fs::read_dir(self.dir()) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(adi_config::Error::Io(e).into()),
        };
        let mut connections = Vec::new();
        for entry in entries {
            let entry = entry.map_err(adi_config::Error::Io)?;
            if !entry.file_type().map_err(adi_config::Error::Io)?.is_file() {
                continue;
            }
            let Ok(file_name) = entry.file_name().into_string() else {
                continue;
            };
            let Some(id) = file_name.strip_suffix(&format!(".{}", adi_config::MANIFEST_EXT))
            else {
                continue;
            };
            if validate_id(id).is_err() {
                continue;
            }
            let manifest = self.file(id).load()?;
            connections.push(Connection {
                id: id.to_string(),
                manifest,
            });
        }
        connections.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(connections)
    }

    /// The connection `id`, or `None` if it isn't registered.
    ///
    /// # Errors
    /// [`Error::InvalidId`] for an unsafe id, or [`Error::Config`] if the manifest is invalid.
    pub fn get(&self, id: &str) -> Result<Option<Connection>> {
        validate_id(id)?;
        let file = self.file(id);
        if !file.exists() {
            return Ok(None);
        }
        Ok(Some(Connection {
            id: id.to_string(),
            manifest: file.load()?,
        }))
    }

    /// The connection `id`, or [`Error::NotFound`].
    ///
    /// # Errors
    /// As [`get`](Self::get), plus [`Error::NotFound`] when it doesn't exist.
    pub fn require(&self, id: &str) -> Result<Connection> {
        self.get(id)?.ok_or_else(|| Error::NotFound(id.to_string()))
    }

    fn save(&self, connection: &mut Connection) -> Result<()> {
        connection.manifest.updated_at = now_unix();
        self.file(&connection.id).save(&connection.manifest)?;
        Ok(())
    }

    /// Bind the routing key and owner sender id the install/link flow resolved, and mark the
    /// connection live. Idempotent: linking twice just overwrites the binding (a chat re-linked
    /// after the bot was re-added, say).
    ///
    /// # Errors
    /// As [`require`](Self::require), plus a write failure.
    pub fn mark_linked(&self, id: &str, routing_key: &str, owner_sender_id: &str) -> Result<Connection> {
        let mut connection = self.require(id)?;
        connection.manifest.routing_key = routing_key.to_string();
        connection.manifest.owner_sender_id = owner_sender_id.to_string();
        connection.manifest.linked = true;
        self.save(&mut connection)?;
        Ok(connection)
    }

    /// Change a connection's target — `adi-mono channels route` / `POST /api/channels/route`.
    ///
    /// # Errors
    /// As [`require`](Self::require), plus a write failure.
    pub fn set_target(&self, id: &str, target: Target) -> Result<Connection> {
        let mut connection = self.require(id)?;
        connection.manifest.target = target;
        self.save(&mut connection)?;
        Ok(connection)
    }

    /// Change who may talk — `adi-mono channels allow` / `POST /api/channels/allow`.
    ///
    /// # Errors
    /// As [`require`](Self::require), plus a write failure.
    pub fn set_allowlist(&self, id: &str, allowlist: Allowlist) -> Result<Connection> {
        let mut connection = self.require(id)?;
        connection.manifest.allowlist = allowlist;
        self.save(&mut connection)?;
        Ok(connection)
    }

    /// Pause or resume delivery — folded into `route` rather than a new endpoint (§7 "Decisions
    /// taken" #5).
    ///
    /// # Errors
    /// As [`require`](Self::require), plus a write failure.
    pub fn set_paused(&self, id: &str, paused: bool) -> Result<Connection> {
        let mut connection = self.require(id)?;
        connection.manifest.paused = paused;
        self.save(&mut connection)?;
        Ok(connection)
    }

    /// Remove a connection outright — `adi-mono channels disconnect`. Returns whether it existed.
    /// The router/D1 side of disconnection (dropping the DO entry and node-token revocation, if
    /// nothing else on this node needs the provider) is the caller's job, not this store's — it's
    /// an HTTP call, not a file.
    ///
    /// # Errors
    /// [`Error::InvalidId`] for an unsafe id, or [`Error::Config`] on a removal failure.
    pub fn remove(&self, id: &str) -> Result<bool> {
        validate_id(id)?;
        Ok(self
            .config
            .module(CHANNELS_MODULE)
            .remove_manifest(id)?)
    }

    /// The ADI run id a provider thread already maps to, if the thread has said anything before.
    ///
    /// # Errors
    /// As [`require`](Self::require).
    pub fn thread_run(&self, id: &str, thread: &str) -> Result<Option<String>> {
        Ok(self.require(id)?.manifest.threads.get(thread).cloned())
    }

    /// Record that `thread` now maps to `run_id` — called once, the first time a thread is seen
    /// (`docs/channels.md` §5: "the provider's thread id *is* the connection's only state past the
    /// first message").
    ///
    /// # Errors
    /// As [`require`](Self::require), plus a write failure.
    pub fn bind_thread(&self, id: &str, thread: &str, run_id: &str) -> Result<()> {
        let mut connection = self.require(id)?;
        connection
            .manifest
            .threads
            .insert(thread.to_string(), run_id.to_string());
        self.save(&mut connection)
    }

    /// The connection whose thread map names `run_id`, if any, paired with the provider thread key
    /// itself — what the auto-post-back (`docs/channels.md` §5, "the run's final answer posts back
    /// automatically") and the `channel-reply` tool both resolve a run id through, since neither is
    /// handed a connection id (or a thread key) directly. The thread key matters past the
    /// connection id alone: it's what `RouterApi::send`/`set_thinking` need to tell Slack which
    /// channel in a workspace-wide routing key a reply or a "thinking…" status belongs to.
    ///
    /// # Errors
    /// [`Error::Config`] on a listing failure.
    pub fn find_by_run(&self, run_id: &str) -> Result<Option<(Connection, String)>> {
        for connection in self.list()? {
            if let Some((thread, _)) = connection.manifest.threads.iter().find(|(_, r)| *r == run_id) {
                let thread = thread.clone();
                return Ok(Some((connection, thread)));
            }
        }
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> Connections {
        let root = std::env::temp_dir().join(format!(
            "adi-channels-connections-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id(),
        ));
        let _ = std::fs::remove_dir_all(&root);
        Connections::with_config(Config::with_root(root))
    }

    #[test]
    fn create_list_and_get_round_trip() {
        let store = scratch("crud");
        let created = store
            .create(
                "telegram",
                Target::Agent {
                    agent: "adi-agent".into(),
                },
            )
            .expect("create");
        assert!(!created.manifest.linked);
        assert_eq!(store.list().expect("list").len(), 1);
        let fetched = store.get(&created.id).expect("get").expect("exists");
        assert_eq!(fetched.manifest.provider, "telegram");
    }

    #[test]
    fn an_unknown_target_kind_deserializes_rather_than_failing() {
        let store = scratch("unknown-target");
        let created = store
            .create(
                "telegram",
                Target::Agent {
                    agent: "a".into(),
                },
            )
            .expect("create");
        let path = store.file(&created.id).path().to_path_buf();
        let raw = std::fs::read_to_string(&path).unwrap();
        let raw = raw.replace("kind = \"agent\"", "kind = \"widget\"");
        std::fs::write(&path, raw).unwrap();
        let reloaded = store.get(&created.id).expect("get").expect("exists");
        assert_eq!(reloaded.manifest.target, Target::Unknown);
    }

    #[test]
    fn mark_linked_binds_the_routing_key_and_owner() {
        let store = scratch("link");
        let created = store
            .create(
                "telegram",
                Target::Agent {
                    agent: "adi-agent".into(),
                },
            )
            .expect("create");
        let linked = store
            .mark_linked(&created.id, "12345", "sender-1")
            .expect("link");
        assert!(linked.manifest.linked);
        assert_eq!(linked.manifest.routing_key, "12345");
        assert_eq!(linked.manifest.owner_sender_id, "sender-1");
    }

    #[test]
    fn allowlist_owner_only_admits_only_the_owner() {
        let allow = Allowlist::OwnerOnly;
        assert!(allow.allows("owner", "owner"));
        assert!(!allow.allows("stranger", "owner"));
        // No owner recorded yet (not linked) admits nobody.
        assert!(!allow.allows("owner", ""));
    }

    #[test]
    fn allowlist_list_admits_only_named_senders() {
        let allow = Allowlist::List {
            sender_ids: vec!["a".into(), "b".into()],
        };
        assert!(allow.allows("a", "owner"));
        assert!(!allow.allows("c", "owner"));
    }

    #[test]
    fn allowlist_open_admits_everyone() {
        assert!(Allowlist::Open.allows("anyone", "owner"));
    }

    #[test]
    fn thread_binding_round_trips_and_resolves_back_to_the_connection() {
        let store = scratch("threads");
        let created = store
            .create(
                "telegram",
                Target::Agent {
                    agent: "adi-agent".into(),
                },
            )
            .expect("create");
        assert_eq!(store.thread_run(&created.id, "chat-1").unwrap(), None);
        store.bind_thread(&created.id, "chat-1", "run-1").unwrap();
        assert_eq!(
            store.thread_run(&created.id, "chat-1").unwrap(),
            Some("run-1".to_string())
        );
        let (found, thread) = store.find_by_run("run-1").unwrap().expect("found");
        assert_eq!(found.id, created.id);
        assert_eq!(thread, "chat-1");
        assert!(store.find_by_run("no-such-run").unwrap().is_none());
    }

    #[test]
    fn set_target_allowlist_and_paused_persist() {
        let store = scratch("mutate");
        let created = store
            .create(
                "telegram",
                Target::Agent {
                    agent: "a".into(),
                },
            )
            .expect("create");
        store
            .set_target(
                &created.id,
                Target::Agent {
                    agent: "b".into(),
                },
            )
            .unwrap();
        store
            .set_allowlist(&created.id, Allowlist::Open)
            .unwrap();
        store.set_paused(&created.id, true).unwrap();
        let reloaded = store.require(&created.id).unwrap();
        assert_eq!(
            reloaded.manifest.target,
            Target::Agent { agent: "b".into() }
        );
        assert_eq!(reloaded.manifest.allowlist, Allowlist::Open);
        assert!(reloaded.manifest.paused);
    }

    #[test]
    fn remove_deletes_and_reports_absence() {
        let store = scratch("remove");
        let created = store
            .create(
                "telegram",
                Target::Agent {
                    agent: "a".into(),
                },
            )
            .expect("create");
        assert!(store.remove(&created.id).unwrap());
        assert!(!store.remove(&created.id).unwrap());
        assert!(store.get(&created.id).unwrap().is_none());
    }

    #[test]
    fn require_reports_not_found_for_an_unregistered_id() {
        let store = scratch("notfound");
        assert!(matches!(store.require("ghost"), Err(Error::NotFound(_))));
    }
}
