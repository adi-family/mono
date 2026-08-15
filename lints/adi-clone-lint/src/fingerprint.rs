// Copyright (c) 2024-2025 Ihor
// SPDX-License-Identifier: BUSL-1.1
// See LICENSE file for details

//! Two fingerprints per fragment, answering two different questions.
//!
//! * [`canonical`] — *is this the same code with the variables renamed?* An exact hash over the
//!   fragment in pre-order with every local replaced by the order it was first seen in. Equal
//!   hashes mean the two fragments are alpha-equivalent, which is a Type-2 clone and nothing
//!   weaker.
//! * [`Signature`] — *how much of this code occurs in that code?* A MinHash over the fragment's
//!   bag of AST paths, which estimates Jaccard overlap in a fixed 32 words no matter how large
//!   the fragment is, and buckets into LSH bands so candidate pairs can be found without
//!   comparing every fragment to every other.
//!
//! The renaming is the part that HIR makes sound. A source-level tool has to guess whether two
//! occurrences of `x` are the same variable; rustc has already resolved them, so
//! [`crate::tree::Terminal::Local`] carries the binding's `HirId` and "first seen in this
//! order" is an exact statement about bindings rather than about spellings.

use rustc_hir::HirId;
use rustc_span::Span;

use crate::paths::{mix, Paths, FNV_OFFSET};
use crate::tree::{NodeId, Terminal, Tree};

/// Words in a MinHash signature. 32 estimates Jaccard to about ±0.09 at one standard
/// deviation — coarse per pair, but the threshold is a screen and every surviving pair is
/// re-checked exactly.
pub const SIG_WORDS: usize = 32;

/// Rows per LSH band. 4 rows over 8 bands makes a pair at 0.8 similarity near-certain to share
/// a band (1 - (1 - 0.8^4)^8 ≈ 0.97) while a pair at 0.3 almost never does (≈ 0.06).
pub const BAND_ROWS: usize = 4;
pub const BANDS: usize = SIG_WORDS / BAND_ROWS;

/// A MinHash signature over a fragment's path bag.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Signature {
    words: [u32; SIG_WORDS],
}

impl Signature {
    /// Estimated Jaccard overlap of the two bags, in `0.0..=1.0`.
    pub fn similarity(&self, other: &Signature) -> f64 {
        let matching = self
            .words
            .iter()
            .zip(other.words.iter())
            .filter(|(a, b)| a == b)
            .count();
        matching as f64 / SIG_WORDS as f64
    }

    /// One key per band. Two fragments share a bucket when any band matches outright, which is
    /// what makes candidate search sublinear instead of all-pairs.
    pub fn bands(&self) -> [u64; BANDS] {
        let mut keys = [0u64; BANDS];
        for (band, key) in keys.iter_mut().enumerate() {
            let mut hash = FNV_OFFSET;
            hash = mix(hash, &(band as u32).to_le_bytes());
            for row in 0..BAND_ROWS {
                hash = mix(hash, &self.words[band * BAND_ROWS + row].to_le_bytes());
            }
            *key = hash;
        }
        keys
    }
}

#[cfg(test)]
impl Signature {
    /// Build a signature directly, for tests that care about grouping rather than about which
    /// paths produced it.
    pub(crate) fn from_words(words: [u32; SIG_WORDS]) -> Self {
        Self { words }
    }
}

/// MinHash the paths lying inside a fragment's leaf range.
pub fn signature(paths: &Paths, leaf_lo: u32, leaf_hi: u32) -> (Signature, u32) {
    let mut words = [u32::MAX; SIG_WORDS];
    let mut count = 0u32;

    for context in paths.in_range(leaf_lo, leaf_hi) {
        count += 1;
        for (slot, word) in words.iter_mut().enumerate() {
            let hashed = (splitmix64(context.loose ^ SEEDS[slot]) >> 32) as u32;
            if hashed < *word {
                *word = hashed;
            }
        }
    }

    (Signature { words }, count)
}

