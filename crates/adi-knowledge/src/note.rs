//! The **Knowledge** entity: one text note, of any length, and the derived facts that decide
//! when it has to be embedded again.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::error::{Error, Result};
use crate::scope::BaseId;

/// How much text goes into one embedded chunk, in characters.
///
/// The embedder truncates at 512 tokens (see the indexer's `MAX_TOKENS`), and code-ish English
/// runs roughly 3.5–4 characters per token — so ~1400 characters fills the window without
/// spilling over it. A note shorter than this is one chunk and costs one vector; a long one is
/// split rather than **silently cut off at the 512th token**, which is what "notes of any
/// length" has to mean if it means anything.
pub const CHUNK_CHARS: usize = 1400;

/// How much of the previous chunk each next one repeats.
///
/// A boundary that lands mid-argument would otherwise leave both halves unfindable: neither
/// chunk contains the whole thought, and the query matches whole thoughts. The overlap costs
/// ~15% more vectors and buys back the sentences that straddle a cut.
pub const CHUNK_OVERLAP: usize = 200;

/// One note in a knowledge base.
///
/// `content_hash` is over exactly the text that gets embedded, and `embedding` records the hash
/// and model of the vectors actually stored. When the two disagree the note is
/// [stale](Self::is_stale) — which is the entire mechanism behind "re-embedded whenever they
/// change", and why an edit through any path (CLI, tool, a future API) can never leave a note
/// describing itself with a vector of its old text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Knowledge {
    /// Stable id within the base, derived from the title on creation.
    pub id: String,
    /// Which base this came out of. Filled in by the store on read; backends do not store it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base: Option<BaseId>,
    /// A one-line name for the note.
    pub title: String,
    /// The note itself. Any length.
    pub body: String,
    /// Free-form labels, lowercased and deduplicated.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    /// Where this came from — a URL, a file, a run id — for a reader who wants the original.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    /// SHA-256 of [`Self::embed_text`], hex — what the vectors are checked against.
    pub content_hash: String,
    /// The state of this note's vectors.
    pub embedding: EmbeddingState,
    /// Unix seconds.
    pub created_at: u64,
    /// Unix seconds; moves on every content edit.
    pub updated_at: u64,
}

/// What is known about a note's stored vectors.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct EmbeddingState {
    /// The model that produced them, so a model swap invalidates exactly the vectors it should.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// The content hash they were produced from. `None` means never embedded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hash: Option<String>,
    /// How many chunk vectors are stored.
    #[serde(default)]
    pub chunks: u32,
    /// Vector width.
    #[serde(default)]
    pub dimensions: u32,
}

impl Knowledge {
    /// The text that gets embedded: the title, then the tags, then the body.
    ///
    /// Tags are in because a note tagged `postgres` should answer a question about Postgres even
    /// when the body never spells the word — which is exactly the case a tag is written for.
    #[must_use]
    pub fn embed_text(&self) -> String {
        embed_text(&self.title, &self.tags, &self.body)
    }

    /// The chunks this note embeds as: one per [`CHUNK_CHARS`] window of the body, each carrying
    /// the note's heading so a chunk taken from the middle still says what it is about.
    #[must_use]
    pub fn chunks(&self) -> Vec<String> {
        let heading = heading(&self.title, &self.tags);
        let body = self.body.trim();
        if body.is_empty() {
            return if heading.is_empty() {
                Vec::new()
            } else {
                vec![heading]
            };
        }
        chunk(body)
            .into_iter()
            .map(|part| {
                if heading.is_empty() {
                    part
                } else {
                    format!("{heading}\n\n{part}")
                }
            })
            .collect()
    }

    /// Whether the stored vectors no longer describe this note — either the text changed since
    /// they were made, or they were made by a different model.
    #[must_use]
    pub fn is_stale(&self, model: &str) -> bool {
        self.embedding.hash.as_deref() != Some(self.content_hash.as_str())
            || self.embedding.model.as_deref() != Some(model)
    }

    /// Whether this note has ever been embedded at all.
    #[must_use]
    pub fn is_embedded(&self) -> bool {
        self.embedding.hash.is_some()
    }

    /// The first line or so of the body, for a list that has to fit on a terminal.
    #[must_use]
    pub fn preview(&self, width: usize) -> String {
        let flat = self.body.split_whitespace().collect::<Vec<_>>().join(" ");
        if flat.chars().count() <= width {
            return flat;
        }
        let cut: String = flat.chars().take(width.saturating_sub(1)).collect();
        format!("{}…", cut.trim_end())
    }
}

