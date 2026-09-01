//! adi-knowledge — the knowledge base behind `adi-mono knowledge`.
//!
//! A **[`Knowledge`]** is a text note of any length. It lives in a **base**, a named collection
//! at one of three [isolation levels](scope) — global, project, or agent — and it is embedded so
//! that a base can be searched by what a note *means* rather than which words it happens to use.
//!
//! ```no_run
//! use adi_knowledge::{BaseId, KnowledgeStore, NewKnowledge};
//!
//! let store = KnowledgeStore::open();
//! let base: BaseId = "global/runbooks".parse()?;
//! store.ensure_base(&base)?;
//!
//! store.add(&base, NewKnowledge::new(
//!     "Restarting the control panel",
//!     "launchctl kickstart -k gui/$(id -u)/family.adi.app.control-panel",
//! ))?;
//!
//! let hits = store.search(&[base], "how do I bring the panel back up", 5)?;
//! assert_eq!(hits[0].knowledge.title, "Restarting the control panel");
//! # Ok::<(), adi_knowledge::Error>(())
//! ```
//!
//! # Three levels, and why agents can read each other
//!
//! * `global/<name>` — the machine's shared knowledge; everyone reads and writes it.
//! * `project:<id>/<name>` — knowledge about one codebase, invisible outside it.
//! * `agent:<name>/<base>` — one agent's own notes. Its owner writes, and **every other agent
//!   may read**.
//!
//! That last rule is the reason the agent level is a level and not a private file. An agent that
//! worked out how this deployment actually behaves has learned something the next agent needs;
//! what isolation is protecting is the *authorship* of that memory, not its secrecy. See
//! [`Reader::access`] for the whole model in one table.
//!
//! Say plainly what that model is not: it is **not a sandbox**. A [`Reader`] is supplied by the
//! caller, and any process that can construct one can also read the files underneath. What the
//! levels buy is that one agent's memory cannot be *rewritten* by another, and that a run's
//! default view is the knowledge that concerns it. Anything that must actually be kept from a
//! reader belongs in `adi-secrets`, which encrypts it.
//!
//! # Staying embedded
//!
//! Every note carries a `content_hash` over exactly the text that gets embedded, and its vectors
//! record the hash they were made from. The two disagreeing is the definition of
//! [stale](Knowledge::is_stale), and it is what makes "re-embedded whenever they change" a
//! property of the data rather than a promise about call sites: an edit through any path clears
//! the vectors as it writes ([`Backend::put`]), and the write path embeds again immediately.
//! [`reembed`](KnowledgeStore::reembed) is the sweeper for whatever was written while the model
//! was unavailable — and for the day the model changes, which invalidates every vector at once.
//!
//! A note longer than the model's window is **chunked** rather than truncated, with overlap, and
//! ranked at its best chunk (see [`note::chunk`]). "Notes of any length" is otherwise not true.
//!
//! # Pluggable storage
//!
//! What holds a base's notes is a [`Provider`] — SQLite by default, a `HashMap` for tests, and
//! anything else somebody registers. Scoping, access, chunking, staleness, and search are all
//! above that line, so a new backend implements storage and inherits the rest.

// Vector widths, chunk counts, and row counts all cross between `usize`, `u32`, and SQLite's
// `i64` on the way in and out of storage. Every one of them is bounded by construction — a note
// has thousands of chunks at the very outside, and a count read back from a column the schema
// declares non-negative cannot be negative — so the three cast lints would fire on arithmetic
// that has no way to be wrong, and drown the one that might.
#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss
)]

pub mod backend;
pub mod base;
pub mod embed;
pub mod error;
pub mod note;
pub mod scope;

#[cfg(test)]
mod tests;

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use adi_config::{Config, now_unix};
use serde::{Deserialize, Serialize};

pub use backend::{Backend, BaseContext, Provider, Providers};
pub use base::{Base, BaseManifest, BaseRegistry, BaseStatus};
pub use embed::{Embedder, HashEmbedder};
pub use error::{Error, Result};
pub use note::{
    EmbeddingState, Filter, Hit, Knowledge, KnowledgePatch, NewKnowledge, normalize_tags,
};
pub use scope::{Access, BaseId, DEFAULT_BASE, MEMORY_BASE, Reader, Scope};

