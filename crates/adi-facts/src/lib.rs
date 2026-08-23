//! adi-facts — the fact base behind `adi-mono facts`.
//!
//! A **fact** is one plain sentence somebody said, recorded the way they would say it to
//! someone who was not there: *"We do not support the CIS."* It carries who meant it and who
//! wrote it down, and nothing else. No subject, no predicate, no polarity, no tags, no status —
//! all of those were built, measured against the flat sentence, and dropped for buying nothing
//! (`experiment/knowledge-base/DESIGN.md`).
//!
//! ```no_run
//! use adi_facts::{BaseId, FactStore};
//!
//! let store = FactStore::open();
//! let base: BaseId = "global/default".parse()?;
//! store.ensure_base(&base)?;
//!
//! let staging = store.add(&base, "igor", "agent:chat@1", vec![
//!     "The company supports all countries except the CIS.".to_string(),
//! ])?;
//! // Nothing is in the base yet: the pairs needing a decision come back with the transaction.
//! for pair in staging.open() {
//!     println!("{:.3} {} {}", pair.strength, pair.kind, pair.new_text);
//! }
//! # Ok::<(), adi_facts::Error>(())
//! ```
//!
//! # What the machine does, and where it stops
//!
//! The machine embeds a new fact, ranks it against everything already there, and asks a
//! classifier which of the close pairs a reviewer should look at. **It decides nothing.** Above
//! 0.80 similarity the measured base held ten controversies against eight duplicates, and the
//! sixth-ranked pair in the whole base was *"supports all countries except the CIS"* against
//! *"within the CIS, supports Ukraine"* — merging on similarity would have silently deleted the
//! carve-out (`RESULTS.md` §9). So the similarity floor bounds the *search*; a person or a
//! verifier agent rules on every pair, and every ruling records who made it.
//!
//! # Staleness is mechanical
//!
//! A derived node records the ids it was built from and, on each edge, the exact `version` those
//! sources were at. Recompute; a version that no longer matches means the node is out of date —
//! transitively, instantly, with no model in the loop.
//!
//! `version` is a **per-node counter, never a timestamp**. That is the one piece of this design
//! bought with a silent failure: the prototype's first run wrote a fact and edited it inside the
//! same millisecond, the wall-clock stamp never moved, and the edit was invisible to every
//! dependent with no error anywhere. `updated_at` survives only so a human can see when
//! something last moved; nothing compares it.
//!
//! # Three levels, borrowed whole
//!
//! A base is addressed exactly as a knowledge base is — `global/<name>`,
//! `project:<id>/<name>`, `agent:<name>/<base>` — because it *is* the same addressing:
//! [`BaseId`], [`Scope`], and [`Reader`] are `adi_knowledge`'s types, not lookalikes, and the
//! directory layout comes from its [`adi_knowledge::BaseRegistry`]. An agent
//! writes its own base and may read every other agent's.
//!
//! # The embedder is `nomic-embed-text`, and that is not a preference
//!
//! Facts are embedded by `nomic-embed-text` over the same local ollama the classifier uses —
//! **not** by the candle model the indexer and `adi-knowledge` share. Every threshold in this
//! design was measured against that model: the floor, the recall table, the band structure. A
//! different embedder does not shift those numbers, it invalidates them (`RESULTS.md` §8). See
//! [`embed`] for the measurements and for what changing it costs.
//!
//! Vectors from two models must never be compared. Nothing relies on remembering that: every
//! cached vector records the model that produced it, and a row from any other model is treated
//! as absent and re-embedded.

// Sequence numbers, vector widths, and row counts all cross between `usize`, `u32`, and
// SQLite's `i64`. Every one is bounded by construction — a batch is tens of facts, a vector is
// hundreds of floats — so these three lints would fire on arithmetic with no way to be wrong and
// drown the one that might.
#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_precision_loss
)]

pub mod db;
pub mod embed;
pub mod error;
pub mod judge;
pub mod model;
pub mod ollama;
pub mod render;

#[cfg(test)]
mod tests;

use std::collections::BTreeMap;
use std::sync::Arc;

