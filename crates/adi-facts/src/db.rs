//! One SQLite file per base, holding the whole of `graph.sql`.
//!
//! Shaped after `adi-knowledge`'s sqlite backend — the same pragmas, the same
//! `Mutex<Connection>`, the same "schema is a const applied on open" — but deliberately **not**
//! behind a provider trait the way that crate's storage is. There, the trait earns its keep
//! because a base is a bag of notes and a hosted vector store could hold one just as well. Here
//! the schema *is* the design: the version counter, the edge stamp, and a recursive CTE that
//! answers transitive staleness in one query are what the whole thing is for, and a trait over
//! them would be thirty methods with one implementation and no second one in sight.
//!
//! Every SQL statement in this crate lives in this file, so the shape of the data has one place
//! to be read.

use std::path::Path;
use std::sync::Mutex;

use rusqlite::{Connection, OptionalExtension, Transaction, params};

use crate::error::{Error, Result};
use crate::model::{Committed, Event, Fact, Neighbour, Pending, Stale, Truncation, Verdict};

/// The file a base's facts live in, inside the base directory.
pub(crate) const DB_FILE: &str = "facts.db";

/// The same settings the rest of the platform's SQLite uses, for the same reason: agents, their
/// tools, and a person at the terminal reach one base at once. `busy_timeout` is set first
/// deliberately — setting `journal_mode` takes a lock, and on a connection with no timeout yet
/// that fails outright when another process is mid-write.
const PRAGMAS: &str = "\
    pragma busy_timeout = 5000;\n\
    pragma journal_mode = WAL;\n\
    pragma foreign_keys = ON;\n\
    pragma synchronous = NORMAL;\n";

const SCHEMA: &str = include_str!("graph.sql");

/// A base's facts, its graph, and its staged transactions.
#[derive(Debug)]
pub(crate) struct Db {
    conn: Mutex<Connection>,
}

/// A candidate pair, before anybody has looked at it.
#[derive(Debug, Clone)]
pub(crate) struct Candidate {
    pub new_seq: i64,
    pub base_id: Option<String>,
    pub base_seq: Option<i64>,
    pub strength: f32,
    pub kind: String,
    pub why: String,
}

impl Db {
    /// Open (creating and migrating if needed) the base at `path`.
    pub(crate) fn open(path: &Path) -> Result<Self> {
        let conn = Connection::open(path)?;
        conn.execute_batch(PRAGMAS)?;
        conn.execute_batch(SCHEMA)?;
        migrate(&conn)?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    fn conn(&self) -> Result<std::sync::MutexGuard<'_, Connection>> {
        self.conn
            .lock()
            .map_err(|e| Error::Backend(format!("fact base lock poisoned: {e}")))
    }

    // ------------------------------------------------------------------ nodes