use backend::Query;
use embed::EmbedderSlot;
use note::{content_hash, embed_text, slug};

/// The store module knowledge lives under: `~/.adi/mono/knowledge`.
const KNOWLEDGE_MODULE: &str = "knowledge";

/// How many chunks are handed to the embedder at once.
///
/// The same 32 the indexer's `EmbeddingConfig` defaults to, and for the same reason: a batch
/// pads to its longest member and attention costs the square of that, so a large batch of
/// unevenly sized texts is slower than several small ones.
const EMBED_BATCH: usize = 32;

/// What [`KnowledgeStore::add`] and [`KnowledgeStore::update`] give back: the note as stored, and
/// whether it came out of the call searchable by meaning.
///
/// Embedding failure is reported rather than raised. A note that could not be embedded — no
/// model on disk and no network to fetch one — is still knowledge, is still full-text
/// searchable, and is still there to be embedded later by
/// [`reembed`](KnowledgeStore::reembed); losing it because a download failed would be the worse
/// outcome. What must not happen is *silence*, so the reason travels with the result.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Saved {
    /// The note as it now stands.
    pub knowledge: Knowledge,
    /// Whether its vectors are current.
    pub embedded: bool,
    /// Why they are not, when they are not.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub embed_error: Option<String>,
}

/// What a [`reembed`](KnowledgeStore::reembed) pass did.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ReembedReport {
    /// How many notes were looked at.
    pub scanned: usize,
    /// How many were embedded on this pass.
    pub embedded: usize,
    /// How many were already current and left alone.
    pub unchanged: usize,
    /// How many chunk vectors were written.
    pub chunks: usize,
    /// The notes that could not be embedded, and why.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub failed: Vec<EmbedFailure>,
    /// The model the pass ran with.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
}

/// One note that could not be embedded.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EmbedFailure {
    /// The note's id.
    pub id: String,
    /// What went wrong.
    pub error: String,
}

/// The knowledge store: bases, notes, and the search over them.
///
/// Cheap to clone; all state is on disk, except a loaded embedding model, which clones share.
///
/// Every call is made **as somebody** — the [`Reader`] the store carries. A store opened with
/// [`open`](Self::open) is the person at the terminal and may do anything; narrow it with
/// [`as_agent`](Self::as_agent) to get an agent's view, where the three levels apply. Carrying
/// the reader rather than passing it per call means no call site can forget to.
#[derive(Debug, Clone)]
pub struct KnowledgeStore {
    bases: BaseRegistry,
    providers: Arc<Providers>,
    embedder: EmbedderSlot,
    reader: Reader,
}

impl Default for KnowledgeStore {
    fn default() -> Self {
        Self::open()
    }
}

impl KnowledgeStore {
    /// Open the store backed by the standard config (`~/.adi/mono`, honoring `$ADI_DIR`), as an
    /// [admin](Reader::admin) reader.
    #[must_use]
    pub fn open() -> Self {
        Self::with_config(Config::open())
    }

    /// Open the store backed by a caller-supplied [`Config`] — for tests or alternate installs.
    #[must_use]
    pub fn with_config(config: Config) -> Self {
        Self {
            bases: BaseRegistry::new(config, KNOWLEDGE_MODULE),
            providers: Arc::new(Providers::builtin()),
            embedder: EmbedderSlot::default(),
            reader: Reader::admin(),
        }
    }

    /// Use a different provider registry — the hook for a backend this crate has never heard of.
    #[must_use]
    pub fn with_providers(mut self, providers: Arc<Providers>) -> Self {
        self.providers = providers;
        self
    }

    /// Use a specific embedder instead of loading the default one.
    #[must_use]
    pub fn with_embedder(mut self, embedder: Arc<dyn Embedder>) -> Self {
        self.embedder = EmbedderSlot::injected(embedder);
        self
    }

    /// The same store seen by `reader`.
    #[must_use]
    pub fn as_reader(mut self, reader: Reader) -> Self {
        self.reader = reader;
        self
    }