/// A note being created. The store fills in the id, hashes, and timestamps.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct NewKnowledge {
    /// A one-line name.
    pub title: String,
    /// The note. Any length; may be empty when the title says it all.
    #[serde(default)]
    pub body: String,
    /// Free-form labels.
    #[serde(default)]
    pub tags: Vec<String>,
    /// Where it came from.
    #[serde(default)]
    pub source: Option<String>,
    /// Force a specific id instead of deriving one from the title — for an importer that has to
    /// stay idempotent across runs.
    #[serde(default)]
    pub id: Option<String>,
}

impl NewKnowledge {
    /// A note with just a title and a body.
    #[must_use]
    pub fn new(title: impl Into<String>, body: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            body: body.into(),
            ..Self::default()
        }
    }

    /// Attach tags.
    #[must_use]
    pub fn tagged(mut self, tags: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.tags = tags.into_iter().map(Into::into).collect();
        self
    }
}

/// An edit. Every field is "leave it alone" when absent — the same omit-to-keep rule the rest of
/// the platform's `save` paths follow, so a caller that only knows about titles cannot blank a
/// body it never asked about.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct KnowledgePatch {
    /// A new title.
    #[serde(default)]
    pub title: Option<String>,
    /// A new body.
    #[serde(default)]
    pub body: Option<String>,
    /// A new tag set, replacing the old one wholesale.
    #[serde(default)]
    pub tags: Option<Vec<String>>,
    /// A new source. `Some(None)` clears it.
    #[serde(default)]
    pub source: Option<Option<String>>,
}

impl KnowledgePatch {
    /// Whether this patch would change anything at all.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.title.is_none() && self.body.is_none() && self.tags.is_none() && self.source.is_none()
    }
}

/// What to list. Every field narrows; all-default lists the base.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Filter {
    /// Keep only notes carrying every one of these tags.
    #[serde(default)]
    pub tags: Vec<String>,
    /// Cap the result. `None` means no cap.
    #[serde(default)]
    pub limit: Option<usize>,
    /// Keep only notes whose vectors are out of date — what `reembed` walks.
    #[serde(default)]
    pub stale_only: bool,
}

impl Filter {
    /// A filter that keeps at most `limit` notes.
    #[must_use]
    pub fn limit(limit: usize) -> Self {
        Self {
            limit: Some(limit),
            ..Self::default()
        }
    }

    /// A filter that keeps notes carrying every one of `tags`.
    #[must_use]
    pub fn tagged(tags: impl IntoIterator<Item = impl Into<String>>) -> Self {
        Self {
            tags: tags.into_iter().map(Into::into).collect(),
            ..Self::default()
        }
    }
}

/// One search result: the note, how well it matched, and which of its chunks did.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Hit {
    /// The note that matched.
    pub knowledge: Knowledge,
    /// Similarity in `0.0..=1.0` for a vector search; a normalized BM25 rank for a text one.
    pub score: f32,
    /// Which chunk of the note matched best — 0 for a note that fits in one.
    #[serde(default)]
    pub chunk: u32,
}

/// Normalize a tag set: trimmed, lowercased, de-blanked, deduplicated, sorted.
///
/// Sorted because a tag list is a set, and two notes tagged the same way should hash and compare
/// the same however the tags were typed.
#[must_use]
pub fn normalize_tags(tags: impl IntoIterator<Item = impl AsRef<str>>) -> Vec<String> {
    let mut out: Vec<String> = tags
        .into_iter()
        .map(|t| t.as_ref().trim().to_ascii_lowercase())
        .filter(|t| !t.is_empty())
        .collect();
    out.sort();
    out.dedup();
    out
}

/// The exact text a note's [`Knowledge::content_hash`] is taken over.
#[must_use]
pub fn embed_text(title: &str, tags: &[String], body: &str) -> String {
    let heading = heading(title, tags);
    let body = body.trim();
    match (heading.is_empty(), body.is_empty()) {
        (true, _) => body.to_string(),
        (false, true) => heading,
        (false, false) => format!("{heading}\n\n{body}"),
    }
}

/// SHA-256 of `text`, hex — the content address a stored vector is checked against.
#[must_use]
pub fn content_hash(text: &str) -> String {
    hex::encode(Sha256::digest(text.as_bytes()))
}