use adi_config::{Config, now_unix};
use adi_knowledge::embed::EmbedderSlot;
use adi_knowledge::{BaseManifest, BaseRegistry, Embedder};

pub use adi_knowledge::{Access, BaseId, Reader, Scope};
pub use embed::OllamaEmbedder;
pub use error::{Error, Result};
pub use judge::{Judge, JudgeError, Judgement, NoJudge, OllamaJudge, Relation};
pub use ollama::Ollama;
pub use model::{
    Committed, Event, Fact, Neighbour, Pending, Reference, Stale, Staging, Truncation, Verdict,
};

use db::Db;
use model::split_reference;

/// The store module facts live under: `~/.adi/mono/facts`.
const FACTS_MODULE: &str = "facts";

/// The similarity below which a pair is not even looked at.
///
/// **Measured, on the model this crate actually embeds with.** `RESULTS.md` §9 classified all
/// 6441 pairs of a 114-fact base with `nomic-embed-text`, found the lowest cosine at which a
/// genuine finding still appeared (0.631), and set the floor below it. `nomic-embed-text` is what
/// [`embed::OllamaEmbedder`] runs, so that number applies here directly and
/// `tests::the_floor_admits_every_hand_labelled_pair_on_the_measured_corpus` reproduces it.
///
/// The floor spends **compute, not attention** — the classifier stands between it and a person
/// and filters hard — which is why it can be generous. Dropping it from 0.60 to 0.55 doubled
/// machine time and added eleven items to a reviewer's queue, of which about four were real.
/// Below 0.50 the return collapses: 1157 more pairs bought 4 more flags, none of which survived
/// reading.
///
/// It belongs to the embedder and to how verbosely the extractor writes — §10 moves the same
/// relation from 0.583 to 0.886 across four phrasings. Change either and it must be measured
/// again, from scratch: classify every pair of a real base once, read the findings by hand, find
/// the lowest cosine at which a genuine one still appears, set the floor below it.
///
/// Override per store with [`FactStore::with_floor`], or per process with `ADI_FACTS_FLOOR`.
pub const DEFAULT_FLOOR: f32 = 0.55;

/// How many pairs one transaction may put in front of a reviewer.
///
/// Whatever is cut is **reported** — how many, and below what strength. A silent cap reads as
/// "nothing else to see".
pub const DEFAULT_MAX_PENDING: usize = 40;

/// The fact store: bases, the facts in them, and the graph over them.
///
/// Cheap to clone; all state is on disk except a loaded embedding model, which clones share.
///
/// Every call is made **as somebody** — the [`Reader`] the store carries. [`open`](Self::open)
/// is the person at the terminal and may do anything; [`as_agent`](Self::as_agent) narrows it to
/// an agent's view. Carrying the reader rather than passing it per call means no call site can
/// forget to.
#[derive(Debug, Clone)]
pub struct FactStore {
    bases: BaseRegistry,
    reader: Reader,
    embedder: EmbedderSlot,
    judge: Arc<dyn Judge>,
    floor: f32,
    max_pending: usize,
}

impl Default for FactStore {
    fn default() -> Self {
        Self::open()
    }
}

impl FactStore {
    /// Open the store backed by the standard config (`~/.adi/mono`, honoring `$ADI_DIR`), as the
    /// owner.
    #[must_use]
    pub fn open() -> Self {
        Self::with_config(Config::open())
    }