    /// The same store seen by one agent, optionally working inside a project.
    #[must_use]
    pub fn as_agent(self, agent: impl Into<String>, project: Option<&str>) -> Self {
        self.as_reader(Reader::agent(agent, project))
    }

    /// Who this store is acting as.
    #[must_use]
    pub fn reader(&self) -> &Reader {
        &self.reader
    }

    /// The store this reads from.
    #[must_use]
    pub fn config(&self) -> &Config {
        self.bases.config()
    }

    /// The provider registry in force.
    #[must_use]
    pub fn providers(&self) -> &Providers {
        &self.providers
    }

    /// The embedding model's name — but **only if one is already loaded**, never by loading one.
    ///
    /// For the places that merely *label* an answer: a status line, a listing, a page header. The
    /// distinction matters because loading is the expensive thing this crate does, and a panel
    /// that printed the model name would otherwise pull ~300MB of weights into memory to fill in
    /// a column nobody asked a question of. `None` means "nothing has needed it yet", which is
    /// not the same as "there isn't one".
    #[must_use]
    pub fn model_name_if_loaded(&self) -> Option<String> {
        self.embedder.model_name_if_known()
    }

    /// The `knowledge` directory: `~/.adi/mono/knowledge`.
    #[must_use]
    pub fn dir(&self) -> PathBuf {
        self.bases.dir()
    }

    /// Where one base's files live.
    #[must_use]
    pub fn base_dir(&self, id: &BaseId) -> PathBuf {
        self.bases.base_dir(id)
    }

    // ---------------------------------------------------------------- bases

    /// Create a base. `provider` defaults to [`backend::SQLITE`].
    ///
    /// # Errors
    /// [`Error::BaseExists`] if it is already there, [`Error::NoSuchProvider`] for an unknown
    /// provider, [`Error::Denied`] when the reader may not write the scope, or a config/IO error.
    pub fn create_base(
        &self,
        id: &BaseId,
        provider: Option<&str>,
        description: Option<&str>,
        settings: BTreeMap<String, String>,
    ) -> Result<Base> {
        self.reader.require_write(id)?;
        if self.bases.exists(id) {
            return Err(Error::BaseExists(id.to_string()));
        }
        let provider = provider.unwrap_or(backend::SQLITE);
        // Fail before writing anything: a manifest naming a provider nothing can open is a base
        // that exists in the listing and answers no question ever asked of it.
        self.providers.get(provider)?;

        let now = now_unix();
        let manifest = BaseManifest {
            provider: provider.to_string(),
            description: clean(description),
            settings,
            created_at: now,
            updated_at: now,
        };
        self.bases.save(id, &manifest)?;
        let base = Base {
            id: id.clone(),
            manifest,
        };
        // Materialize the storage now, so a base that lists is a base that works.
        self.open_backend(&base)?;
        Ok(base)
    }

    /// The base, creating it with defaults if it isn't there yet.
    ///
    /// # Errors
    /// As [`create_base`](Self::create_base), minus [`Error::BaseExists`].
    pub fn ensure_base(&self, id: &BaseId) -> Result<Base> {
        match self.get_base(id)? {
            Some(base) => Ok(base),
            None => self.create_base(id, None, None, BTreeMap::new()),
        }
    }

    /// An agent's own memory base (`agent:<name>/memory`), created if absent.
    ///
    /// # Errors
    /// As [`ensure_base`](Self::ensure_base).
    pub fn ensure_memory(&self, agent: &str) -> Result<Base> {
        self.ensure_base(&BaseId::memory(agent)?)
    }

    /// One base by id, or `None` if it does not exist.
    ///
    /// # Errors
    /// [`Error::Denied`] when the base exists but is not this reader's to see, or a config error.
    pub fn get_base(&self, id: &BaseId) -> Result<Option<Base>> {
        if !self.bases.exists(id) {
            return Ok(None);
        }
        self.reader.require_read(id)?;
        self.bases.load(id)
    }

