//! The default provider: one SQLite file per base.
//!
//! Why its own file rather than a table in [`adi_db`](adi_config)'s shared store: a base is
//! addressed by *scope*, and the agent level has no equivalent there — `db/global.db` and
//! `db/projects/<id>.db` are the only two scopes that store knows. A file per base also makes
//! "delete this base" a directory removal instead of a cascade, and keeps one agent's memory
//! from sharing a write lock with every other agent's.
//!
//! ## Why the vector search is a scan
//!
//! The indexer keeps a usearch HNSW next to its SQLite file, because it ranks tens of thousands
//! of symbols. A knowledge base holds notes somebody wrote on purpose — hundreds, thousands at
//! the outside — and at that size an exact scan of stored f32 blobs beats an approximate index
//! on every axis that matters: it is exact (no recall cliff), it has no second file to keep in
//! step with the rows, and 10,000 chunks × 768 dimensions is ~30MB and a few milliseconds. When
//! a base outgrows that, the answer is a provider that speaks to something built for it — which
//! is what the [provider trait](super::Provider) is for.

use std::path::Path;
use std::sync::Arc;
use std::sync::Mutex;

use rusqlite::{Connection, OptionalExtension, params, params_from_iter};

use crate::error::{Error, Result};
use crate::note::{EmbeddingState, Knowledge};

use super::{
    Backend, BaseContext, ChunkHit, Provider, Query, SQLITE, best_per_note, bm25_score, cosine,
    decode_vector, encode_vector, fts_query,
};

/// The file a base's notes live in, inside the base directory.
const DB_FILE: &str = "knowledge.db";

/// The same settings [`adi_db`](adi_config)'s store applies, for the same reason: several
/// processes (agents, their tools, the control panel) read and write one base at once. WAL so
/// readers and the writer do not block each other, and a busy timeout so the remaining
/// writer-vs-writer contention is a short wait rather than an instant `SQLITE_BUSY`.
///
/// `busy_timeout` is first deliberately — setting `journal_mode` takes a lock, and on a
/// connection with no timeout yet that fails outright when another process is mid-write.
const PRAGMAS: &str = "\
    pragma busy_timeout = 5000;\n\
    pragma journal_mode = WAL;\n\
    pragma foreign_keys = ON;\n\
    pragma synchronous = NORMAL;\n";

const SCHEMA: &str = "\
    create table if not exists notes (
        id            text primary key,
        title         text not null,
        body          text not null,
        tags          text not null default '',
        source        text,
        content_hash  text not null,
        embed_model   text,
        embed_hash    text,
        chunks        integer not null default 0,
        dimensions    integer not null default 0,
        created_at    integer not null,
        updated_at    integer not null
    );

    create table if not exists vectors (
        note_id  text not null references notes(id) on delete cascade,
        chunk    integer not null,
        vector   blob not null,
        primary key (note_id, chunk)
    );

    create index if not exists notes_updated on notes(updated_at desc);

    create virtual table if not exists notes_fts using fts5(
        id unindexed, title, body, tags, tokenize = 'porter unicode61'
    );
";

/// Opens [`SqliteBackend`]s.
#[derive(Debug, Clone, Copy, Default)]
pub struct SqliteProvider;

impl Provider for SqliteProvider {
    fn name(&self) -> &str {
        SQLITE
    }

    fn description(&self) -> &str {
        "One SQLite file per base: FTS5 for words, stored f32 vectors for meaning."
    }

    fn open(&self, ctx: &BaseContext) -> Result<Arc<dyn Backend>> {
        std::fs::create_dir_all(&ctx.dir)?;
        Ok(Arc::new(SqliteBackend::open(&ctx.dir.join(DB_FILE))?))
    }
}

/// A base held in one SQLite file.
#[derive(Debug)]
pub struct SqliteBackend {
    conn: Mutex<Connection>,
}

impl SqliteBackend {
    /// Open (creating and migrating if needed) the base at `path`.
    ///
    /// # Errors
    /// [`Error::Backend`] when SQLite cannot open the file or apply the schema.
    pub fn open(path: &Path) -> Result<Self> {
        let conn = Connection::open(path)?;
        conn.execute_batch(PRAGMAS)?;
        conn.execute_batch(SCHEMA)?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    /// A connection guard, turning a poisoned lock into an ordinary error.
    fn conn(&self) -> Result<std::sync::MutexGuard<'_, Connection>> {
        self.conn
            .lock()
            .map_err(|e| Error::Backend(format!("knowledge base lock poisoned: {e}")))
    }
}

