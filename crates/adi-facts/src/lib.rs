//! adi-facts — the fact base behind `adi-mono facts`.
//!
//! A **fact** is one plain sentence somebody said, recorded the way they would say it to
//! someone who was not there: *"We do not support the CIS."* It carries who meant it and who
//! wrote it down, and nothing else. No subject, no predicate, no polarity, no tags, no status —
//! all of those were built, measured against the flat sentence, and dropped for buying nothing
//! (`experiment/knowledge-base/DESIGN.md`).
//!
//! ```no_run
//! use adi_facts::{BaseId, FactStore, Incoming};
//!
//! let store = FactStore::open();
//! let base: BaseId = "global/default".parse()?;
//! store.ensure_base(&base)?;
//!
//! let staging = store.add(&base, &Incoming::new("igor", "agent:chat@1"), vec![
//!     "The company supports all countries except the CIS.".to_string(),
//! ])?;
//! // Nothing is in the base yet: the pairs needing a decision come back with the transaction.
//! for pair in staging.open() {
//!     println!("{:.3} {} {}", pair.strength, pair.kind, pair.new_text);
//! }
//!
//! // A conclusion drawn from a fact goes through the same door — provenance and checking are
//! // one operation, so an agent never has to choose between them.
//! let conclusion = Incoming::new("igor", "agent:planner@1")
//!     .from_sources(vec!["f_1a02_0".to_string()])
//!     .as_artifact();
//! store.add(&base, &conclusion, vec!["Market entry plan: skip China.".to_string()])?;
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
//! carve-out (`RESULTS.md` §9). So neighbour selection bounds the *search* — the closest
//! [`DEFAULT_TOP_K`] and no more; a person or a verifier agent rules on every pair, and every
//! ruling records who made it.
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
//! design was measured against that model: the recall table, the band structure, the ranking that
//! top-K reads. A different embedder does not shift those numbers, it invalidates them
//! (`RESULTS.md` §8). See [`embed`] for the measurements and for what changing it costs.
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
pub use judge::{Judge, JudgeError, Judgement, NoJudge, OllamaJudge, Relation, Side};
pub use model::{
    Committed, Event, Fact, Neighbour, Pending, Reference, Staging, Stale, Truncation, Verdict,
};
pub use ollama::Ollama;

/// A batch's authorship, provenance, and kind — see [`Incoming`].
pub use self::Incoming as IncomingBatch;

use db::Db;
use model::split_reference;

/// The store module facts live under: `~/.adi/mono/facts`.
const FACTS_MODULE: &str = "facts";

/// How many nearest neighbours a fact is compared against.
///
/// This replaced a **similarity floor**, and the replacement removed a concept rather than
/// renaming one. A threshold admits a roughly constant *fraction* of a base, so what it costs
/// grows with the base: on the live 97-fact `project:adi/business` base, one inserted fact drew
/// 43 pairs above 0.55 and another drew **76 of 96 — 79% of everything there**. Top-K is constant
/// at any size, which is the property the queue actually needs.
///
/// Nor was the floor judging quality. What filters is the classifier: everything selected here
/// goes to it, it answers `independent` for the weak ones, and only what it flags reaches a
/// person. A floor on top of that was not filtering, it was declining to look.
///
/// 20 is the knee, measured on the 114-fact base with 124 actionable pairs: K=10 caught 96,
/// K=20 caught 108 (87%), K=30 caught 112. Thirty buys three points for half again the work.
///
/// Override per store with [`FactStore::with_top_k`], or per process with `ADI_FACTS_TOP_K`.
pub const DEFAULT_TOP_K: usize = 20;