    /// Every base this reader may see, optionally narrowed to one scope, sorted by id.
    ///
    /// Bases the reader has no access to are **left out**, not refused: "what is there" is a
    /// different question from "let me into this one", and a listing that errored on the first
    /// base belonging to somebody else would be useless to every agent.
    ///
    /// # Errors
    /// A config or IO error while reading the manifests.
    pub fn list_bases(&self, scope: Option<&Scope>) -> Result<Vec<Base>> {
        let mut out = Vec::new();
        for id in self.bases.scan() {
            if scope.is_some_and(|s| *s != id.scope) || self.reader.access(&id).is_none() {
                continue;
            }
            let file = self.bases.manifest_file(&id);
            match file.load() {
                Ok(manifest) => out.push(Base { id, manifest }),
                // A manifest that will not parse is a broken base, not a broken listing.
                Err(e) => tracing::warn!(base = %id, error = %e, "skipping unreadable base"),
            }
        }
        out.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(out)
    }

    /// Delete a base and everything in it. `false` if it wasn't there.
    ///
    /// # Errors
    /// [`Error::Denied`] without write access, or an IO error removing the directory.
    pub fn delete_base(&self, id: &BaseId) -> Result<bool> {
        let Some(base) = self.get_base(id)? else {
            return Ok(false);
        };
        self.reader.require_write(id)?;
        // Ask the provider to drop its contents first: a backend that keeps them somewhere other
        // than this directory (a hosted vector store) would otherwise leak the whole base.
        match self.open_backend(&base) {
            Ok(backend) => backend.clear()?,
            Err(e) => {
                tracing::warn!(base = %id, error = %e, "deleting a base its provider cannot open")
            }
        }
        self.bases.remove(id)?;
        Ok(true)
    }

    /// Follow a project rename into this store: move every base under `project:<from>` to
    /// `project:<to>`. Returns how many bases moved.
    ///
    /// A base is addressed by where it sits (`knowledge/projects/<id>/<base>/`) and nothing inside
    /// it records its own scope, so this is a directory move per base — the notes, their
    /// embeddings, and the provider's storage all travel untouched. Agent definitions naming a
    /// moved base as `project:<from>/<name>` are somebody else's to follow (`adi_agents`).
    ///
    /// Nothing moves unless everything can: the destination is checked for collisions and the
    /// reader for write access to both addresses **before** the first rename, so a refusal leaves
    /// the store exactly as it was.
    ///
    /// # Errors
    /// [`Error::InvalidName`] for an unsafe id, [`Error::BaseExists`] when `to` already has a base
    /// of that name, [`Error::Denied`] when the reader may not write one of the two scopes, or
    /// [`Error::Io`] if a move fails.
    pub fn rename_project(&self, from: &str, to: &str) -> Result<usize> {
        let (from_scope, to_scope) = (Scope::project(from)?, Scope::project(to)?);
        if from == to {
            return Ok(0);
        }
        let root = self.dir();
        let source = root.join(from_scope.rel_dir());

        let mut moving = Vec::new();
        for name in base::child_names(&source) {
            let old = BaseId::new(from_scope.clone(), name.clone())?;
            if !self.bases.exists(&old) {
                continue;
            }
            let new = BaseId::new(to_scope.clone(), name)?;
            self.reader.require_write(&old)?;
            self.reader.require_write(&new)?;
            if self.base_dir(&new).exists() {
                return Err(Error::BaseExists(new.to_string()));
            }
            moving.push((old, new));
        }
        if moving.is_empty() {
            return Ok(0);
        }

        std::fs::create_dir_all(root.join(to_scope.rel_dir()))?;
        for (old, new) in &moving {
            std::fs::rename(self.base_dir(old), self.base_dir(new))?;
        }
        // Best-effort: the old scope dir goes only if the move emptied it.
        let _ = std::fs::remove_dir(&source);
        Ok(moving.len())
    }

