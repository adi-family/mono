//! The in-memory provider: the same contract as [`sqlite`](super::sqlite), in a map, gone when
//! the process is.
//!
//! It exists for two reasons. Tests get a base with no file to clean up and no SQLite in the
//! way of what they are actually asserting. And it is the worked example a third-party provider
//! can be written from — everything the trait requires and nothing else.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use crate::error::{Error, Result};
use crate::note::{EmbeddingState, Knowledge};

use super::{
    Backend, BaseContext, ChunkHit, MEMORY, Provider, Query, best_per_note, cosine,
};

/// Opens [`MemoryBackend`]s, and hands the same one back for the same base.
///
/// Reopening has to return the same instance: a store opens a base per call, and a provider that
/// handed out a fresh empty map each time would lose every write between two reads — which looks
/// exactly like a storage bug in whatever is being tested.
#[derive(Debug, Default)]
pub struct MemoryProvider {
    bases: Mutex<BTreeMap<String, Arc<MemoryBackend>>>,
}

impl Provider for MemoryProvider {
    fn name(&self) -> &str {
        MEMORY
    }

    fn description(&self) -> &str {
        "In process, in a map, gone on exit — for tests and scratch bases."
    }

    fn open(&self, ctx: &BaseContext) -> Result<Arc<dyn Backend>> {
        let mut bases = self
            .bases
            .lock()
            .map_err(|e| Error::Backend(format!("memory provider lock poisoned: {e}")))?;
        let backend = bases
            .entry(ctx.id.to_string())
            .or_insert_with(|| Arc::new(MemoryBackend::default()))
            .clone();
        Ok(backend)
    }
}

/// One base, held in memory.
#[derive(Debug, Default)]
pub struct MemoryBackend {
    notes: Mutex<BTreeMap<String, Entry>>,
}

#[derive(Debug, Clone)]
struct Entry {
    note: Knowledge,
    vectors: Vec<Vec<f32>>,
}

impl MemoryBackend {
    fn notes(&self) -> Result<std::sync::MutexGuard<'_, BTreeMap<String, Entry>>> {
        self.notes
            .lock()
            .map_err(|e| Error::Backend(format!("memory base lock poisoned: {e}")))
    }
}

impl Backend for MemoryBackend {
    fn provider(&self) -> &str {
        MEMORY
    }

    fn put(&self, note: &Knowledge) -> Result<()> {
        let mut notes = self.notes()?;
        let mut stored = note.clone();
        // The base a note came from is the store's business, not storage's; a backend that kept
        // it could hand back a note labelled with someone else's base after a copy.
        stored.base = None;
        // Same rule as the SQLite backend: an empty embedding state means the vectors are gone.
        let keep = if note.embedding.hash.is_some() {
            notes.get(&note.id).map(|e| e.vectors.clone()).unwrap_or_default()
        } else {
            Vec::new()
        };
        notes.insert(
            note.id.clone(),
            Entry {
                note: stored,
                vectors: keep,
            },
        );
        Ok(())
    }

    fn get(&self, id: &str) -> Result<Option<Knowledge>> {
        Ok(self.notes()?.get(id).map(|e| e.note.clone()))
    }

    fn list(&self, query: &Query) -> Result<Vec<Knowledge>> {
        let notes = self.notes()?;
        let mut out: Vec<Knowledge> = notes
            .values()
            .filter(|e| query.tags.iter().all(|t| e.note.tags.contains(t)))
            .map(|e| e.note.clone())
            .collect();
        out.sort_by(|a, b| b.updated_at.cmp(&a.updated_at).then_with(|| a.id.cmp(&b.id)));
        if let Some(limit) = query.limit {
            out.truncate(limit);
        }
        Ok(out)
    }

    fn delete(&self, id: &str) -> Result<bool> {
        Ok(self.notes()?.remove(id).is_some())
    }

    fn count(&self) -> Result<usize> {
        Ok(self.notes()?.len())
    }

    fn set_vectors(&self, id: &str, state: &EmbeddingState, vectors: &[Vec<f32>]) -> Result<()> {
        let mut notes = self.notes()?;
        let entry = notes
            .get_mut(id)
            .ok_or_else(|| Error::NoSuchKnowledge(id.to_string()))?;
        entry.note.embedding = state.clone();
        entry.vectors = vectors.to_vec();
        Ok(())
    }

    fn search_vectors(&self, query: &[f32], limit: usize) -> Result<Vec<ChunkHit>> {
        if query.is_empty() || limit == 0 {
            return Ok(Vec::new());
        }
        let notes = self.notes()?;
        let hits = notes
            .values()
            .flat_map(|e| {
                e.vectors.iter().enumerate().map(|(ix, v)| ChunkHit {
                    id: e.note.id.clone(),
                    chunk: ix as u32,
                    score: cosine(query, v),
                })
            })
            .filter(|h| h.score > 0.0)
            .collect();
        Ok(best_per_note(hits, limit))
    }