/// What a fragment reduces to once its variables are renamed apart.
#[derive(Debug, Clone)]
pub struct Canonical {
    /// Hash of the pre-order walk. Equal means alpha-equivalent.
    pub hash: u64,
    /// The locals this fragment binds or reads, in the order the renaming numbered them. Two
    /// fragments with the same `hash` have these lists in correspondence, which is what lets
    /// the diagnostic say `sum` stands where `acc` stands.
    pub bindings: Vec<HirId>,
    /// Literal spans in pre-order, likewise in correspondence — the values a Type-2 clone is
    /// allowed to differ in, and which the diagnostic reports as what actually changed.
    pub literals: Vec<Span>,
}

/// Hash a fragment's subtree(s) with locals replaced by first-occurrence order.
pub fn canonical(tree: &Tree, roots: &[NodeId], keep_names: bool) -> Canonical {
    let mut state = Canonical {
        hash: FNV_OFFSET,
        bindings: Vec::new(),
        literals: Vec::new(),
    };
    // Renaming is scoped to the fragment, not the body: two fragments are alpha-equivalent when
    // their *own* bindings line up, whatever else the enclosing function happens to bind.
    let mut seen: Vec<HirId> = Vec::new();

    for &root in roots {
        // A statement run hashes as the concatenation of its statements, so the separator keeps
        // a run of two from colliding with a single statement that happens to contain them.
        state.hash = mix(state.hash, b"|");
        walk(tree, root, keep_names, &mut seen, &mut state);
    }

    state.bindings = seen;
    state
}

fn walk(tree: &Tree, id: NodeId, keep_names: bool, seen: &mut Vec<HirId>, state: &mut Canonical) {
    let node = &tree.nodes[id as usize];

    state.hash = mix(state.hash, node.label.as_bytes());
    if keep_names {
        if let Some(name) = node.name {
            state.hash = mix(state.hash, name.as_str().as_bytes());
        }
    }

    match node.terminal {
        Some(Terminal::Local(hir_id)) => {
            let index = match seen.iter().position(|&s| s == hir_id) {
                Some(index) => index,
                None => {
                    seen.push(hir_id);
                    seen.len() - 1
                }
            };
            state.hash = mix(state.hash, b"v");
            state.hash = mix(state.hash, &(index as u32).to_le_bytes());
        }
        Some(Terminal::Item(def_id)) => {
            state.hash = mix(state.hash, b"item");
            state.hash = mix(state.hash, &def_id.krate.as_u32().to_le_bytes());
            state.hash = mix(state.hash, &def_id.index.as_u32().to_le_bytes());
        }
        Some(Terminal::Lit(class)) => {
            state.hash = mix(state.hash, b"lit");
            state.hash = mix(state.hash, class.as_bytes());
            state.literals.push(node.span);
        }
        Some(Terminal::Named(name)) => {
            state.hash = mix(mix(state.hash, b"named"), name.as_str().as_bytes());
        }
        Some(Terminal::Other) => state.hash = mix(state.hash, b"other"),
        None => {}
    }

    // Descend depth-first. The close marker is what stops `f(g(x))` and `f(g, x)` — same labels
    // in the same pre-order, different tree — from hashing alike.
    state.hash = mix(state.hash, b"(");
    for &child in &node.children {
        walk(tree, child, keep_names, seen, state);
    }
    state.hash = mix(state.hash, b")");
}

/// Independent-enough hash functions for MinHash, from one path hash.
const SEEDS: [u64; SIG_WORDS] = {
    let mut seeds = [0u64; SIG_WORDS];
    let mut i = 0;
    // A fixed table, generated at compile time so the lint's output does not depend on how the
    // build happened to seed anything.
    let mut state = 0x9e37_79b9_7f4a_7c15u64;
    while i < SIG_WORDS {
        state = state.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        seeds[i] = state;
        i += 1;
    }
    seeds
};

/// splitmix64's finalizer — cheap, and avalanches well enough that the 32 seeded variants
/// behave as independent hash functions.
fn splitmix64(mut x: u64) -> u64 {
    x = (x ^ (x >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    x ^ (x >> 31)
}