/// How many pairs one transaction may put in front of a reviewer.
///
/// **A backstop against a runaway queue, not a workload control.** What bounds the queue in the
/// normal case is [`DEFAULT_TOP_K`]: a batch of `n` facts selects at most `n × K` pairs however
/// large the base is, so this should not bind at all.
///
/// It binds only when a batch is itself enormous — fifty facts at K=20 can reach a thousand
/// pairs — and that is the runaway it exists to catch. Under the similarity floor it used to bind
/// at ordinary size (one fact against a 97-fact base drew 43 pairs, another 76), which is how a
/// cap ended up silently deciding what a reviewer saw. It no longer does.
///
/// Whatever is cut is **reported**: how many, and below what strength. A silent cap reads as
/// "nothing else to see".
pub const DEFAULT_MAX_PENDING: usize = 200;

/// Who is writing a batch, what it was derived from, and what it becomes.
///
/// `sources` is per **batch**, not per line, and that is deliberate: the input format is one
/// plain sentence per line and stays that way. A caller with two conclusions drawn from two
/// different source sets calls [`add`](FactStore::add) twice — cheap, because the expensive work
/// is per fact either way, and it reads better in a transcript than an encoded line syntax.
#[derive(Debug, Clone)]
pub struct Incoming {
    /// Whose meaning this is — usually the person who said it.
    pub author: String,
    /// Who physically writes the record — usually an agent, with its version.
    pub creator: String,
    /// What every fact in this batch was derived from: committed fact ids, or `#N` for a fact
    /// staged in this same batch. Each becomes an edge at commit, stamped with the source's
    /// version **at commit**.
    pub sources: Vec<String>,
    /// What the batch becomes: `fact` for something stated, `artifact` for something derived and
    /// regenerable.
    pub kind: String,
}

impl Default for Incoming {
    fn default() -> Self {
        Self {
            author: "human".to_string(),
            creator: "agent:unknown".to_string(),
            sources: Vec::new(),
            kind: KIND_FACT.to_string(),
        }
    }
}

impl Incoming {
    /// A batch stated by `author` and written by `creator`, derived from nothing.
    #[must_use]
    pub fn new(author: impl Into<String>, creator: impl Into<String>) -> Self {
        Self {
            author: author.into(),
            creator: creator.into(),
            ..Self::default()
        }
    }

    /// Derive this batch from `sources`.
    #[must_use]
    pub fn from_sources(mut self, sources: Vec<String>) -> Self {
        self.sources = sources;
        self
    }

    /// Make this batch artifacts rather than stated facts.
    #[must_use]
    pub fn as_artifact(mut self) -> Self {
        self.kind = KIND_ARTIFACT.to_string();
        self
    }
}

/// A node somebody stated.
pub const KIND_FACT: &str = "fact";