    /// What a base holds, and how much of it is currently searchable by meaning.
    ///
    /// # Errors
    /// [`Error::NoSuchBase`], [`Error::Denied`], or a backend error.
    pub fn base_status(&self, id: &BaseId) -> Result<BaseStatus> {
        let base = self.require_base(id)?;
        self.reader.require_read(id)?;
        let backend = self.open_backend(&base)?;
        let notes = backend.list(&Query::default())?;
        // Judge staleness against a model that is already known, never by loading one: `status`
        // is what a person runs to find out whether the model is even available.
        let model = self.embedder.model_name_if_known();
        let (embedded, stale) = match model.as_deref() {
            Some(model) => {
                let fresh = notes.iter().filter(|n| !n.is_stale(model)).count();
                (fresh, notes.len() - fresh)
            }
            None => {
                let any = notes.iter().filter(|n| n.is_embedded()).count();
                (any, notes.len() - any)
            }
        };
        Ok(BaseStatus {
            base,
            notes: notes.len(),
            embedded,
            stale,
            model,
        })
    }

    // ---------------------------------------------------------------- notes

    /// Add a note, embedding it as it goes.
    ///
    /// The id comes from [`NewKnowledge::id`] when given — an importer that must stay idempotent
    /// across runs sets it, and a second add under the same id **replaces** the note. Otherwise
    /// it is derived from the title and made unique with a numeric suffix, so two notes called
    /// "Deploy" are two notes.
    ///
    /// # Errors
    /// [`Error::NoSuchBase`] when the base has not been created, [`Error::Denied`] without write
    /// access, [`Error::Empty`] for a note with neither title nor body, or a backend error.
    /// Embedding failure is **not** an error here — see [`Saved`].
    pub fn add(&self, base: &BaseId, new: NewKnowledge) -> Result<Saved> {
        let stored = self.require_base(base)?;
        self.reader.require_write(base)?;
        let backend = self.open_backend(&stored)?;

        let title = new.title.trim().to_string();
        let body = new.body.trim_end().to_string();
        if title.is_empty() && body.trim().is_empty() {
            return Err(Error::Empty);
        }
        let tags = normalize_tags(new.tags);
        let id = match new.id {
            Some(id) => {
                let id = id.trim().to_string();
                scope::validate_segment(&id)?;
                id
            }
            None => self.unique_id(&*backend, &slug_or_body(&title, &body)?)?,
        };

        let now = now_unix();
        let mut note = Knowledge {
            id,
            base: None,
            content_hash: content_hash(&embed_text(&title, &tags, &body)),
            title,
            body,
            tags,
            source: clean(new.source.as_deref()),
            embedding: EmbeddingState::default(),
            // An explicit id that lands on an existing note keeps that note's birthday.
            created_at: now,
            updated_at: now,
        };
        if let Some(existing) = backend.get(&note.id)? {
            note.created_at = existing.created_at;
        }
        backend.put(&note)?;
        Ok(self.embed_note(&*backend, base, note))
    }

    /// One note, or `None`.
    ///
    /// # Errors
    /// [`Error::NoSuchBase`], [`Error::Denied`] without read access, or a backend error.
    pub fn get(&self, base: &BaseId, id: &str) -> Result<Option<Knowledge>> {
        let stored = self.require_base(base)?;
        self.reader.require_read(base)?;
        let mut note = self.open_backend(&stored)?.get(id)?;
        if let Some(note) = &mut note {
            note.base = Some(base.clone());
        }
        Ok(note)
    }

    /// Edit a note. Absent patch fields are left alone.
    ///
    /// A change to the embedded text (title, body, or tags) invalidates the vectors and they are
    /// made again here. A change to anything else — the source — keeps them, because they are
    /// still accurate.
    ///
    /// # Errors
    /// [`Error::NoSuchKnowledge`], [`Error::NoSuchBase`], [`Error::Denied`] without write access,
    /// [`Error::Empty`] if the edit would leave nothing behind, or a backend error.
    pub fn update(&self, base: &BaseId, id: &str, patch: KnowledgePatch) -> Result<Saved> {
        let stored = self.require_base(base)?;
        self.reader.require_write(base)?;
        let backend = self.open_backend(&stored)?;
        let existing = backend
            .get(id)?
            .ok_or_else(|| Error::NoSuchKnowledge(id.to_string()))?;

        let mut note = existing.clone();
        if let Some(title) = patch.title {
            note.title = title.trim().to_string();
        }
        if let Some(body) = patch.body {
            note.body = body.trim_end().to_string();
        }
        if let Some(tags) = patch.tags {
            note.tags = normalize_tags(tags);
        }
        if let Some(source) = patch.source {
            note.source = clean(source.as_deref());
        }
        if note.title.is_empty() && note.body.trim().is_empty() {
            return Err(Error::Empty);
        }

        note.content_hash = content_hash(&note.embed_text());
        if note == existing {
            // Nothing moved. Don't bump `updated_at` — an edit that changed nothing is not an
            // edit, and a listing sorted by recency should not reorder because someone looked.
            return Ok(self.saved(base, note));
        }
        note.updated_at = now_unix();
        if note.content_hash == existing.content_hash {
            // The embedded text is untouched, so the vectors still describe it: carry them.
            backend.put(&note)?;
            return Ok(self.saved(base, note));
        }
        note.embedding = EmbeddingState::default();
        backend.put(&note)?;
        Ok(self.embed_note(&*backend, base, note))
    }