/// Turn a title into a stable, filesystem-safe id: lowercased, non-alphanumerics collapsed to
/// single dashes, trimmed to something a person can still read and type.
///
/// # Errors
/// [`Error::Empty`] when the title has nothing an id could be made of.
pub fn slug(title: &str) -> Result<String> {
    let mut out = String::new();
    for ch in title.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    let slug: String = out.trim_matches('-').chars().take(60).collect();
    let slug = slug.trim_matches('-').to_string();
    if slug.is_empty() {
        return Err(Error::Empty);
    }
    Ok(slug)
}

/// The heading every chunk of a note carries: `title [tag, tag]`.
fn heading(title: &str, tags: &[String]) -> String {
    let title = title.trim();
    match (title.is_empty(), tags.is_empty()) {
        (true, true) => String::new(),
        (true, false) => format!("[{}]", tags.join(", ")),
        (false, true) => title.to_string(),
        (false, false) => format!("{title} [{}]", tags.join(", ")),
    }
}

/// Split `text` into overlapping windows of at most [`CHUNK_CHARS`] characters.
///
/// Windows end on a paragraph or word boundary when one is available in the last quarter of the
/// window, and fall back to a hard cut when a single unbroken run is longer than the budget
/// (minified JSON, a base64 blob) — the cut is at a character boundary, never inside a
/// multi-byte character.
#[must_use]
pub fn chunk(text: &str) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    if chars.is_empty() {
        return Vec::new();
    }
    if chars.len() <= CHUNK_CHARS {
        return vec![text.to_string()];
    }

    let mut out = Vec::new();
    let mut start = 0usize;
    while start < chars.len() {
        let hard_end = (start + CHUNK_CHARS).min(chars.len());
        let end = if hard_end == chars.len() {
            hard_end
        } else {
            // Only look for a boundary in the last quarter: a break any earlier than that throws
            // away more of the window than a clean seam is worth.
            let floor = start + (CHUNK_CHARS * 3 / 4);
            break_at(&chars, floor, hard_end).unwrap_or(hard_end)
        };
        let piece: String = chars[start..end].iter().collect();
        let piece = piece.trim();
        if !piece.is_empty() {
            out.push(piece.to_string());
        }
        if end >= chars.len() {
            break;
        }
        // Step back by the overlap, but always make progress — otherwise a pathological
        // boundary could hand back the same window forever.
        start = end.saturating_sub(CHUNK_OVERLAP).max(start + 1);
    }
    out
}