    /// Open the store backed by a caller-supplied [`Config`] — for tests or alternate installs.
    #[must_use]
    pub fn with_config(config: Config) -> Self {
        Self {
            bases: BaseRegistry::new(config, FACTS_MODULE),
            reader: Reader::admin(),
            // The slot is kept for its injection point rather than for laziness: building an
            // ollama client loads no weights and opens no socket, so unlike a candle embedder
            // there is nothing here to defer.
            embedder: EmbedderSlot::lazily(default_embedder),
            judge: Arc::new(OllamaJudge::new()),
            floor: env_f32("ADI_FACTS_FLOOR").unwrap_or(DEFAULT_FLOOR),
            max_pending: env_usize("ADI_FACTS_MAX_PENDING").unwrap_or(DEFAULT_MAX_PENDING),
        }
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

    /// Use a specific embedder instead of loading the default one.
    #[must_use]
    pub fn with_embedder(mut self, embedder: Arc<dyn Embedder>) -> Self {
        self.embedder = EmbedderSlot::injected(embedder);
        self
    }

    /// Use a specific classifier/extractor instead of the default local one.
    #[must_use]
    pub fn with_judge(mut self, judge: Arc<dyn Judge>) -> Self {
        self.judge = judge;
        self
    }

    /// Set the similarity floor. See [`DEFAULT_FLOOR`] for why this is a per-embedder number.
    #[must_use]
    pub fn with_floor(mut self, floor: f32) -> Self {
        self.floor = floor;
        self
    }

    /// Cap how many pairs one transaction puts in front of a reviewer.
    #[must_use]
    pub fn with_max_pending(mut self, max_pending: usize) -> Self {
        self.max_pending = max_pending;
        self
    }

    /// Who this store is acting as.
    #[must_use]
    pub fn reader(&self) -> &Reader {
        &self.reader
    }

    /// The similarity floor in force.
    #[must_use]
    pub fn floor(&self) -> f32 {
        self.floor
    }

    /// The classifier in force.
    #[must_use]
    pub fn judge(&self) -> &Arc<dyn Judge> {
        &self.judge
    }

    /// The `facts` directory: `~/.adi/mono/facts`.
    #[must_use]
    pub fn dir(&self) -> std::path::PathBuf {
        self.bases.dir()
    }

    // ---------------------------------------------------------------- bases

    /// The base, creating it if it isn't there.
    ///
    /// # Errors
    /// [`Error::Denied`] when the reader may not write the scope, or an IO/config error.
    pub fn ensure_base(&self, id: &BaseId) -> Result<()> {
        self.reader.require_write(id)?;
        if self.bases.exists(id) {
            return Ok(());
        }
        let now = now_unix();
        self.bases.save(
            id,
            &BaseManifest {
                provider: "sqlite".to_string(),
                description: None,
                settings: BTreeMap::new(),
                created_at: now,
                updated_at: now,
            },
        )?;
        // Materialize the file now, so a base that lists is a base that answers.
        self.open_db(id)?;
        Ok(())
    }

    /// Every fact base this reader may see, sorted by id.
    ///
    /// Bases the reader has no access to are left out rather than refused — "what is there" is a
    /// different question from "let me into this one".
    #[must_use]
    pub fn list_bases(&self, scope: Option<&Scope>) -> Vec<BaseId> {
        let mut out: Vec<BaseId> = self
            .bases
            .scan()
            .into_iter()
            .filter(|id| scope.is_none_or(|s| *s == id.scope) && self.reader.access(id).is_some())
            .collect();
        out.sort();
        out
    }

    /// How many facts a base holds.
    ///
    /// # Errors
    /// [`Error::NoSuchBase`], [`Error::Denied`], or a backend error.
    pub fn count(&self, id: &BaseId) -> Result<usize> {
        Ok(self.read_db(id)?.nodes()?.len())
    }

    // -------------------------------------------------------------- ingestion

    /// Stage facts, rank them against the base, and return what needs deciding.
    ///
    /// One fact per element — this is the bulk path, and fifty at once is the point of it.
    /// Nothing is visible to the base until [`commit`](Self::commit).
    ///
    /// # Errors
    /// [`Error::Denied`], [`Error::NoSuchBase`], [`Error::Embed`] when the model cannot be
    /// loaded, or a backend error. A classifier that cannot be reached is **not** an error: the
    /// pairs come back marked `unclassified` and the reason travels with them.
    pub fn add(&self, base: &BaseId, author: &str, creator: &str, facts: Vec<String>) -> Result<Staging> {
        self.stage(base, author, creator, None, facts)
    }

    /// Store one raw note, extract facts from it, and stage those.
    ///
    /// The fallback path, not the default: a caller in a live conversation has context a
    /// background extractor never will, and every remaining extraction failure measured was
    /// anaphora — a fact whose referent lives in a different note (`CLI.md`). The note is stored
    /// either way, because re-projecting facts from the original is only possible if the
    /// original survived.
    ///
    /// # Errors
    /// As [`add`](Self::add), plus [`Error::Judge`] when the extractor cannot be reached — with
    /// nothing extracted there is nothing to stage, so this one does fail.
    pub fn add_note(
        &self,
        base: &BaseId,
        author: &str,
        creator: &str,
        text: &str,
        note_id: Option<&str>,
    ) -> Result<Staging> {
        let facts = self
            .judge
            .extract(text)
            .map_err(|e| Error::Judge(e.to_string()))?;
        let note_id = note_id.map_or_else(|| format!("note_{:x}", db::now_ms()), ToString::to_string);
        self.stage(base, author, creator, Some((note_id.as_str(), text)), facts)
    }

    fn stage(
        &self,
        base: &BaseId,
        author: &str,
        creator: &str,
        note: Option<(&str, &str)>,
        facts: Vec<String>,
    ) -> Result<Staging> {
        self.reader.require_write(base)?;
        let db = self.read_db(base)?;
        let tx = format!("tx_{:x}", db::now_ms());
        let facts: Vec<String> = facts
            .into_iter()
            .map(|f| f.trim().to_string())
            .filter(|f| !f.is_empty())
            .collect();
        db.stage(&tx, author, creator, note, &facts)?;

        let (candidates, truncated) = self.candidates(&db, &facts)?;
        let pairs: Vec<(&str, &str)> = candidates
            .iter()
            .map(|c| (c.left.as_str(), c.right.as_str()))
            .collect();
        let (judged, judge_error) = match self.judge.classify(&pairs) {
            Ok(judged) => (judged, None),
            // Nothing came back at all. Every candidate still reaches the reviewer — an
            // unreachable classifier must not read as "nothing to decide".
            Err(e) => (
                pairs
                    .iter()
                    .map(|_| Judgement {
                        relation: Relation::Unclassified,
                        why: String::new(),
                    })
                    .collect(),
                Some(e.to_string()),
            ),
        };

        let rows: Vec<db::Candidate> = candidates
            .iter()
            .zip(&judged)
            .filter(|(_, j)| j.relation.is_actionable())
            .map(|(c, j)| db::Candidate {
                new_seq: c.new_seq,
                base_id: c.base_id.clone(),
                base_seq: c.base_seq,
                strength: c.strength,
                kind: j.relation.as_str().to_string(),
                why: j.why.clone(),
            })
            .collect();
        db.record_pending(&tx, &rows)?;

        let mut staging = staging_of(&db, &tx)?;
        staging.truncated = truncated;
        staging.judge_error = judge_error;
        Ok(staging)
    }

    /// Every staged fact against the base **and against its siblings**, above the floor.
    ///
    /// Siblings matter as much as the base: a batch of fifty facts can contradict itself, and a
    /// pair whose two halves are both incoming is the case the prototype got wrong twice.
    fn candidates(&self, db: &Db, facts: &[String]) -> Result<(Vec<Pair>, Option<Truncation>)> {
        if facts.is_empty() {
            return Ok((Vec::new(), None));
        }
        let embedder = self.embedder.get()?;
        let model = embedder.model_name().to_string();

        let nodes = db.nodes()?;
        let mut base_vectors: Vec<(String, String, Vec<f32>)> = Vec::with_capacity(nodes.len());
        for (id, fact) in nodes {
            let vector = vector_of(db, embedder.as_ref(), &model, &id, &fact)?;
            base_vectors.push((id, fact, vector));
        }

        let texts: Vec<&str> = facts.iter().map(String::as_str).collect();
        let staged_vectors = embedder
            .embed(&texts)
            .map_err(|e| Error::Embed(e.to_string()))?;

        let mut out = Vec::new();
        for (i, fact) in facts.iter().enumerate() {
            for (id, other, vector) in &base_vectors {
                let strength = adi_knowledge::backend::cosine(&staged_vectors[i], vector);
                if strength >= self.floor {
                    out.push(Pair {
                        new_seq: i as i64,
                        base_id: Some(id.clone()),
                        base_seq: None,
                        strength,
                        left: fact.clone(),
                        right: other.clone(),
                    });
                }
            }
            for (j, sibling) in facts.iter().enumerate().skip(i + 1) {
                let strength =
                    adi_knowledge::backend::cosine(&staged_vectors[i], &staged_vectors[j]);
                if strength >= self.floor {
                    out.push(Pair {
                        new_seq: i as i64,
                        base_id: None,
                        base_seq: Some(j as i64),
                        strength,
                        left: fact.clone(),
                        right: sibling.clone(),
                    });
                }
            }
        }
        out.sort_by(|a, b| {
            b.strength
                .partial_cmp(&a.strength)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        Ok(db::cap(out, self.max_pending, |c| c.strength))
    }

    /// A staged transaction as it now stands.
    ///
    /// # Errors
    /// [`Error::NoSuchTransaction`], [`Error::Denied`], or a backend error.
    pub fn show(&self, base: &BaseId, tx: &str) -> Result<Staging> {
        self.reader.require_read(base)?;
        let db = self.read_db(base)?;
        staging_of(&db, tx)
    }

    /// Rule on one pair, and apply what that verdict does to the staged batch.
    ///
    /// # Errors
    /// [`Error::NoSuchPair`]; [`Error::MergeNeedsFact`] when `merge` arrives without a sentence;
    /// [`Error::SupersedeNeedsKeep`] and [`Error::KeepIsNotASide`] — the second of which is the
    /// fix for a typo in `--keep` being read as "the base side won" and discarding the incoming
    /// fact in silence.
    #[allow(
        clippy::too_many_arguments,
        reason = "one argument per thing `facts tx resolve` takes; bundling them into a struct \
                  would only move the same list somewhere the CLI has to fill in anyway"
    )]
    pub fn resolve(
        &self,
        base: &BaseId,
        tx: &str,
        pair: i64,
        verdict: Verdict,
        keep: Option<&str>,
        fact: Option<&str>,
        confirmer: &str,
    ) -> Result<Staging> {
        self.reader.require_write(base)?;
        let db = self.read_db(base)?;
        require_open(&db, tx)?;
        let row = db
            .pair(tx, pair)?
            .ok_or_else(|| Error::NoSuchPair {
                tx: tx.to_string(),
                pair,
            })?;
        let (mine, theirs) = (row.mine(), row.theirs());

        let fact = fact.map(str::trim).filter(|f| !f.is_empty());
        if verdict == Verdict::Merge && fact.is_none() {
            return Err(Error::MergeNeedsFact {
                left: row.new_text.clone(),
                right: row.other_text.clone(),
            });
        }
        if verdict == Verdict::Supersede {
            let Some(keep) = keep else {
                return Err(Error::SupersedeNeedsKeep {
                    left: mine,
                    right: theirs,
                });
            };
            if keep != mine && keep != theirs {
                return Err(Error::KeepIsNotASide {
                    keep: keep.to_string(),
                    pair,
                    left: mine,
                    right: theirs,
                });
            }
        }

        db.resolve(tx, &row, verdict, keep, fact, confirmer)?;
        staging_of(&db, tx)
    }

    /// Land a transaction.
    ///
    /// # Errors
    /// [`Error::StillOpen`] while any pair is undecided, naming them; [`Error::Denied`];
    /// [`Error::NoSuchTransaction`].
    pub fn commit(&self, base: &BaseId, tx: &str) -> Result<Committed> {
        self.reader.require_write(base)?;
        let db = self.read_db(base)?;
        require_open(&db, tx)?;
        db.commit(tx)
    }

    /// Discard a whole transaction.
    ///
    /// # Errors
    /// [`Error::NoSuchTransaction`], [`Error::Denied`], or a backend error.
    pub fn abort(&self, base: &BaseId, tx: &str) -> Result<()> {
        self.reader.require_write(base)?;
        let db = self.read_db(base)?;
        require_open(&db, tx)?;
        db.abort(tx)
    }

    // ------------------------------------------------------------------ graph

    /// What is out of date, and which fact changed under it.
    ///
    /// # Errors
    /// [`Error::NoSuchBase`], [`Error::Denied`], or a backend error.
    pub fn stale(&self, base: &BaseId) -> Result<Vec<Stale>> {
        self.reader.require_read(base)?;
        self.read_db(base)?.stale()
    }

    /// A derived node was regenerated: re-stamp its incoming edges, and bump its own version so
    /// anything built on it goes stale in turn.
    ///
    /// # Errors
    /// [`Error::NoSuchFact`], [`Error::Denied`], or a backend error.
    pub fn refresh(&self, base: &BaseId, id: &str) -> Result<()> {
        self.reader.require_write(base)?;
        self.read_db(base)?.refresh(id)
    }

    /// Record a node derived from others — a plan, a summary, anything built *on* facts.
    ///
    /// Each edge is stamped with the source's version right now, which is what makes the
    /// derived node go stale the moment any of those sources moves. Returns the new node's id.
    ///
    /// # Errors
    /// [`Error::NoSuchFact`] when a source is not in the base, [`Error::Denied`], or a backend
    /// error.
    pub fn derive(
        &self,
        base: &BaseId,
        sources: &[String],
        fact: &str,
        author: &str,
        creator: &str,
        kind: &str,
    ) -> Result<String> {
        self.reader.require_write(base)?;
        let db = self.read_db(base)?;
        let id = format!("d_{:x}", db::now_ms());
        db.derive(&id, fact, author, creator, kind, sources)?;
        Ok(id)
    }

    // ---------------------------------------------------------------- reading

    /// The facts closest to one fact, strongest first — the queue around it, for a verifier
    /// agent to work.
    ///
    /// # Errors
    /// [`Error::NoSuchFact`], [`Error::Embed`], [`Error::Denied`], or a backend error.
    pub fn near(&self, base: &BaseId, id: &str, top: usize) -> Result<Vec<Neighbour>> {
        self.reader.require_read(base)?;
        let db = self.read_db(base)?;
        let me = db
            .fact(id)?
            .ok_or_else(|| Error::NoSuchFact(id.to_string()))?;
        let embedder = self.embedder.get()?;
        let model = embedder.model_name().to_string();

        let mut vectors = Vec::new();
        for (node, text) in db.nodes()? {
            let vector = vector_of(&db, embedder.as_ref(), &model, &node, &text)?;
            vectors.push((node, text, vector));
        }
        let mine = vectors
            .iter()
            .find(|(node, _, _)| node == &me.id)
            .map(|(_, _, v)| v.clone())
            .unwrap_or_default();
        let mut out = db::rank(&me.id, &mine, &vectors, self.floor);
        out.truncate(top);
        Ok(out)
    }

    /// What a reference to a fact id resolves to today, and what changed under it.
    ///
    /// `id` may carry the version the reference was written against — `f_abc@1`. That is what
    /// makes an outside reference self-checking: because `merge` and `supersede` rewrite the
    /// winner **in place**, a committed id is never destroyed and a reference never dangles, but
    /// the same id can end up saying the opposite of what it said. A dangling pointer announces
    /// itself; a pointer whose target quietly changed does not.
    ///
    /// # Errors
    /// [`Error::NoSuchFact`], [`Error::Denied`], or a backend error.
    pub fn get(&self, base: &BaseId, id: &str) -> Result<Reference> {
        self.reader.require_read(base)?;
        let db = self.read_db(base)?;
        let (id, referenced) = split_reference(id);
        let fact = db
            .fact(id)?
            .ok_or_else(|| Error::NoSuchFact(id.to_string()))?;
        let history = db.history(id)?;
        let drifted = referenced.is_some_and(|v| v != fact.version);
        let since = match referenced {
            Some(v) if drifted => history.into_iter().filter(|e| e.version > v).collect(),
            _ => history,
        };
        Ok(Reference {
            fact,
            referenced,
            since,
            drifted,
        })
    }

    /// One fact by id, without the history.
    ///
    /// # Errors
    /// [`Error::Denied`] or a backend error.
    pub fn fact(&self, base: &BaseId, id: &str) -> Result<Option<Fact>> {
        self.reader.require_read(base)?;
        self.read_db(base)?.fact(id)
    }

    // ------------------------------------------------------------------ files

    fn open_db(&self, id: &BaseId) -> Result<Db> {
        let dir = self.bases.base_dir(id);
        std::fs::create_dir_all(&dir)?;
        Db::open(&dir.join(db::DB_FILE))
    }

    /// The base's storage, refusing rather than creating when the base is not there.
    ///
    /// `facts add` is the one command that creates a base. Anything else opening one on demand
    /// would turn a mistyped base id into an empty base and an answer of "nothing here".
    fn read_db(&self, id: &BaseId) -> Result<Db> {
        if !self.bases.exists(id) {
            return Err(Error::NoSuchBase(id.to_string()));
        }
        self.open_db(id)
    }
}

/// A transaction as it now stands, whichever call is looking at it.
fn staging_of(db: &Db, tx: &str) -> Result<Staging> {
    let state = db
        .state(tx)?
        .ok_or_else(|| Error::NoSuchTransaction(tx.to_string()))?;
    Ok(Staging {
        tx: tx.to_string(),
        state,
        staged: db.staged(tx)?,
        pending: db.pending(tx)?,
        truncated: None,
        judge_error: None,
    })
}

/// Refuse to work on a transaction that has already landed or been thrown away.
fn require_open(db: &Db, tx: &str) -> Result<()> {
    let state = db
        .state(tx)?
        .ok_or_else(|| Error::NoSuchTransaction(tx.to_string()))?;
    if state == "committed" || state == "aborted" {
        return Err(Error::TransactionClosed {
            tx: tx.to_string(),
            state,
        });
    }
    Ok(())
}

/// A node's vector: the cached one when this model made it, else a fresh one, cached as it goes.
fn vector_of(
    db: &Db,
    embedder: &dyn Embedder,
    model: &str,
    id: &str,
    text: &str,
) -> Result<Vec<f32>> {
    if let Some(vector) = db.cached_vector(id, model)? {
        return Ok(vector);
    }
    let vector = embed_one(embedder, text)?;
    db.store_vector(id, model, &vector)?;
    Ok(vector)
}

/// One candidate pair before the classifier has seen it.
#[derive(Debug, Clone)]
struct Pair {
    new_seq: i64,
    base_id: Option<String>,
    base_seq: Option<i64>,
    strength: f32,
    left: String,
    right: String,
}

/// `nomic-embed-text` on the local ollama — see [`embed`] for why it is that and nothing else.
///
/// # Errors
/// Never, today: building the client cannot fail, and reaching the host is deferred to the first
/// text. The `Result` is [`EmbedderSlot::lazily`]'s signature, which exists for the embedder that
/// has to be *loaded* rather than dialled — `adi-knowledge`'s candle one, which fails on a
/// machine with no model and no network. Collapsing it here would mean this crate could never be
/// pointed at such an embedder.
#[allow(
    clippy::unnecessary_wraps,
    reason = "the shared EmbedderSlot's builder signature; see the doc above"
)]
fn default_embedder() -> adi_knowledge::Result<Arc<dyn Embedder>> {
    Ok(Arc::new(OllamaEmbedder::new()))
}

fn embed_one(embedder: &dyn Embedder, text: &str) -> Result<Vec<f32>> {
    embedder
        .embed(&[text])
        .map_err(|e| Error::Embed(e.to_string()))?
        .into_iter()
        .next()
        .ok_or_else(|| Error::Embed("the embedder returned nothing for one text".to_string()))
}

fn env_f32(key: &str) -> Option<f32> {
    std::env::var(key).ok()?.trim().parse().ok()
}

fn env_usize(key: &str) -> Option<usize> {
    std::env::var(key).ok()?.trim().parse().ok()
}