    /// Delete a note. `false` if it wasn't there.
    ///
    /// # Errors
    /// [`Error::NoSuchBase`], [`Error::Denied`] without write access, or a backend error.
    pub fn remove(&self, base: &BaseId, id: &str) -> Result<bool> {
        let stored = self.require_base(base)?;
        self.reader.require_write(base)?;
        self.open_backend(&stored)?.delete(id)
    }

    /// The notes in a base, newest first.
    ///
    /// # Errors
    /// [`Error::NoSuchBase`], [`Error::Denied`] without read access, or a backend error.
    pub fn list(&self, base: &BaseId, filter: &Filter) -> Result<Vec<Knowledge>> {
        let stored = self.require_base(base)?;
        self.reader.require_read(base)?;
        let backend = self.open_backend(&stored)?;

        // "Only the stale ones" is a question about the current model, which the backend has no
        // way to know — so it is answered here, and the limit is applied after it rather than by
        // the query, which would otherwise cap the scan instead of the answer.
        let query = Query {
            tags: normalize_tags(&filter.tags),
            limit: if filter.stale_only {
                None
            } else {
                filter.limit
            },
        };
        let mut notes = backend.list(&query)?;
        if filter.stale_only {
            let model = self.embedder.get().map(|e| e.model_name().to_string());
            notes.retain(|n| match &model {
                Ok(model) => n.is_stale(model),
                // With no embedder to name a model, "stale" can only mean "never embedded".
                Err(_) => !n.is_embedded(),
            });
            if let Some(limit) = filter.limit {
                notes.truncate(limit);
            }
        }
        for note in &mut notes {
            note.base = Some(base.clone());
        }
        Ok(notes)
    }

    // --------------------------------------------------------------- search

    /// Search `bases` by meaning, best first.
    ///
    /// The query is embedded once and put to every base, so searching a project's knowledge and
    /// another agent's memory together costs one embed, not two.
    ///
    /// # Errors
    /// [`Error::Denied`] for a base this reader may not read, [`Error::NoSuchBase`],
    /// [`Error::Embed`] when the query cannot be embedded, or a backend error.
    pub fn search(&self, bases: &[BaseId], query: &str, limit: usize) -> Result<Vec<Hit>> {
        if bases.is_empty() || limit == 0 || query.trim().is_empty() {
            return Ok(Vec::new());
        }
        let embedder = self.embedder.get()?;
        let vector = embedder
            .embed(&[query])
            .map_err(|e| Error::Embed(e.to_string()))?
            .into_iter()
            .next()
            .unwrap_or_default();
        if vector.is_empty() {
            return Err(Error::Embed("the embedder returned no vector".into()));
        }
        self.gather(bases, limit, |backend| {
            backend.search_vectors(&vector, limit)
        })
    }

    /// Search `bases` by word, best first — no embedder, no model, no network.
    ///
    /// # Errors
    /// [`Error::Denied`] for a base this reader may not read, [`Error::NoSuchBase`], or a
    /// backend error.
    pub fn search_text(&self, bases: &[BaseId], query: &str, limit: usize) -> Result<Vec<Hit>> {
        if bases.is_empty() || limit == 0 || query.trim().is_empty() {
            return Ok(Vec::new());
        }
        self.gather(bases, limit, |backend| backend.search_text(query, limit))
    }