/// The best place to end a window inside `floor..hard_end`: after the last paragraph break, else
/// after the last whitespace, else nowhere.
fn break_at(chars: &[char], floor: usize, hard_end: usize) -> Option<usize> {
    let window = &chars[floor..hard_end];
    window
        .iter()
        .enumerate()
        .rev()
        .find(|(i, c)| **c == '\n' && *i > 0 && window[i - 1] == '\n')
        .or_else(|| {
            window
                .iter()
                .enumerate()
                .rev()
                .find(|(_, c)| c.is_whitespace())
        })
        .map(|(i, _)| floor + i + 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn note(title: &str, body: &str) -> Knowledge {
        let tags = vec!["ops".to_string()];
        let text = embed_text(title, &tags, body);
        Knowledge {
            id: slug(title).unwrap(),
            base: None,
            title: title.to_string(),
            body: body.to_string(),
            tags,
            source: None,
            content_hash: content_hash(&text),
            embedding: EmbeddingState::default(),
            created_at: 1,
            updated_at: 1,
        }
    }

    #[test]
    fn a_note_is_stale_until_its_vectors_match_its_text_and_model() {
        let mut n = note("Restarting the panel", "launchctl kickstart -k …");
        assert!(n.is_stale("jina"), "never embedded");
        assert!(!n.is_embedded());

        n.embedding = EmbeddingState {
            model: Some("jina".into()),
            hash: Some(n.content_hash.clone()),
            chunks: 1,
            dimensions: 768,
        };
        assert!(!n.is_stale("jina"));
        assert!(n.is_embedded());

        // A different model invalidates vectors that are otherwise current.
        assert!(n.is_stale("other-model"));
    }

    /// The mechanism the whole "re-embed on change" promise rests on.
    #[test]
    fn editing_the_text_moves_the_content_hash() {
        let before = note("Title", "body");
        let after = note("Title", "body, revised");
        assert_ne!(before.content_hash, after.content_hash);

        // ... and so does editing only the title, or only the tags.
        assert_ne!(
            before.content_hash,
            note("Other title", "body").content_hash
        );
        let mut retagged = before.clone();
        retagged.tags = vec!["net".into()];
        let rehashed = content_hash(&retagged.embed_text());
        assert_ne!(before.content_hash, rehashed);
    }

    #[test]
    fn a_short_note_is_one_chunk() {
        let n = note("Short", "two words");
        let chunks = n.chunks();
        assert_eq!(chunks.len(), 1);
        assert!(chunks[0].starts_with("Short [ops]"));
        assert!(chunks[0].contains("two words"));
    }

    #[test]
    fn a_long_note_is_split_and_every_chunk_says_what_it_is_about() {
        let body = "paragraph one is quite long. ".repeat(400);
        let n = note("Runbook", &body);
        let chunks = n.chunks();
        assert!(chunks.len() > 5, "got {} chunks", chunks.len());
        for c in &chunks {
            assert!(c.starts_with("Runbook [ops]"), "chunk lost its heading");
        }
        // The chunker's budget is over the body; the heading rides on top of it.
        let heading = "Runbook [ops]\n\n".chars().count();
        for c in &chunks {
            assert!(
                c.chars().count() <= CHUNK_CHARS + heading,
                "chunk of {} chars overflows the window",
                c.chars().count()
            );
        }
    }

    #[test]
    fn the_whole_text_survives_chunking() {
        // Every word of a long body must appear in some chunk — a splitter that drops the tail
        // loses knowledge silently, which is the one failure nobody would notice.
        // `w<n>z`, not `word<n>`: "word1" is a substring of "word10", so a splitter that lost
        // the tail would still pass. The trailing `z` makes each token its own needle.
        let body: String = (0..900).map(|i| format!("w{i}z ")).collect();
        let chunks = chunk(body.trim());
        let joined = chunks.join(" ");
        for i in [0usize, 1, 450, 898, 899] {
            assert!(joined.contains(&format!("w{i}z")), "lost w{i}z");
        }
    }

    #[test]
    fn chunks_overlap_so_a_thought_cut_in_half_is_still_findable() {
        let body: String = (0..900).map(|i| format!("w{i}z ")).collect();
        let chunks = chunk(body.trim());
        assert!(chunks.len() > 1);
        let first_tail: Vec<&str> = chunks[0].split_whitespace().rev().take(10).collect();
        let second: &str = &chunks[1];
        assert!(
            first_tail.iter().any(|w| second.contains(*w)),
            "no overlap between consecutive chunks"
        );
    }

    /// A body with no whitespace at all — minified JSON, a base64 blob — still has to split.
    #[test]
    fn an_unbroken_run_is_cut_rather_than_left_whole() {
        let body = "x".repeat(CHUNK_CHARS * 3);
        let chunks = chunk(&body);
        assert!(chunks.len() >= 3, "got {} chunks", chunks.len());
        for c in &chunks {
            assert!(c.chars().count() <= CHUNK_CHARS);
        }
    }

    /// Character boundaries, not byte boundaries: a hard cut through a multi-byte character
    /// would panic or produce mojibake.
    #[test]
    fn multibyte_text_is_cut_between_characters() {
        let body = "日本語のテキスト".repeat(600);
        let chunks = chunk(&body);
        assert!(chunks.len() > 1);
        for c in &chunks {
            assert!(c.chars().count() <= CHUNK_CHARS);
            assert!(c.is_char_boundary(0));
        }
    }

    #[test]
    fn an_empty_note_has_nothing_to_embed() {
        let n = Knowledge {
            title: String::new(),
            body: String::new(),
            tags: Vec::new(),
            ..note("placeholder", "")
        };
        assert!(n.chunks().is_empty());
        assert!(n.embed_text().is_empty());
    }

    #[test]
    fn tags_are_a_set_however_they_were_typed() {
        assert_eq!(
            normalize_tags(["Ops", " ops ", "net", ""]),
            vec!["net".to_string(), "ops".to_string()]
        );
    }

    #[test]
    fn an_id_is_derived_from_the_title_and_stays_typeable() {
        assert_eq!(slug("Restarting app.adi!").unwrap(), "restarting-app-adi");
        assert_eq!(slug("  ---  ").ok(), None);
        assert!(slug(&"a".repeat(200)).unwrap().chars().count() <= 60);
    }

    #[test]
    fn a_preview_never_runs_past_its_width() {
        let n = note("t", &"long ".repeat(100));
        assert!(n.preview(20).chars().count() <= 20);
        assert_eq!(note("t", "short").preview(20), "short");
    }
}
