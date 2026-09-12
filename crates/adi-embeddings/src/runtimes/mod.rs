//! The four runtimes behind [`adi_indexer::embed::Embedder`] — see `docs/embedding-backends.md`.
//!
//! `candle` has no module of its own here: it is `adi_indexer::embed::CandleEmbedder`, built
//! directly by `crate::backend::EmbeddingBackends::build` behind the `candle` feature. The other
//! three each need
//! either state a bare function call doesn't have anywhere to live (`hash`'s constants) or a
//! network client (`ollama`, `openai`), so they get one file each.

pub mod hash;
pub mod ollama;
pub mod openai;

/// Install the process-wide `ring` crypto provider `reqwest`'s `rustls-no-provider` feature
/// leaves unset — every blocking client in this crate needs this called first, exactly once.
/// Idempotent: a second install returns `Err`, which is why the result is dropped. See the note
/// on this in the root `Cargo.toml` and `adi_facts::ollama::Ollama::post`, which carries the same
/// line for the same reason — the two crates cannot share it without one depending on the other.
pub(crate) fn install_tls_provider() {
    rustls::crypto::ring::default_provider()
        .install_default()
        .ok();
}
