//! `hash` — a deterministic bag-of-words embedder: no model, no download, no network.
//!
//! Moved down from `adi-knowledge`, where it lived before this crate existed
//! (`adi_knowledge::HashEmbedder` re-exports this type so every existing caller keeps working
//! unchanged). A registry that cannot list every runtime in one place is not a registry — see
//! `docs/embedding-backends.md`.
//!
//! Each token is hashed into one of [`HASH_DIMENSIONS`] buckets and the vector is L2-normalized,
//! so cosine similarity between two texts is essentially their **word overlap**.
//!
//! Be clear about what that is and isn't. It finds "restart the panel" from "panel restart", and
//! it will never find it from "bring the control surface back up" — it has no idea the two mean
//! the same thing, which is the entire point of a real embedding model. It is here so that tests
//! can assert ranking without a 300MB download, and so a build with every real runtime turned off
//! degrades to something that works rather than to nothing — nameable, now, as `runtime = "hash"`
//! rather than only ever reached as a silent fallback.

use adi_indexer::embed::{EmbedError, Embedder};

/// A deterministic bag-of-words embedder: no model, no download, no network.
#[derive(Debug, Clone, Copy, Default)]
pub struct HashEmbedder;

/// The width of a [`HashEmbedder`] vector. Wide enough that unrelated words rarely collide,
/// narrow enough to be cheap.
pub const HASH_DIMENSIONS: u32 = 256;

/// The model name every [`HashEmbedder`] vector is recorded under, so seeding and validation have
/// one literal to agree on rather than each spelling it out.
pub const MODEL_NAME: &str = "hash-bow-256";

impl Embedder for HashEmbedder {
    fn embed(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, EmbedError> {
        Ok(texts.iter().map(|t| hash_vector(t)).collect())
    }

    fn dimensions(&self) -> u32 {
        HASH_DIMENSIONS
    }

    fn model_name(&self) -> &str {
        MODEL_NAME
    }
}

fn hash_vector(text: &str) -> Vec<f32> {
    let mut v = vec![0f32; HASH_DIMENSIONS as usize];
    for token in text
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| !t.is_empty())
    {
        let bucket = fnv1a(&token.to_lowercase()) as usize % v.len();
        v[bucket] += 1.0;
    }
    let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm > 0.0 {
        for x in &mut v {
            *x /= norm;
        }
    }
    v
}

/// FNV-1a, 64-bit — a stable hash, unlike `DefaultHasher`, whose output is not promised to be
/// the same across Rust releases. Vectors written today must still match a query tomorrow.
fn fnv1a(s: &str) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for b in s.as_bytes() {
        hash ^= u64::from(*b);
        hash = hash.wrapping_mul(0x1000_0000_01b3);
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Vectors here are already L2-normalized, so a plain dot product *is* cosine similarity —
    /// same trick `adi_indexer::embed::candle`'s own test uses.
    fn cosine(a: &[f32], b: &[f32]) -> f32 {
        a.iter().zip(b).map(|(x, y)| x * y).sum()
    }

    #[test]
    fn the_same_text_always_embeds_to_the_same_vector() {
        let e = HashEmbedder;
        let a = e.embed(&["restart the panel"]).expect("embed");
        let b = e.embed(&["restart the panel"]).expect("embed");
        assert_eq!(a, b);
        assert_eq!(a[0].len(), HASH_DIMENSIONS as usize);
    }

    #[test]
    fn word_order_and_case_do_not_change_the_vector() {
        let e = HashEmbedder;
        let a = e.embed(&["Restart The Panel"]).expect("embed");
        let b = e.embed(&["panel restart the"]).expect("embed");
        assert!((cosine(&a[0], &b[0]) - 1.0).abs() < 1e-5);
    }

    #[test]
    fn overlapping_text_scores_above_unrelated_text() {
        let e = HashEmbedder;
        let query = &e.embed(&["restart the control panel"]).expect("embed")[0];
        let close = &e.embed(&["how to restart the panel"]).expect("embed")[0];
        let far = &e.embed(&["sourdough hydration ratios"]).expect("embed")[0];
        assert!(cosine(query, close) > cosine(query, far));
        assert_eq!(cosine(query, far), 0.0);
    }

    #[test]
    fn a_batch_comes_back_in_order_and_empty_text_is_a_zero_vector() {
        let e = HashEmbedder;
        let out = e.embed(&["alpha", "", "beta"]).expect("embed");
        assert_eq!(out.len(), 3);
        assert!(out[1].iter().all(|x| *x == 0.0));
        assert_ne!(out[0], out[2]);
    }
}