    /// Every node, with the provenance a classifier needs to read it.
    ///
    /// Every kind, not only `fact`. A derived artifact is a node, so a new fact is checked
    /// against it too; that is how "we can support China" surfaced against a plan that said
    /// "skip China for now" (`CLI.md`).
    ///
    /// Read through `facts_v` rather than `nodes`, so the author and creator come back as names
    /// rather than as the integers they are interned to.
    pub(crate) fn nodes(&self) -> Result<Vec<Fact>> {
        let conn = self.conn()?;
        let mut stmt = conn.prepare(
            "select id, fact, author, creator, version, updated_at, kind from facts_v order by id",
        )?;
        let rows = stmt.query_map([], row_to_fact)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Every fact in the base, most recently touched first.
    ///
    /// What `facts list` prints. Separate from [`nodes`](Self::nodes) because it is a person's
    /// question rather than the pair scan's: newest first is the order somebody reading wants,
    /// and a cap keeps a large base from filling a terminal.
    pub(crate) fn list(&self, limit: usize) -> Result<Vec<Fact>> {
        let conn = self.conn()?;
        let mut stmt = conn.prepare(
            "select id, fact, author, creator, version, updated_at, kind from facts_v
             order by updated_at desc, id limit ?1",
        )?;
        let rows = stmt.query_map(params![i64::try_from(limit).unwrap_or(i64::MAX)], row_to_fact)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// One fact as it stands, with the identities spelled back out.
    pub(crate) fn fact(&self, id: &str) -> Result<Option<Fact>> {
        let conn = self.conn()?;
        Ok(conn
            .prepare("select id, fact, author, creator, version, updated_at, kind from facts_v where id = ?1")?
            .query_row(params![id], row_to_fact)
            .optional()?)
    }

    // ------------------------------------------------------------------ graph

    /// Everything out of date, and what changed under it. Shallowest first.
    pub(crate) fn stale(&self) -> Result<Vec<Stale>> {
        let conn = self.conn()?;
        let mut stmt = conn.prepare(
            "select s.id, n.fact, s.root_cause, s.depth
             from stale s join nodes n on n.id = s.id
             order by s.depth, s.id",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(Stale {
                id: r.get(0)?,
                fact: r.get(1)?,
                root_cause: r.get(2)?,
                depth: r.get(3)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// A derived node was regenerated: bring its incoming edges up to its sources' versions,
    /// then bump its own so anything built on *it* goes stale in turn.
    pub(crate) fn refresh(&self, id: &str) -> Result<()> {
        let mut conn = self.conn()?;
        let tx = conn.transaction()?;
        if node_version(&tx, id)?.is_none() {
            return Err(Error::NoSuchFact(id.to_string()));
        }
        let sources: Vec<String> = {
            let mut stmt = tx.prepare("select src from edges where dst = ?1")?;
            let rows = stmt.query_map(params![id], |r| r.get(0))?;
            rows.collect::<rusqlite::Result<Vec<_>>>()?
        };
        for source in &sources {
            let version = node_version(&tx, source)?.unwrap_or(1);
            tx.execute(
                "update edges set src_version = ?1 where src = ?2 and dst = ?3",
                params![version, source, id],
            )?;
        }
        bump(&tx, id)?;
        tx.commit()?;
        Ok(())
    }

    // ---------------------------------------------------------------- vectors

    /// The cached vector for a node, if **this** model made it.
    ///
    /// A row from another model is treated as absent rather than returned. Facts are embedded by
    /// `nomic-embed-text` and the code index by a candle model; comparing across two vector
    /// spaces produces plausible-looking rankings and no error anywhere.
    ///
    /// `dims` is checked against the blob it was stored with rather than against the embedder's
    /// declared width — the model's name already fixes the space, and validating a row against
    /// itself catches a truncated blob without making the check depend on an
    /// [`Embedder::dimensions`](adi_indexer::embed::Embedder::dimensions) that a swapped model
    /// would make wrong.
    pub(crate) fn cached_vector(&self, id: &str, model: &str) -> Result<Option<Vec<f32>>> {
        let conn = self.conn()?;
        let row: Option<(String, i64, Vec<u8>)> = conn
            .prepare("select model, dims, vec from vectors where id = ?1")?
            .query_row(params![id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .optional()?;
        Ok(row
            .filter(|(m, _, _)| m == model)
            .map(|(_, dims, blob)| (dims, adi_knowledge::backend::decode_vector(&blob)))
            .filter(|(dims, vector)| usize::try_from(*dims).unwrap_or(0) == vector.len())
            .map(|(_, vector)| vector))
    }

    pub(crate) fn store_vector(&self, id: &str, model: &str, vector: &[f32]) -> Result<()> {
        let conn = self.conn()?;
        conn.execute(
            "insert or replace into vectors (id, model, dims, vec) values (?1, ?2, ?3, ?4)",
            params![
                id,
                model,
                i64::try_from(vector.len()).unwrap_or(0),
                adi_knowledge::backend::encode_vector(vector)
            ],
        )?;
        Ok(())
    }

    // ------------------------------------------------------------ transactions

    /// Stage a batch of facts under a fresh transaction: the note they came from when there was
    /// one, what they were derived from, and what kind of node they become.
    ///
    /// `sources` are checked here so a typo costs nothing — the caller finds out before the
    /// batch is embedded and classified — and checked **again** at commit, because the base can
    /// move while a transaction is open.
    #[allow(
        clippy::too_many_arguments,
        reason = "one argument per thing a batch carries into the base — who, what, from where, \
                  as what. A struct here would be `Incoming` again, one layer down."
    )]
    pub(crate) fn stage(
        &self,
        tx_id: &str,
        author: &str,
        creator: &str,
        note: Option<(&str, &str)>,
        facts: &[String],
        sources: &[String],
        kind: &str,
    ) -> Result<()> {
        let mut conn = self.conn()?;
        let tx = conn.transaction()?;
        for source in sources {
            resolve_source(&tx, source, facts.len(), None)?;
        }
        let (author_id, creator_id) = (actor(&tx, author)?, actor(&tx, creator)?);
        let now = now_ms();
        if let Some((note_id, text)) = note {
            tx.execute(
                "insert or replace into notes (id, text, author, created_at) values (?1, ?2, ?3, ?4)",
                params![note_id, text, author_id, now],
            )?;
        }
        tx.execute(
            "insert into transactions (id, state, author, creator, note_id, kind, created_at)
             values (?1, 'needs_review', ?2, ?3, ?4, ?5, ?6)",
            params![tx_id, author_id, creator_id, note.map(|(id, _)| id), kind, now],
        )?;
        for (seq, fact) in facts.iter().enumerate() {
            tx.execute(
                "insert into staged (tx, seq, fact) values (?1, ?2, ?3)",
                params![tx_id, i64::try_from(seq).unwrap_or(0), fact],
            )?;
        }
        for source in sources {
            tx.execute(
                "insert or ignore into tx_sources (tx, src) values (?1, ?2)",
                params![tx_id, source],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Record the pairs a reviewer has to rule on, and settle the transaction's state.
    pub(crate) fn record_pending(&self, tx_id: &str, candidates: &[Candidate]) -> Result<()> {
        let mut conn = self.conn()?;
        let tx = conn.transaction()?;
        for (pair, c) in candidates.iter().enumerate() {
            tx.execute(
                "insert into pending (tx, pair, new_seq, base_id, base_seq, strength, kind, why)
                 values (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![
                    tx_id,
                    i64::try_from(pair).unwrap_or(0),
                    c.new_seq,
                    c.base_id,
                    c.base_seq,
                    f64::from(c.strength),
                    c.kind,
                    c.why
                ],
            )?;
        }
        let state = if candidates.is_empty() {
            "ready"
        } else {
            "needs_review"
        };
        tx.execute(
            "update transactions set state = ?1 where id = ?2",
            params![state, tx_id],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// A transaction's state, or `None` if there is no such transaction.
    pub(crate) fn state(&self, tx_id: &str) -> Result<Option<String>> {
        let conn = self.conn()?;
        Ok(conn
            .prepare("select state from transactions where id = ?1")?
            .query_row(params![tx_id], |r| r.get(0))
            .optional()?)
    }

    /// The staged facts still due to land, in order.
    pub(crate) fn staged(&self, tx_id: &str) -> Result<Vec<String>> {
        let conn = self.conn()?;
        let mut stmt =
            conn.prepare("select fact from staged where tx = ?1 and dropped = 0 order by seq")?;
        let rows = stmt.query_map(params![tx_id], |r| r.get(0))?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Every pair in a transaction, strongest first, with both sentences resolved.
    pub(crate) fn pending(&self, tx_id: &str) -> Result<Vec<Pending>> {
        let conn = self.conn()?;
        let mut stmt = conn.prepare(
            "select p.pair, p.new_seq, p.base_id, p.base_seq, p.strength, p.kind, p.why,
                    p.verdict, p.keep, a.name,
                    mine.fact,
                    coalesce(node.fact, theirs.fact, '')
             from pending p
             left join actors a    on a.id = p.confirmer
             join staged mine      on mine.tx = p.tx and mine.seq = p.new_seq
             left join nodes node  on node.id = p.base_id
             left join staged theirs on theirs.tx = p.tx and theirs.seq = p.base_seq
             where p.tx = ?1
             order by p.strength desc, p.pair",
        )?;
        let rows = stmt.query_map(params![tx_id], |r| {
            Ok(Pending {
                pair: r.get(0)?,
                new_seq: r.get(1)?,
                base_id: r.get(2)?,
                base_seq: r.get(3)?,
                strength: r.get::<_, f64>(4)? as f32,
                kind: r.get(5)?,
                why: r.get(6)?,
                verdict: r.get(7)?,
                keep: r.get(8)?,
                confirmer: r.get(9)?,
                new_text: r.get(10)?,
                other_text: r.get(11)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// One pair by number.
    pub(crate) fn pair(&self, tx_id: &str, pair: i64) -> Result<Option<Pending>> {
        Ok(self.pending(tx_id)?.into_iter().find(|p| p.pair == pair))
    }

    /// Write a verdict, and apply what it does to the staged batch.
    ///
    /// **`merge` and `supersede` are one mechanism**: the losing fact is replaced, never left
    /// beside the winner. Which row that is depends on whether the other side is already in the
    /// base or is another fact in this same batch — and getting that wrong is where two of the
    /// prototype's three silent bugs lived. `merge` used to rewrite only the incoming fact and
    /// leave the base one alive, and when both sides were staged neither `merge` nor `supersede`
    /// retired anything at all, so both landed.
    pub(crate) fn resolve(
        &self,
        tx_id: &str,
        pair: &Pending,
        verdict: Verdict,
        keep: Option<&str>,
        fact: Option<&str>,
        confirmer: &str,
    ) -> Result<usize> {
        let mut conn = self.conn()?;
        let tx = conn.transaction()?;
        let confirmer_id = actor(&tx, confirmer)?;
        tx.execute(
            "update pending set verdict = ?1, keep = ?2, confirmer = ?3, resolved_at = ?4
             where tx = ?5 and pair = ?6",
            params![
                verdict.as_str(),
                keep,
                confirmer_id,
                now_ms(),
                tx_id,
                pair.pair
            ],
        )?;

        let drop = |seq: i64| -> Result<()> {
            tx.execute(
                "update staged set dropped = 1 where tx = ?1 and seq = ?2",
                params![tx_id, seq],
            )?;
            Ok(())
        };
        match verdict {
            Verdict::Coexist => {}
            Verdict::Drop => drop(pair.new_seq)?,
            Verdict::Merge => {
                tx.execute(
                    "update staged set fact = ?1 where tx = ?2 and seq = ?3",
                    params![fact.unwrap_or_default(), tx_id, pair.new_seq],
                )?;
                if let Some(seq) = pair.base_seq {
                    drop(seq)?;
                }
            }
            Verdict::Supersede => {
                if keep == Some(pair.theirs().as_str()) {
                    drop(pair.new_seq)?;
                } else if let Some(seq) = pair.base_seq {
                    drop(seq)?;
                }
            }
        }

        let left: i64 = tx.query_row(
            "select count(*) from pending where tx = ?1 and verdict is null",
            params![tx_id],
            |r| r.get(0),
        )?;
        if left == 0 {
            tx.execute(
                "update transactions set state = 'ready' where id = ?1",
                params![tx_id],
            )?;
        }
        tx.commit()?;
        Ok(usize::try_from(left).unwrap_or(0))
    }

    /// Land a transaction. Refuses while any pair is open, and says which.
    #[allow(
        clippy::too_many_lines,
        reason = "one transaction end to end: rewrite the losers, insert the survivors, log both. \
                  Splitting it would mean threading the same open `Transaction` and half a dozen \
                  intermediate lists through helpers that mean nothing apart."
    )]
    pub(crate) fn commit(&self, tx_id: &str) -> Result<Committed> {
        let mut conn = self.conn()?;
        let tx = conn.transaction()?;

        let open: Vec<i64> = {
            let mut stmt = tx.prepare(
                "select pair from pending where tx = ?1 and verdict is null order by strength desc",
            )?;
            let rows = stmt.query_map(params![tx_id], |r| r.get(0))?;
            rows.collect::<rusqlite::Result<Vec<_>>>()?
        };
        if !open.is_empty() {
            return Err(Error::StillOpen {
                count: open.len(),
                pairs: open
                    .iter()
                    .map(|p| format!("p{p}"))
                    .collect::<Vec<_>>()
                    .join(", "),
            });
        }

        let (author, creator, note_id, kind): (i64, i64, Option<String>, String) = tx.query_row(
            "select author, creator, note_id, kind from transactions where id = ?1",
            params![tx_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )?;
        let sources: Vec<String> = {
            let mut stmt = tx.prepare("select src from tx_sources where tx = ?1 order by src")?;
            let rows = stmt.query_map(params![tx_id], |r| r.get(0))?;
            rows.collect::<rusqlite::Result<Vec<_>>>()?
        };

        // A base fact is rewritten in place when the incoming one wins: always for `merge`
        // (which supplies the sentence), and for `supersede` only when the winner was the
        // incoming side. Never a second row — an extra row is what would leave the base holding
        // both the merged sentence and the original it was meant to replace.
        let rewrites: Vec<(i64, String, String, Option<i64>)> = {
            let mut stmt = tx.prepare(
                "select new_seq, base_id, verdict, confirmer from pending
                 where tx = ?1 and base_id is not null and verdict in ('merge', 'supersede')",
            )?;
            let rows = stmt.query_map(params![tx_id], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, Option<i64>>(3)?,
                ))
            })?;
            let mut keeps = Vec::new();
            for row in rows {
                let (new_seq, base_id, verdict, confirmer) = row?;
                let keep: Option<String> = tx.query_row(
                    "select keep from pending where tx = ?1 and new_seq = ?2 and base_id = ?3",
                    params![tx_id, new_seq, base_id],
                    |r| r.get(0),
                )?;
                if verdict == "merge" || keep.as_deref() == Some(&format!("#{new_seq}")) {
                    keeps.push((new_seq, base_id, verdict, confirmer));
                }
            }
            keeps
        };

        let mut rewritten = Vec::new();
        for (seq, base_id, verdict, confirmer) in rewrites {
            let winner: String = tx.query_row(
                "select fact from staged where tx = ?1 and seq = ?2",
                params![tx_id, seq],
                |r| r.get(0),
            )?;
            let (was, version): (String, i64) = tx.query_row(
                "select fact, version from nodes where id = ?1",
                params![base_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )?;
            tx.execute(
                "update nodes set fact = ?1, version = version + 1, updated_at = ?2 where id = ?3",
                params![winner, now_ms(), base_id],
            )?;
            // The cached vector describes the sentence that just stopped being this node's
            // sentence. Left in place it would rank the node by what it used to say, silently.
            tx.execute("delete from vectors where id = ?1", params![base_id])?;
            tx.execute(
                "update staged set dropped = 1 where tx = ?1 and seq = ?2",
                params![tx_id, seq],
            )?;
            log(
                &tx,
                &base_id,
                version + 1,
                &verdict,
                &was,
                &winner,
                "",
                confirmer,
                tx_id,
            )?;
            rewritten.push(base_id);
        }

        // A pair whose two halves are both in this batch: the loser never reaches the base, so it
        // never gets an id anybody could reference. It is logged against the survivor as
        // `absorbed`, with its text, so the decision stays findable by the wording somebody
        // remembers.
        let absorbed: Vec<(i64, String, String, Option<i64>)> = {
            let mut stmt = tx.prepare(
                "select new_seq, base_seq, verdict, keep, confirmer from pending
                 where tx = ?1 and base_id is null and verdict in ('merge', 'supersede')",
            )?;
            let rows = stmt.query_map(params![tx_id], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, i64>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, Option<String>>(3)?,
                    r.get::<_, Option<i64>>(4)?,
                ))
            })?;
            let mut out = Vec::new();
            for row in rows {
                let (new_seq, base_seq, verdict, keep, confirmer) = row?;
                let loser = if verdict == "merge" || keep.as_deref() == Some(&format!("#{new_seq}"))
                {
                    base_seq
                } else {
                    new_seq
                };
                let winner = if loser == base_seq { new_seq } else { base_seq };
                let text: String = tx.query_row(
                    "select fact from staged where tx = ?1 and seq = ?2",
                    params![tx_id, loser],
                    |r| r.get(0),
                )?;
                out.push((winner, text, verdict, confirmer));
            }
            out
        };

        let rows: Vec<(i64, String)> = {
            let mut stmt =
                tx.prepare("select seq, fact from staged where tx = ?1 and dropped = 0 order by seq")?;
            let rows = stmt.query_map(params![tx_id], |r| Ok((r.get(0)?, r.get(1)?)))?;
            rows.collect::<rusqlite::Result<Vec<_>>>()?
        };

        // Nodes first, edges second, in two passes. A `#N` source names a fact staged in this
        // same batch, so its id does not exist until every row has been inserted.
        let mut added: Vec<(String, String)> = Vec::new();
        let mut ids_by_seq: Vec<(i64, String)> = Vec::new();
        for (seq, fact) in rows {
            let id = fresh_id(&tx, seq, &kind)?;
            let now = now_ms();
            tx.execute(
                "insert into nodes (id, fact, author, creator, version, updated_at, kind)
                 values (?1, ?2, ?3, ?4, 1, ?5, ?6)",
                params![id, fact, author, creator, now, &kind],
            )?;
            log(
                &tx,
                &id,
                1,
                "created",
                "",
                &fact,
                &sources.join(", "),
                Some(creator),
                tx_id,
            )?;
            for (winner, lost, verdict, confirmer) in &absorbed {
                if *winner == seq {
                    log(&tx, &id, 1, "absorbed", lost, &fact, verdict, *confirmer, tx_id)?;
                }
            }
            ids_by_seq.push((seq, id.clone()));
            added.push((id, fact));
        }

        // Every edge is stamped with its source's version **as of now**, not as of staging. A
        // source that moved while the transaction was open would otherwise be recorded at the
        // version it had when the caller started typing, and the derived node would be born
        // claiming to be current when it never saw that text.
        let now = now_ms();
        if let Some(note_id) = &note_id {
            // The note is a node so that editing it makes every fact drawn from it stale — the
            // same mechanism as `--from`, with no special case for "came from prose".
            tx.execute(
                "insert or ignore into nodes (id, fact, author, creator, version, updated_at, kind)
                 values (?1, '(source note)', ?2, ?3, 1, ?4, 'note')",
                params![note_id, author, creator, now],
            )?;
        }
        let mut linked = 0usize;
        for (_, dst) in &ids_by_seq {
            let mut srcs: Vec<String> = Vec::new();
            if let Some(note_id) = &note_id {
                srcs.push(note_id.clone());
            }
            for source in &sources {
                srcs.push(resolve_source(&tx, source, 0, Some(&ids_by_seq))?);
            }
            for src in srcs {
                // A node never derives from itself. `--from #0` on a batch that includes #0 is
                // the readable way to say "everything else here follows from that one", so the
                // self-edge is skipped rather than made an error.
                if &src == dst {
                    continue;
                }
                let version = node_version(&tx, &src)?
                    .ok_or_else(|| Error::NoSuchFact(src.clone()))?;
                tx.execute(
                    "insert or replace into edges (src, dst, src_version, created_at)
                     values (?1, ?2, ?3, ?4)",
                    params![src, dst, version, now],
                )?;
                linked += 1;
            }
        }

        let dropped: i64 = tx.query_row(
            "select count(*) from staged where tx = ?1 and dropped = 1",
            params![tx_id],
            |r| r.get(0),
        )?;
        tx.execute(
            "update transactions set state = 'committed' where id = ?1",
            params![tx_id],
        )?;
        tx.commit()?;
        Ok(Committed {
            added,
            linked,
            rewritten,
            dropped: usize::try_from(dropped).unwrap_or(0),
        })
    }

    pub(crate) fn abort(&self, tx_id: &str) -> Result<()> {
        let conn = self.conn()?;
        conn.execute(
            "update transactions set state = 'aborted' where id = ?1",
            params![tx_id],
        )?;
        Ok(())
    }

    // ---------------------------------------------------------------- history

    /// A fact's decision log, oldest first.
    pub(crate) fn history(&self, id: &str) -> Result<Vec<Event>> {
        let conn = self.conn()?;
        let mut stmt = conn.prepare(
            "select h.version, h.event, h.was, h.now, coalesce(a.name, ''), h.at
             from history h left join actors a on a.id = h.confirmer
             where h.id = ?1 order by h.seq",
        )?;
        let rows = stmt.query_map(params![id], |r| {
            Ok(Event {
                version: r.get(0)?,
                event: r.get(1)?,
                was: r.get(2)?,
                now: r.get(3)?,
                confirmer: r.get(4)?,
                at: r.get(5)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }
}

/// Rank every other node against `vector`, best first.
///
/// Nothing is dropped for scoring low. Both callers take a top-N slice of the result, which is
/// what bounds it; a similarity cut-off here would mean an answer of "nothing" whenever the best
/// the base had to offer happened to sit below some number.
pub(crate) fn rank(
    mine: &str,
    vector: &[f32],
    others: &[(String, String, Vec<f32>)],
) -> Vec<Neighbour> {
    let mut out: Vec<Neighbour> = others
        .iter()
        .filter(|(id, _, _)| id != mine)
        .map(|(id, fact, v)| Neighbour {
            id: id.clone(),
            fact: fact.clone(),
            strength: adi_knowledge::backend::cosine(vector, v),
        })
        .collect();
    out.sort_by(|a, b| {
        b.strength
            .partial_cmp(&a.strength)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.id.cmp(&b.id))
    });
    out
}

/// Cap a candidate list, reporting what was cut and below what strength.
///
/// Truncation is always reported. A silent cap reads as "nothing else to see", which is the one
/// lie this interface must never tell.
pub(crate) fn cap<T>(mut candidates: Vec<T>, max: usize, strength: impl Fn(&T) -> f32) -> (Vec<T>, Option<Truncation>) {
    if candidates.len() <= max {
        return (candidates, None);
    }
    let dropped = candidates.len() - max;
    candidates.truncate(max);
    let below = candidates.last().map_or(0.0, &strength);
    (candidates, Some(Truncation { dropped, below }))
}

/// Intern an identity. Everything that writes a name goes through here, so `actors` is the only
/// place a name is spelled out.
fn actor(tx: &Transaction<'_>, name: &str) -> Result<i64> {
    let name = name.trim();
    if let Some(id) = tx
        .prepare("select id from actors where name = ?1")?
        .query_row(params![name], |r| r.get::<_, i64>(0))
        .optional()?
    {
        return Ok(id);
    }
    // A name carrying a colon or an `@` is an agent id — `agent:chat@1`. A bare word is a
    // person. Nothing else distinguishes the two at this layer, and guessing beats asking every
    // caller to declare it.
    let kind = if name.contains(':') || name.contains('@') {
        "agent"
    } else {
        "human"
    };
    tx.execute(
        "insert into actors (name, kind) values (?1, ?2)",
        params![name, kind],
    )?;
    Ok(tx.last_insert_rowid())
}

/// Turn one `--from` argument into the node id its edge will point at.
///
/// Two forms, and both can go wrong quietly if nobody checks:
///
/// * a **committed id**, which must exist — a typo would otherwise write no edge at all, and a
///   derived node with a missing edge is one that never goes stale. That is the failure this
///   whole design exists to prevent, arriving as silence;
/// * **`#N`**, a fact staged in this same batch. `staged` is `Some` at commit, when every
///   surviving row has an id; `None` at staging, when the only thing checkable is that `N` is in
///   range. A `#N` that was dropped by a verdict has no id to point at, and saying so is the
///   difference between a refused commit and a dangling edge.
fn resolve_source(
    tx: &Transaction<'_>,
    source: &str,
    staged_len: usize,
    staged: Option<&[(i64, String)]>,
) -> Result<String> {
    let Some(n) = source.strip_prefix('#') else {
        if node_version(tx, source)?.is_none() {
            return Err(Error::NoSuchFact(source.to_string()));
        }
        return Ok(source.to_string());
    };
    let seq: i64 = n
        .trim()
        .parse()
        .map_err(|_| Error::BadSource(source.to_string()))?;
    let Some(rows) = staged else {
        if seq < 0 || usize::try_from(seq).unwrap_or(usize::MAX) >= staged_len {
            return Err(Error::BadSource(source.to_string()));
        }
        return Ok(source.to_string());
    };
    rows.iter()
        .find(|(s, _)| *s == seq)
        .map(|(_, id)| id.clone())
        .ok_or_else(|| Error::SourceDropped(source.to_string()))
}

/// Add what `create table if not exists` cannot.
///
/// A base created before `--from` existed has a `transactions` table with no `kind` column, and
/// the schema script will not add one — every later `add` would fail with `no such column` and
/// no hint about why.
fn migrate(conn: &Connection) -> Result<()> {
    let has_kind = conn
        .prepare("select 1 from pragma_table_info('transactions') where name = 'kind'")?
        .exists([])?;
    if !has_kind {
        conn.execute(
            "alter table transactions add column kind text not null default 'fact'",
            [],
        )?;
    }
    Ok(())
}

fn node_version(tx: &Transaction<'_>, id: &str) -> Result<Option<i64>> {
    Ok(tx
        .prepare("select version from nodes where id = ?1")?
        .query_row(params![id], |r| r.get(0))
        .optional()?)
}

/// Bump a node's version — the one write the staleness graph reads.
fn bump(tx: &Transaction<'_>, id: &str) -> Result<()> {
    tx.execute(
        "update nodes set version = version + 1, updated_at = ?1 where id = ?2",
        params![now_ms(), id],
    )?;
    Ok(())
}

#[allow(clippy::too_many_arguments, reason = "one row of an append-only log, written in one place")]
fn log(
    tx: &Transaction<'_>,
    id: &str,
    version: i64,
    event: &str,
    was: &str,
    now: &str,
    other: &str,
    confirmer: Option<i64>,
    tx_id: &str,
) -> Result<()> {
    tx.execute(
        "insert into history (id, at, version, event, was, now, other, confirmer, tx)
         values (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![id, now_ms(), version, event, was, now, other, confirmer, tx_id],
    )?;
    Ok(())
}

/// A node id nothing else has taken.
///
/// `f_<ms>_<seq>` is the prototype's shape and reads well in a reference; an artifact gets `d_`
/// so a stale-report line says at a glance whether a person stated it or an agent derived it.
/// The retry is insurance the prototype did not have: two processes committing in the same
/// millisecond would otherwise collide on the primary key and fail the whole transaction.
fn fresh_id(tx: &Transaction<'_>, seq: i64, kind: &str) -> Result<String> {
    let prefix = if kind == "artifact" { "d" } else { "f" };
    let stem = format!("{prefix}_{:x}_{seq}", now_ms());
    for suffix in 0..1000 {
        let candidate = if suffix == 0 {
            stem.clone()
        } else {
            format!("{stem}_{suffix}")
        };
        if node_version(tx, &candidate)?.is_none() {
            return Ok(candidate);
        }
    }
    Err(Error::Backend(format!("cannot find a free id near {stem}")))
}

fn row_to_fact(row: &rusqlite::Row<'_>) -> rusqlite::Result<Fact> {
    Ok(Fact {
        id: row.get(0)?,
        fact: row.get(1)?,
        author: row.get(2)?,
        creator: row.get(3)?,
        version: row.get(4)?,
        updated_at: row.get(5)?,
        kind: row.get(6)?,
    })
}

/// Wall clock in milliseconds — for `updated_at` and for id stems, never for staleness.
pub(crate) fn now_ms() -> i64 {
    i64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis(),
    )
    .unwrap_or(0)
}