/// A node something else was built from — a plan, a summary, a conclusion.
pub const KIND_ARTIFACT: &str = "artifact";

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
    top_k: usize,
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
            top_k: env_usize("ADI_FACTS_TOP_K").unwrap_or(DEFAULT_TOP_K),
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

    /// Set how many nearest neighbours a fact is compared against. See [`DEFAULT_TOP_K`].
    #[must_use]
    pub fn with_top_k(mut self, top_k: usize) -> Self {
        self.top_k = top_k;
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

    /// How many nearest neighbours a fact is compared against.
    #[must_use]
    pub fn top_k(&self) -> usize {
        self.top_k
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
    pub fn add(&self, base: &BaseId, incoming: &Incoming, facts: Vec<String>) -> Result<Staging> {
        self.stage(base, incoming, None, facts)
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
        incoming: &Incoming,
        text: &str,
        note_id: Option<&str>,
    ) -> Result<Staging> {
        let facts = self
            .judge
            .extract(text)
            .map_err(|e| Error::Judge(e.to_string()))?;
        let note_id =
            note_id.map_or_else(|| format!("note_{:x}", db::now_ms()), ToString::to_string);
        self.stage(base, incoming, Some((note_id.as_str(), text)), facts)
    }

    fn stage(
        &self,
        base: &BaseId,
        incoming: &Incoming,
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
        db.stage(
            &tx,
            &incoming.author,
            &incoming.creator,
            note,
            &facts,
            &incoming.sources,
            &incoming.kind,
        )?;

        let (candidates, truncated) = self.candidates(&db, incoming, &facts)?;
        let pairs: Vec<(Side<'_>, Side<'_>)> = candidates
            .iter()
            .map(|c| (c.left_by.side(&c.left), c.right_by.side(&c.right)))
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
            .map(|(c, j)| {
                let (relation, why) = correct_duplicate(c, j);
                db::Candidate {
                    new_seq: c.new_seq,
                    base_id: c.base_id.clone(),
                    base_seq: c.base_seq,
                    strength: c.strength,
                    kind: relation.as_str().to_string(),
                    why,
                }
            })
            .collect();
        db.record_pending(&tx, &rows)?;

        let mut staging = staging_of(&db, &tx)?;
        staging.truncated = truncated;
        staging.judge_error = judge_error;
        Ok(staging)
    }

    /// The **top K nearest neighbours** of each staged fact, symmetrically.
    ///
    /// A pair surfaces when *either* side holds the other in its own top K. The symmetry is
    /// load-bearing rather than tidy: a fact in a sparse neighbourhood keeps a busy fact in its
    /// top K while the busy one, surrounded by closer things, does not reciprocate. Selecting on
    /// one direction only would lose exactly those pairs, and the measured 108-of-124 at K=20
    /// assumes both.
    ///
    /// Siblings count as neighbours too. A batch of fifty facts can contradict itself, and a pair
    /// whose two halves are both incoming is the case the prototype got wrong twice.
    ///
    /// **Nothing is discarded for scoring low.** There is no floor; a pair is selected because it
    /// is among the closest, never because it cleared a number. The scores travel with the pairs
    /// because they inform a reader, not because they gate anything.
    fn candidates(
        &self,
        db: &Db,
        incoming: &Incoming,
        facts: &[String],
    ) -> Result<(Vec<Pair>, Option<Truncation>)> {
        if facts.is_empty() {
            return Ok((Vec::new(), None));
        }
        let embedder = self.embedder.get()?;
        let model = embedder.model_name().to_string();

        let nodes = db.nodes()?;
        let mut base_vectors: Vec<(Fact, Vec<f32>)> = Vec::with_capacity(nodes.len());
        for node in nodes {
            let vector = vector_of(db, embedder.as_ref(), &model, &node.id, &node.fact)?;
            base_vectors.push((node, vector));
        }

        let texts: Vec<&str> = facts.iter().map(String::as_str).collect();
        let staged_vectors = embedder
            .embed(&texts)
            .map_err(|e| Error::Embed(e.to_string()))?;

        let k = self.top_k.max(1);
        let cos = adi_knowledge::backend::cosine;
        // A pair is identified by the staged fact and whatever sits on the other side, so the two
        // directions can propose the same pair and it is still one row.
        let mut chosen: std::collections::BTreeSet<(usize, Other)> =
            std::collections::BTreeSet::new();

        // Direction one: what each staged fact holds closest.
        for i in 0..facts.len() {
            let mut ranked: Vec<(f32, Other)> =
                Vec::with_capacity(base_vectors.len() + facts.len());
            for (b, (_, vector)) in base_vectors.iter().enumerate() {
                ranked.push((cos(&staged_vectors[i], vector), Other::Base(b)));
            }
            for j in 0..facts.len() {
                if j != i {
                    ranked.push((
                        cos(&staged_vectors[i], &staged_vectors[j]),
                        Other::Staged(j),
                    ));
                }
            }
            for (_, other) in take_top(ranked, k) {
                chosen.insert(normalize(i, other));
            }
        }

        // Direction two: which staged facts a *base* node holds closest. This needs each base
        // node ranked against the whole base as well as against the batch, which is the one
        // quadratic step in an insert. It is nothing at the sizes this base runs at (97 facts is
        // ~9k dot products) and is the first thing to cache — a per-node K-th similarity,
        // invalidated exactly like a vector — if a base ever reaches tens of thousands.
        for (b, (_, bvec)) in base_vectors.iter().enumerate() {
            let mut ranked: Vec<(f32, Other)> =
                Vec::with_capacity(base_vectors.len() + facts.len());
            for (c, (_, cvec)) in base_vectors.iter().enumerate() {
                if c != b {
                    // A base-to-base pair was ruled on when the later of the two was inserted;
                    // it is here only to fill this node's top K, never to be surfaced again.
                    ranked.push((cos(bvec, cvec), Other::Base(c)));
                }
            }
            for (i, svec) in staged_vectors.iter().enumerate() {
                ranked.push((cos(bvec, svec), Other::Staged(i)));
            }
            for (_, other) in take_top(ranked, k) {
                if let Other::Staged(i) = other {
                    chosen.insert(normalize(i, Other::Base(b)));
                }
            }
        }

        // Every staged fact carries the batch's own provenance; a base fact carries its own. The
        // classifier is shown both, so it can tell a person's statement from a conclusion drawn
        // from it — two sentences that can be near-identical in wording and are never the same
        // record.
        let mine = Provenance {
            author: incoming.author.clone(),
            creator: incoming.creator.clone(),
            kind: incoming.kind.clone(),
        };
        let mut out: Vec<Pair> = chosen
            .into_iter()
            .map(|(i, other)| match other {
                Other::Base(b) => {
                    let (node, vector) = &base_vectors[b];
                    Pair {
                        new_seq: i as i64,
                        base_id: Some(node.id.clone()),
                        base_seq: None,
                        strength: cos(&staged_vectors[i], vector),
                        left: facts[i].clone(),
                        left_by: mine.clone(),
                        right: node.fact.clone(),
                        right_by: Provenance {
                            author: node.author.clone(),
                            creator: node.creator.clone(),
                            kind: node.kind.clone(),
                        },
                    }
                }
                Other::Staged(j) => Pair {
                    new_seq: i as i64,
                    base_id: None,
                    base_seq: Some(j as i64),
                    strength: cos(&staged_vectors[i], &staged_vectors[j]),
                    left: facts[i].clone(),
                    left_by: mine.clone(),
                    right: facts[j].clone(),
                    right_by: mine.clone(),
                },
            })
            .collect();
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
        let row = db.pair(tx, pair)?.ok_or_else(|| Error::NoSuchPair {
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

    // ---------------------------------------------------------------- reading

    /// The facts closest to one fact, strongest first — the queue around it, for a verifier
    /// agent to work.
    ///
    /// Top-N and nothing else, exactly as [`search`](Self::search) is. This used to apply a
    /// similarity floor and could therefore answer "nothing close" about a base that plainly held
    /// something closest; a caller cannot act on that, and it was never a real distinction.
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
        for node in db.nodes()? {
            let vector = vector_of(&db, embedder.as_ref(), &model, &node.id, &node.fact)?;
            vectors.push((node.id, node.fact, vector));
        }
        let mine = vectors
            .iter()
            .find(|(node, _, _)| node == &me.id)
            .map(|(_, _, v)| v.clone())
            .unwrap_or_default();
        let mut out = db::rank(&me.id, &mine, &vectors);
        out.truncate(top);
        Ok(out)
    }

    /// Every fact in the base, most recently changed first.
    ///
    /// # Errors
    /// [`Error::NoSuchBase`], [`Error::Denied`], or a backend error.
    pub fn list(&self, base: &BaseId, limit: usize) -> Result<Vec<Fact>> {
        self.reader.require_read(base)?;
        self.read_db(base)?.list(limit)
    }

    /// The facts closest in meaning to a query, best first.
    ///
    /// Every fact is ranked, the top `top` come back, and the scores travel with them: a weak
    /// match shown with its score is an honest answer, and the caller can judge it. Nothing is
    /// cut for scoring low — an answer of "nothing found" about a base that holds something
    /// closest is not one a caller can act on.
    ///
    /// **This is the command to run before [`add`](Self::add), not after.** A caller that asks
    /// what the base already knows will not stage a fact it already holds — which is the review
    /// queue reduced at the source instead of worked through afterwards.
    ///
    /// # Errors
    /// [`Error::NoSuchBase`], [`Error::Embed`], [`Error::Denied`], or a backend error.
    pub fn search(&self, base: &BaseId, query: &str, top: usize) -> Result<Vec<Neighbour>> {
        self.reader.require_read(base)?;
        let db = self.read_db(base)?;
        let embedder = self.embedder.get()?;
        let model = embedder.model_name().to_string();
        let wanted = embed_one(embedder.as_ref(), query)?;

        let mut vectors = Vec::new();
        for node in db.nodes()? {
            let vector = vector_of(&db, embedder.as_ref(), &model, &node.id, &node.fact)?;
            vectors.push((node.id, node.fact, vector));
        }
        // The empty id excludes nothing: the query is not a node, so it cannot be its own result.
        let mut out = db::rank("", &wanted, &vectors);
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

/// Refuse a `duplicate` verdict across two different kinds of record.
///
/// A statement and a conclusion drawn from it can be near-identical in wording — 0.954 on a real
/// pair — and calling that a duplicate invites a `merge` that deletes what a person actually
/// said. The classifier is *told* this, and the telling measurably helps, but it is not reliable:
/// the same pair came back `narrows` alone and `duplicate` when a second pair shared the batch.
/// A prompt cannot be depended on for a rule that is decidable from data we already hold, so this
/// decides it here and the prompt is left in as the belt to this brace.
///
/// It downgrades a label; it never drops a pair. Both `duplicate` and `narrows` reach the
/// reviewer, so nothing is hidden — what changes is the hint they are given, and that hint is
/// what sent a real agent toward the wrong verdict.
fn correct_duplicate(pair: &Pair, judged: &Judgement) -> (Relation, String) {
    if judged.relation != Relation::Duplicate || pair.left_by.kind == pair.right_by.kind {
        return (judged.relation, judged.why.clone());
    }
    (
        Relation::Narrows,
        format!(
            "a {} and a {} are different records, not one said twice",
            pair.left_by.kind, pair.right_by.kind
        ),
    )
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

/// The far side of a candidate pair: a node already in the base, or a sibling in this batch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Other {
    /// An index into the base vectors.
    Base(usize),
    /// An index into the staged facts.
    Staged(usize),
}

/// Put a staged-to-staged pair in one canonical order, so the two directions that can propose it
/// agree on which half is `new_seq`. Anything against the base is already one-directional.
fn normalize(i: usize, other: Other) -> (usize, Other) {
    match other {
        Other::Staged(j) if j < i => (j, Other::Staged(i)),
        _ => (i, other),
    }
}

/// The `k` highest-scoring entries, best first.
///
/// `select_nth_unstable_by` rather than a full sort: only the top `k` are wanted, and a base of
/// any size is ranked once per staged fact and once per node.
fn take_top(mut ranked: Vec<(f32, Other)>, k: usize) -> Vec<(f32, Other)> {
    let order = |a: &(f32, Other), b: &(f32, Other)| {
        b.0.partial_cmp(&a.0)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.1.cmp(&b.1))
    };
    if ranked.len() > k {
        ranked.select_nth_unstable_by(k, order);
        ranked.truncate(k);
    }
    ranked.sort_by(order);
    ranked
}

/// One candidate pair before the classifier has seen it.
#[derive(Debug, Clone)]
struct Pair {
    new_seq: i64,
    base_id: Option<String>,
    base_seq: Option<i64>,
    strength: f32,
    left: String,
    left_by: Provenance,
    right: String,
    right_by: Provenance,
}

/// Who a sentence belongs to, owned — a [`Side`] borrows from this and from the text beside it.
#[derive(Debug, Clone)]
struct Provenance {
    author: String,
    creator: String,
    kind: String,
}

impl Provenance {
    fn side<'a>(&'a self, fact: &'a str) -> Side<'a> {
        Side {
            fact,
            author: &self.author,
            creator: &self.creator,
            kind: &self.kind,
        }
    }
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

fn env_usize(key: &str) -> Option<usize> {
    std::env::var(key).ok()?.trim().parse().ok()
}