    /// Every base this reader may read — what `search` is handed when nobody named a base.
    ///
    /// # Errors
    /// A config or IO error while listing.
    pub fn visible_bases(&self) -> Result<Vec<BaseId>> {
        Ok(self.list_bases(None)?.into_iter().map(|b| b.id).collect())
    }

    // ------------------------------------------------------------ embedding

    /// Embed everything in a base that needs it.
    ///
    /// This is the sweeper behind the promise: notes written while the model was unavailable,
    /// and — with `force`, or simply because the model name changed — every note in the base
    /// when the embedding model is swapped. A note whose vectors are already current is left
    /// alone, so a second pass over an up-to-date base costs one listing and no embedding.
    ///
    /// # Errors
    /// [`Error::NoSuchBase`], [`Error::Denied`] without write access, [`Error::Embed`] when no
    /// embedder can be built at all, or a backend error. A *single* note that fails to embed is
    /// recorded in the report rather than abandoning the pass.
    pub fn reembed(&self, base: &BaseId, force: bool) -> Result<ReembedReport> {
        let stored = self.require_base(base)?;
        self.reader.require_write(base)?;
        let backend = self.open_backend(&stored)?;
        let embedder = self.embedder.get()?;
        let model = embedder.model_name().to_string();

        let mut report = ReembedReport {
            model: Some(model.clone()),
            ..ReembedReport::default()
        };
        for note in backend.list(&Query::default())? {
            report.scanned += 1;
            if !force && !note.is_stale(&model) {
                report.unchanged += 1;
                continue;
            }
            match embed_chunks(&*embedder, &note) {
                Ok((state, vectors)) => {
                    let chunks = vectors.len();
                    backend.set_vectors(&note.id, &state, &vectors)?;
                    report.embedded += 1;
                    report.chunks += chunks;
                }
                Err(e) => report.failed.push(EmbedFailure {
                    id: note.id.clone(),
                    error: e.to_string(),
                }),
            }
        }
        Ok(report)
    }

    // ------------------------------------------------------------- internals

    /// Embed one note straight after it was written, folding failure into the result.
    fn embed_note(&self, backend: &dyn Backend, base: &BaseId, mut note: Knowledge) -> Saved {
        let outcome = self
            .embedder
            .get()
            .and_then(|embedder| embed_chunks(&*embedder, &note))
            .and_then(|(state, vectors)| {
                backend.set_vectors(&note.id, &state, &vectors)?;
                Ok(state)
            });
        match outcome {
            Ok(state) => {
                note.embedding = state;
                self.saved(base, note)
            }
            Err(e) => {
                tracing::warn!(id = %note.id, base = %base, error = %e, "stored unembedded");
                let mut saved = self.saved(base, note);
                saved.embed_error = Some(e.to_string());
                saved
            }
        }
    }

    fn saved(&self, base: &BaseId, mut note: Knowledge) -> Saved {
        note.base = Some(base.clone());
        Saved {
            embedded: note.is_embedded(),
            embed_error: None,
            knowledge: note,
        }
    }

    /// Run one search over several bases and merge the results.
    fn gather(
        &self,
        bases: &[BaseId],
        limit: usize,
        run: impl Fn(&dyn Backend) -> Result<Vec<backend::ChunkHit>>,
    ) -> Result<Vec<Hit>> {
        let mut hits = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for id in bases {
            if !seen.insert(id.clone()) {
                continue;
            }
            let stored = self.require_base(id)?;
            self.reader.require_read(id)?;
            let backend = self.open_backend(&stored)?;
            for hit in run(&*backend)? {
                if let Some(mut note) = backend.get(&hit.id)? {
                    note.base = Some(id.clone());
                    hits.push(Hit {
                        knowledge: note,
                        score: hit.score,
                        chunk: hit.chunk,
                    });
                }
            }
        }
        hits.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.knowledge.id.cmp(&b.knowledge.id))
        });
        hits.truncate(limit);
        Ok(hits)
    }

    fn require_base(&self, id: &BaseId) -> Result<Base> {
        self.bases
            .load(id)?
            .ok_or_else(|| Error::NoSuchBase(id.to_string()))
    }

    fn open_backend(&self, base: &Base) -> Result<Arc<dyn Backend>> {
        let ctx = BaseContext {
            id: base.id.clone(),
            dir: self.base_dir(&base.id),
            settings: base.manifest.settings.clone(),
        };
        self.providers.get(&base.manifest.provider)?.open(&ctx)
    }

    /// A free id near `stem`: `stem`, else `stem-2`, `stem-3`, …
    fn unique_id(&self, backend: &dyn Backend, stem: &str) -> Result<String> {
        if backend.get(stem)?.is_none() {
            return Ok(stem.to_string());
        }
        for n in 2..1000 {
            let candidate = format!("{stem}-{n}");
            if backend.get(&candidate)?.is_none() {
                return Ok(candidate);
            }
        }
        Err(Error::InvalidName(format!(
            "{stem}: a thousand notes already share this title"
        )))
    }
}