    /// Token containment, scored by how much of the query a note carries.
    ///
    /// Not BM25 — this is the fallback path of a fallback path, and a map has no inverted index
    /// to be clever with. It finds what contains the words, which is what it claims to do.
    fn search_text(&self, query: &str, limit: usize) -> Result<Vec<ChunkHit>> {
        let terms: Vec<String> = query
            .split_whitespace()
            .map(str::to_ascii_lowercase)
            .filter(|t| !t.is_empty())
            .collect();
        if terms.is_empty() || limit == 0 {
            return Ok(Vec::new());
        }
        let notes = self.notes()?;
        let hits = notes
            .values()
            .filter_map(|e| {
                let hay = format!(
                    "{} {} {}",
                    e.note.title,
                    e.note.body,
                    e.note.tags.join(" ")
                )
                .to_ascii_lowercase();
                let matched = terms.iter().filter(|t| hay.contains(t.as_str())).count();
                (matched > 0).then(|| ChunkHit {
                    id: e.note.id.clone(),
                    chunk: 0,
                    score: matched as f32 / terms.len() as f32,
                })
            })
            .collect();
        Ok(best_per_note(hits, limit))
    }

    fn clear(&self) -> Result<()> {
        self.notes()?.clear();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::note::{content_hash, embed_text};
    use crate::scope::BaseId;

    fn note(id: &str, title: &str, body: &str) -> Knowledge {
        Knowledge {
            id: id.into(),
            base: None,
            title: title.into(),
            body: body.into(),
            tags: Vec::new(),
            source: None,
            content_hash: content_hash(&embed_text(title, &[], body)),
            embedding: EmbeddingState::default(),
            created_at: 1,
            updated_at: 1,
        }
    }

    fn ctx(id: &str) -> BaseContext {
        BaseContext {
            id: id.parse::<BaseId>().expect("base id"),
            dir: std::path::PathBuf::from("/nonexistent"),
            settings: BTreeMap::new(),
        }
    }

    /// The whole reason the provider keeps a map: a store opens the base again on every call.
    #[test]
    fn reopening_a_base_finds_what_was_written_to_it() {
        let provider = MemoryProvider::default();
        provider
            .open(&ctx("global/notes"))
            .expect("open")
            .put(&note("a", "A", "body"))
            .expect("put");

        let again = provider.open(&ctx("global/notes")).expect("reopen");
        assert_eq!(again.count().expect("count"), 1);

        // A different base is a different map.
        let other = provider.open(&ctx("global/other")).expect("open other");
        assert_eq!(other.count().expect("count"), 0);
    }

    #[test]
    fn it_keeps_the_same_put_contract_as_sqlite() {
        let backend = MemoryBackend::default();
        backend.put(&note("a", "A", "first")).expect("put");
        backend
            .set_vectors(
                "a",
                &EmbeddingState { model: Some("m".into()), hash: Some("h".into()), chunks: 1, dimensions: 2 },
                &[vec![1.0, 0.0]],
            )
            .expect("vectors");
        assert_eq!(backend.search_vectors(&[1.0, 0.0], 5).expect("search").len(), 1);

        // Carrying the embedding state forward keeps the vectors …
        let mut kept = note("a", "A", "first");
        kept.embedding = EmbeddingState {
            model: Some("m".into()),
            hash: Some("h".into()),
            chunks: 1,
            dimensions: 2,
        };
        kept.source = Some("elsewhere".into());
        backend.put(&kept).expect("re-put with state");
        assert_eq!(backend.search_vectors(&[1.0, 0.0], 5).expect("search").len(), 1);

        // … and clearing it takes them away.
        backend.put(&note("a", "A", "second")).expect("re-put");
        assert!(
            backend.search_vectors(&[1.0, 0.0], 5).expect("search").is_empty(),
            "a rewrite must take the old vectors with it"
        );
        assert!(!backend.get("a").expect("get").expect("present").is_embedded());
    }

    #[test]
    fn text_search_scores_by_how_much_of_the_query_a_note_carries() {
        let backend = MemoryBackend::default();
        backend.put(&note("both", "restart panel", "")).expect("put");
        backend.put(&note("one", "restart nothing", "")).expect("put");

        let hits = backend.search_text("restart panel", 5).expect("search");
        assert_eq!(hits[0].id, "both");
        assert!(hits[0].score > hits[1].score);
        assert!(backend.search_text("absent", 5).expect("search").is_empty());
    }
}