impl Backend for SqliteBackend {
    fn provider(&self) -> &str {
        SQLITE
    }

    fn put(&self, note: &Knowledge) -> Result<()> {
        let mut conn = self.conn()?;
        let tx = conn.transaction()?;
        // The contract: an empty embedding state means the vectors are no longer valid. Doing
        // the delete *here* is what makes it impossible for a row to outlive its vectors' truth.
        if note.embedding.hash.is_none() {
            tx.execute("delete from vectors where note_id = ?1", params![note.id])?;
        }
        tx.execute(
            "insert into notes (id, title, body, tags, source, content_hash,
                                embed_model, embed_hash, chunks, dimensions, created_at, updated_at)
             values (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
             on conflict(id) do update set
                title = excluded.title, body = excluded.body, tags = excluded.tags,
                source = excluded.source, content_hash = excluded.content_hash,
                embed_model = excluded.embed_model, embed_hash = excluded.embed_hash,
                chunks = excluded.chunks, dimensions = excluded.dimensions,
                updated_at = excluded.updated_at",
            params![
                note.id,
                note.title,
                note.body,
                pack_tags(&note.tags),
                note.source,
                note.content_hash,
                note.embedding.model,
                note.embedding.hash,
                i64::from(note.embedding.chunks),
                i64::from(note.embedding.dimensions),
                note.created_at,
                note.updated_at,
            ],
        )?;
        tx.execute("delete from notes_fts where id = ?1", params![note.id])?;
        tx.execute(
            "insert into notes_fts (id, title, body, tags) values (?1, ?2, ?3, ?4)",
            params![note.id, note.title, note.body, note.tags.join(" ")],
        )?;
        tx.commit()?;
        Ok(())
    }

    fn get(&self, id: &str) -> Result<Option<Knowledge>> {
        let conn = self.conn()?;
        let note = conn
            .prepare(&format!("{SELECT} where id = ?1"))?
            .query_row(params![id], row_to_note)
            .optional()?;
        Ok(note)
    }