/// Resolve the knowledge bases an agent works with, from the fields on its definition.
///
/// This is deliberately a function over plain values rather than a method on an agent type: the
/// knowledge store must not depend on the agent registry (the dependency runs the other way),
/// and taking `&[String]` means the same resolver serves the CLI, a tool, and whatever reads an
/// agent definition next.
///
/// * `configured` — the base ids the agent's definition names. Unparseable entries and bases it
///   may not read are dropped, because a definition is a *wish list*: an agent should not fail to
///   start because somebody deleted a base it was pointed at.
/// * `memory` — whether the agent keeps its own memory base, which is prepended when true.
///
/// # Errors
/// [`Error::InvalidName`] when `agent` is not a usable name.
pub fn resolve_agent_bases(
    agent: &str,
    project: Option<&str>,
    configured: &[String],
    memory: bool,
) -> Result<Vec<BaseId>> {
    let reader = Reader::agent(agent, project);
    let mut out = Vec::new();
    if memory {
        out.push(BaseId::memory(agent)?);
    }
    for entry in configured {
        match entry.parse::<BaseId>() {
            Ok(id) if reader.access(&id).is_some() && !out.contains(&id) => out.push(id),
            Ok(id) => {
                tracing::debug!(%agent, base = %id, "knowledge base not readable by this agent")
            }
            Err(e) => tracing::warn!(%agent, entry, error = %e, "unparseable knowledge base"),
        }
    }
    Ok(out)
}

/// Embed every chunk of `note`, and describe what came back.
fn embed_chunks(
    embedder: &dyn Embedder,
    note: &Knowledge,
) -> Result<(EmbeddingState, Vec<Vec<f32>>)> {
    let chunks = note.chunks();
    if chunks.is_empty() {
        return Ok((EmbeddingState::default(), Vec::new()));
    }
    let mut vectors = Vec::with_capacity(chunks.len());
    for batch in chunks.chunks(EMBED_BATCH) {
        let texts: Vec<&str> = batch.iter().map(String::as_str).collect();
        vectors.extend(
            embedder
                .embed(&texts)
                .map_err(|e| Error::Embed(e.to_string()))?,
        );
    }
    if vectors.len() != chunks.len() {
        return Err(Error::Embed(format!(
            "asked for {} vectors and got {}",
            chunks.len(),
            vectors.len()
        )));
    }
    let dimensions = vectors.first().map_or(0, Vec::len) as u32;
    Ok((
        EmbeddingState {
            model: Some(embedder.model_name().to_string()),
            hash: Some(note.content_hash.clone()),
            chunks: vectors.len() as u32,
            dimensions,
        },
        vectors,
    ))
}

/// The id a note gets when nobody named one: from the title, or failing that from the body's
/// opening words — a note pasted in with no title still needs to be addressable.
fn slug_or_body(title: &str, body: &str) -> Result<String> {
    slug(title).or_else(|_| {
        let opening: String = body
            .split_whitespace()
            .take(8)
            .collect::<Vec<_>>()
            .join(" ");
        slug(&opening)
    })
}

/// The names of every subdirectory of `dir`, or nothing if it isn't one.
/// Trim, and treat the empty string as absent.
fn clean(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(ToString::to_string)
}
