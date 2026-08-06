//! Turning code into vectors, so `search` can rank by meaning instead of by spelling.
//!
//! Upstream this was `lib-embed`, a crate shared with other ADI tools and able to reach an
//! `adi.embed` plugin, ONNX/fastembed, or a hosted API. The indexer only ever used one of
//! those paths in a shipped build — jina-embeddings-v2-base-code on candle — so that is what
//! moved, and the rest stayed behind.

mod config;
mod error;

#[cfg(feature = "candle")]
mod candle;

pub use config::EmbeddingConfig;
pub use error::{EmbedError, Result};

#[cfg(feature = "candle")]
pub use candle::CandleEmbedder;

/// A source of text embeddings.
pub trait Embedder: std::fmt::Debug + Send + Sync {
    /// Embed a batch of texts.
    fn embed(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>>;

    /// Width of the vectors this embedder produces.
    fn dimensions(&self) -> u32;

    /// Model identifier — recorded with every cached embedding, so a model swap invalidates
    /// exactly the vectors it should.
    fn model_name(&self) -> &str;
}

/// The embedder a build without `candle` gets: it never embeds, and says so.
///
/// Indexing still runs (parse, symbols, references, FTS) — only the vector half is missing,
/// and `search` degrades to full-text ranking rather than failing.
#[derive(Debug, Default)]
pub struct NoEmbedder;

impl Embedder for NoEmbedder {
    fn embed(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>> {
        if texts.is_empty() {
            return Ok(vec![]);
        }
        Err(EmbedError::Unavailable(
            "this build has no embedder — rebuild adi-indexer with the `candle` feature for semantic search".to_string(),
        ))
    }

    fn dimensions(&self) -> u32 {
        0
    }

    fn model_name(&self) -> &'static str {
        "none"
    }
}