    fn list(&self, query: &Query) -> Result<Vec<Knowledge>> {
        let conn = self.conn()?;
        let mut sql = SELECT.to_string();
        let mut binds: Vec<String> = Vec::new();
        for (i, tag) in query.tags.iter().enumerate() {
            sql.push_str(if i == 0 { " where " } else { " and " });
            sql.push_str(&format!(" tags like ?{} ", i + 1));
            binds.push(format!("%,{tag},%"));
        }
        sql.push_str(" order by updated_at desc, id asc");
        if let Some(limit) = query.limit {
            sql.push_str(&format!(" limit {limit}"));
        }
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(params_from_iter(binds.iter()), row_to_note)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    fn delete(&self, id: &str) -> Result<bool> {
        let mut conn = self.conn()?;
        let tx = conn.transaction()?;
        // Vectors go by cascade; the FTS table is a plain table and has no foreign key to fire.
        let removed = tx.execute("delete from notes where id = ?1", params![id])?;
        tx.execute("delete from notes_fts where id = ?1", params![id])?;
        tx.commit()?;
        Ok(removed > 0)
    }

    fn count(&self) -> Result<usize> {
        let conn = self.conn()?;
        let n: i64 = conn.query_row("select count(*) from notes", [], |r| r.get(0))?;
        Ok(n.max(0) as usize)
    }

    fn set_vectors(&self, id: &str, state: &EmbeddingState, vectors: &[Vec<f32>]) -> Result<()> {
        let mut conn = self.conn()?;
        let tx = conn.transaction()?;
        // Ask first, inside the transaction. The foreign key would catch this anyway, but it
        // reports "FOREIGN KEY constraint failed" — which says nothing about *which* note is
        // missing, and reads like a schema bug rather than a caller's mistake.
        let known = tx
            .query_row("select 1 from notes where id = ?1", params![id], |_| Ok(()))
            .optional()?
            .is_some();
        if !known {
            return Err(Error::NoSuchKnowledge(id.to_string()));
        }
        tx.execute("delete from vectors where note_id = ?1", params![id])?;
        {
            let mut insert =
                tx.prepare("insert into vectors (note_id, chunk, vector) values (?1, ?2, ?3)")?;
            for (ix, vector) in vectors.iter().enumerate() {
                insert.execute(params![id, ix as i64, encode_vector(vector)])?;
            }
        }
        tx.execute(
            "update notes set embed_model = ?2, embed_hash = ?3, chunks = ?4, dimensions = ?5
             where id = ?1",
            params![
                id,
                state.model,
                state.hash,
                i64::from(state.chunks),
                i64::from(state.dimensions),
            ],
        )?;
        tx.commit()?;
        Ok(())
    }

    fn search_vectors(&self, query: &[f32], limit: usize) -> Result<Vec<ChunkHit>> {
        if query.is_empty() || limit == 0 {
            return Ok(Vec::new());
        }
        let conn = self.conn()?;
        let mut stmt = conn.prepare("select note_id, chunk, vector from vectors")?;
        let rows = stmt.query_map([], |row| {
            let id: String = row.get(0)?;
            let chunk: i64 = row.get(1)?;
            let blob: Vec<u8> = row.get(2)?;
            Ok((id, chunk, blob))
        })?;
        let mut hits = Vec::new();
        for row in rows {
            let (id, chunk, blob) = row?;
            let score = cosine(query, &decode_vector(&blob));
            if score > 0.0 {
                hits.push(ChunkHit {
                    id,
                    chunk: chunk.max(0) as u32,
                    score,
                });
            }
        }
        Ok(best_per_note(hits, limit))
    }

    fn search_text(&self, query: &str, limit: usize) -> Result<Vec<ChunkHit>> {
        let expr = fts_query(query);
        if expr.is_empty() || limit == 0 {
            return Ok(Vec::new());
        }
        let conn = self.conn()?;
        let mut stmt = conn.prepare(
            "select id, bm25(notes_fts) from notes_fts
             where notes_fts match ?1 order by bm25(notes_fts) limit ?2",
        )?;
        let rows = stmt.query_map(params![expr, limit as i64], |row| {
            Ok(ChunkHit {
                id: row.get(0)?,
                chunk: 0,
                score: bm25_score(row.get::<_, f64>(1)?),
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    fn clear(&self) -> Result<()> {
        let conn = self.conn()?;
        conn.execute_batch("delete from vectors; delete from notes_fts; delete from notes;")?;
        Ok(())
    }
}

/// The column list every read shares, so a schema change has one place to break.
const SELECT: &str = "select id, title, body, tags, source, content_hash,
                             embed_model, embed_hash, chunks, dimensions, created_at, updated_at
                      from notes";

fn row_to_note(row: &rusqlite::Row<'_>) -> rusqlite::Result<Knowledge> {
    let tags: String = row.get(3)?;
    Ok(Knowledge {
        id: row.get(0)?,
        base: None,
        title: row.get(1)?,
        body: row.get(2)?,
        tags: unpack_tags(&tags),
        source: row.get(4)?,
        content_hash: row.get(5)?,
        embedding: EmbeddingState {
            model: row.get(6)?,
            hash: row.get(7)?,
            chunks: row.get::<_, i64>(8)?.max(0) as u32,
            dimensions: row.get::<_, i64>(9)?.max(0) as u32,
        },
        created_at: row.get::<_, i64>(10)?.max(0) as u64,
        updated_at: row.get::<_, i64>(11)?.max(0) as u64,
    })
}

/// Tags as `,a,b,` — bracketed on both sides so `like '%,ops,%'` matches the first and last tag
/// as readily as a middle one, and so `ops` never matches `devops`.
fn pack_tags(tags: &[String]) -> String {
    if tags.is_empty() {
        String::new()
    } else {
        format!(",{},", tags.join(","))
    }
}

fn unpack_tags(packed: &str) -> Vec<String> {
    packed
        .split(',')
        .filter(|t| !t.is_empty())
        .map(ToString::to_string)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::note::{content_hash, embed_text};

    fn backend() -> (tempfile::TempDir, SqliteBackend) {
        let dir = tempfile::tempdir().expect("tempdir");
        let backend = SqliteBackend::open(&dir.path().join(DB_FILE)).expect("open");
        (dir, backend)
    }

    fn note(id: &str, title: &str, body: &str, tags: &[&str]) -> Knowledge {
        let tags: Vec<String> = tags.iter().map(ToString::to_string).collect();
        Knowledge {
            id: id.to_string(),
            base: None,
            title: title.to_string(),
            body: body.to_string(),
            content_hash: content_hash(&embed_text(title, &tags, body)),
            tags,
            source: None,
            embedding: EmbeddingState::default(),
            created_at: 100,
            updated_at: 100,
        }
    }

    #[test]
    fn a_note_survives_the_round_trip_whole() {
        let (_dir, backend) = backend();
        let mut n = note(
            "restart",
            "Restarting the panel",
            "kickstart -k",
            &["ops", "adi"],
        );
        n.source = Some("docs/deploy.md".into());
        backend.put(&n).expect("put");

        let read = backend.get("restart").expect("get").expect("present");
        assert_eq!(read, n);
        assert_eq!(backend.count().expect("count"), 1);
        assert!(backend.get("nope").expect("get").is_none());
    }

    #[test]
    fn tag_filters_match_whole_tags_only() {
        let (_dir, backend) = backend();
        backend.put(&note("a", "A", "", &["ops"])).expect("put");
        backend.put(&note("b", "B", "", &["devops"])).expect("put");
        backend
            .put(&note("c", "C", "", &["ops", "net"]))
            .expect("put");

        let ops = backend
            .list(&Query {
                tags: vec!["ops".into()],
                limit: None,
            })
            .expect("list");
        let ids: Vec<&str> = ops.iter().map(|n| n.id.as_str()).collect();
        assert_eq!(ids.len(), 2, "devops must not match ops: {ids:?}");
        assert!(ids.contains(&"a") && ids.contains(&"c"));

        // Several tags are an AND.
        let both = backend
            .list(&Query {
                tags: vec!["ops".into(), "net".into()],
                limit: None,
            })
            .expect("list");
        assert_eq!(both.len(), 1);
        assert_eq!(both[0].id, "c");
    }

    #[test]
    fn a_list_is_newest_first_and_respects_its_limit() {
        let (_dir, backend) = backend();
        for (i, id) in ["old", "mid", "new"].iter().enumerate() {
            let mut n = note(id, id, "", &[]);
            n.updated_at = 100 + i as u64;
            backend.put(&n).expect("put");
        }
        let all = backend.list(&Query::default()).expect("list");
        assert_eq!(
            all.iter().map(|n| n.id.as_str()).collect::<Vec<_>>(),
            vec!["new", "mid", "old"]
        );
        assert_eq!(
            backend
                .list(&Query {
                    tags: vec![],
                    limit: Some(2)
                })
                .expect("list")
                .len(),
            2
        );
    }

    /// The contract that keeps a stale vector from ever being searched: rewriting a note takes
    /// its vectors with it.
    #[test]
    fn rewriting_a_note_drops_the_vectors_that_described_the_old_one() {
        let (_dir, backend) = backend();
        let mut n = note("a", "A", "first text", &[]);
        backend.put(&n).expect("put");
        let state = EmbeddingState {
            model: Some("jina".into()),
            hash: Some(n.content_hash.clone()),
            chunks: 1,
            dimensions: 3,
        };
        backend
            .set_vectors("a", &state, &[vec![1.0, 0.0, 0.0]])
            .expect("vectors");
        assert_eq!(
            backend
                .search_vectors(&[1.0, 0.0, 0.0], 5)
                .expect("search")
                .len(),
            1
        );

        n.body = "second text".into();
        n.content_hash = content_hash(&n.embed_text());
        backend.put(&n).expect("re-put");

        assert!(
            backend
                .search_vectors(&[1.0, 0.0, 0.0], 5)
                .expect("search")
                .is_empty(),
            "the old vector outlived the text it described"
        );
        let read = backend.get("a").expect("get").expect("present");
        assert!(read.is_stale("jina"), "a rewritten note must read as stale");
    }

    /// The other half of the contract: an edit that leaves the embedded text alone — a new
    /// `source`, say — must not throw away vectors that are still perfectly accurate.
    #[test]
    fn a_note_rewritten_with_its_embedding_intact_keeps_its_vectors() {
        let (_dir, backend) = backend();
        let mut n = note("a", "A", "text", &[]);
        backend.put(&n).expect("put");
        let state = EmbeddingState {
            model: Some("jina".into()),
            hash: Some(n.content_hash.clone()),
            chunks: 1,
            dimensions: 2,
        };
        backend
            .set_vectors("a", &state, &[vec![1.0, 0.0]])
            .expect("vectors");

        n.embedding = state;
        n.source = Some("docs/new.md".into());
        backend.put(&n).expect("re-put");

        assert_eq!(
            backend
                .search_vectors(&[1.0, 0.0], 5)
                .expect("search")
                .len(),
            1
        );
        let read = backend.get("a").expect("get").expect("present");
        assert_eq!(read.source.as_deref(), Some("docs/new.md"));
        assert!(!read.is_stale("jina"), "the text never moved");
    }

    #[test]
    fn vectors_for_a_note_that_is_not_there_are_refused() {
        let (_dir, backend) = backend();
        let err = backend
            .set_vectors("ghost", &EmbeddingState::default(), &[vec![1.0]])
            .unwrap_err();
        assert!(matches!(err, Error::NoSuchKnowledge(_)), "{err:?}");
    }

    #[test]
    fn vector_search_ranks_by_closeness_and_counts_a_note_once() {
        let (_dir, backend) = backend();
        for id in ["near", "far"] {
            backend.put(&note(id, id, "", &[])).expect("put");
        }
        let state = |hash: &str| EmbeddingState {
            model: Some("m".into()),
            hash: Some(hash.into()),
            chunks: 2,
            dimensions: 2,
        };
        // Two chunks for `near`, the second one the better match.
        backend
            .set_vectors("near", &state("h"), &[vec![0.0, 1.0], vec![1.0, 0.05]])
            .expect("vectors");
        backend
            .set_vectors("far", &state("h"), &[vec![0.2, 1.0]])
            .expect("vectors");

        let hits = backend.search_vectors(&[1.0, 0.0], 10).expect("search");
        assert_eq!(hits.len(), 2, "one row per note, not per chunk");
        assert_eq!(hits[0].id, "near");
        assert_eq!(hits[0].chunk, 1, "reported at its best chunk");
        assert!(hits[0].score > hits[1].score);
    }

    #[test]
    fn full_text_search_finds_words_in_title_body_and_tags() {
        let (_dir, backend) = backend();
        backend
            .put(&note(
                "a",
                "Restarting the panel",
                "launchctl kickstart",
                &["ops"],
            ))
            .expect("put");
        backend
            .put(&note("b", "Unrelated", "nothing here", &[]))
            .expect("put");

        for query in ["kickstart", "restarting", "ops"] {
            let hits = backend.search_text(query, 5).expect("search");
            assert_eq!(
                hits.first().map(|h| h.id.as_str()),
                Some("a"),
                "query {query:?}"
            );
        }
        // A query made only of noise is answered with nothing, not an FTS syntax error.
        assert!(
            backend
                .search_text("\" NEAR( AND", 5)
                .expect("search")
                .is_empty()
        );
    }

    #[test]
    fn deleting_takes_the_vectors_and_the_text_index_with_it() {
        let (_dir, backend) = backend();
        backend
            .put(&note("a", "Findable", "body", &[]))
            .expect("put");
        backend
            .set_vectors(
                "a",
                &EmbeddingState {
                    model: Some("m".into()),
                    hash: Some("h".into()),
                    chunks: 1,
                    dimensions: 2,
                },
                &[vec![1.0, 0.0]],
            )
            .expect("vectors");

        assert!(backend.delete("a").expect("delete"));
        assert!(!backend.delete("a").expect("second delete"));
        assert_eq!(backend.count().expect("count"), 0);
        assert!(
            backend
                .search_vectors(&[1.0, 0.0], 5)
                .expect("search")
                .is_empty()
        );
        assert!(
            backend
                .search_text("findable", 5)
                .expect("search")
                .is_empty()
        );
    }

    #[test]
    fn clearing_empties_every_table() {
        let (_dir, backend) = backend();
        backend.put(&note("a", "A", "body", &[])).expect("put");
        backend
            .set_vectors(
                "a",
                &EmbeddingState {
                    model: Some("m".into()),
                    hash: Some("h".into()),
                    chunks: 1,
                    dimensions: 2,
                },
                &[vec![1.0, 0.0]],
            )
            .expect("vectors");
        backend.clear().expect("clear");
        assert_eq!(backend.count().expect("count"), 0);
        assert!(backend.search_text("a", 5).expect("search").is_empty());
        assert!(
            backend
                .search_vectors(&[1.0, 0.0], 5)
                .expect("search")
                .is_empty()
        );
    }

    #[test]
    fn a_base_reopened_still_holds_what_was_written() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join(DB_FILE);
        {
            let backend = SqliteBackend::open(&path).expect("open");
            backend.put(&note("a", "A", "body", &["ops"])).expect("put");
        }
        let reopened = SqliteBackend::open(&path).expect("reopen");
        assert_eq!(reopened.count().expect("count"), 1);
        assert_eq!(
            reopened.get("a").expect("get").expect("present").tags,
            vec!["ops"]
        );
    }
}
