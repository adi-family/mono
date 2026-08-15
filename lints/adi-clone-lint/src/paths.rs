// Copyright (c) 2024-2025 Ihor
// SPDX-License-Identifier: BUSL-1.1
// See LICENSE file for details

//! AST paths: the representation the whole lint turns on.
//!
//! A path context is one walk between two leaves of the tree — up from the first to their lowest
//! common ancestor, then back down to the second — written as the labels it passes through with
//! the direction marked. Reading `total += row.weight` gives, among others:
//!
//! ```text
//! path ^ assign:+= v field v path
//! ```
//!
//! A fragment is then a *bag* of those, and two fragments are alike to the degree their bags
//! overlap. That buys three things a flat pre-order sequence of node kinds cannot:
//!
//! * **Insertions stay local.** Adding a statement changes the paths that touch it and leaves
//!   every other path in the bag untouched, so the similarity score degrades in proportion to
//!   the edit instead of the sequence shifting wholesale.
//! * **Order between independent siblings stops mattering**, because a bag has no order — two
//!   copies whose statements were shuffled still overlap.
//! * **Partial overlap is measurable.** `|A ∩ B| / |A|` says how much of A occurs inside B,
//!   which is what "these ten lines of the bigger function are duplicated" actually means.
//!
//! Both bounds below exist because the bag is otherwise quadratic in the number of leaves.
//! Widening them costs time and, past a point, precision — long paths through half a function
//! are shared by code that has nothing to do with each other.

use crate::tree::{NodeId, Terminal, Tree};

/// A path between two leaves, already hashed.
///
/// The endpoints are kept as leaf *indices* rather than node ids because a fragment is a
/// contiguous leaf range, which makes "the paths belonging to this fragment" a range query.
#[derive(Debug, Clone, Copy)]
pub struct PathContext {
    pub from: u32,
    pub to: u32,
    /// The path with its endpoint terminals reduced to categories rather than identities.
    pub loose: u64,
}

/// Every path context in a tree, sorted by `from`.
///
/// Sorted so that [`Paths::in_range`] can binary-search rather than filter the whole list once
/// per fragment; with a fragment per subtree that difference is the whole running time.
#[derive(Debug, Default)]
pub struct Paths {
    contexts: Vec<PathContext>,
}

impl Paths {
    /// Extract every path between leaves at most `max_width` apart and at most `max_length`
    /// labels long.
    pub fn extract(tree: &Tree, max_width: u32, max_length: u32) -> Self {
        let mut contexts = Vec::new();
        let leaves = &tree.leaves;

        for i in 0..leaves.len() {
            let upper = (i + max_width as usize + 1).min(leaves.len());
            for j in (i + 1)..upper {
                if let Some(hash) = encode(tree, leaves[i], leaves[j], max_length) {
                    contexts.push(PathContext {
                        from: i as u32,
                        to: j as u32,
                        loose: hash,
                    });
                }
            }
        }

        // Already ascending in `from` by construction; the sort states the invariant that
        // `in_range` depends on rather than trusting the loop above to keep it.
        contexts.sort_unstable_by_key(|c| c.from);
        Self { contexts }
    }

    /// The paths with both endpoints inside the leaf range `lo..hi`.
    pub fn in_range(&self, lo: u32, hi: u32) -> impl Iterator<Item = &PathContext> {
        let start = self.contexts.partition_point(|c| c.from < lo);
        self.contexts[start..]
            .iter()
            .take_while(move |c| c.from < hi)
            .filter(move |c| c.to < hi)
    }

    pub fn is_empty(&self) -> bool {
        self.contexts.is_empty()
    }
}

/// Hash the path from leaf `a` up to the lowest common ancestor and down to leaf `b`.
///
/// `None` when the walk is longer than `max_length`.
fn encode(tree: &Tree, a: NodeId, b: NodeId, max_length: u32) -> Option<u64> {
    let mut up = Vec::new();
    let mut down = Vec::new();

    let (mut x, mut y) = (a, b);
    // Climb the deeper side first so both walks arrive at the same depth, then step together;
    // the first node they agree on is the lowest common ancestor.
    while tree.nodes[x as usize].depth > tree.nodes[y as usize].depth {
        up.push(x);
        x = tree.nodes[x as usize].parent?;
    }
    while tree.nodes[y as usize].depth > tree.nodes[x as usize].depth {
        down.push(y);
        y = tree.nodes[y as usize].parent?;
    }
    while x != y {
        up.push(x);
        down.push(y);
        x = tree.nodes[x as usize].parent?;
        y = tree.nodes[y as usize].parent?;
    }

    if up.len() + down.len() + 1 > max_length as usize {
        return None;
    }

    let mut hash = FNV_OFFSET;
    hash = mix_terminal(hash, tree.nodes[a as usize].terminal);
    for &node in &up {
        hash = mix(hash, b"^");
        hash = mix(hash, tree.nodes[node as usize].label.as_bytes());
    }
    hash = mix(hash, b"@");
    hash = mix(hash, tree.nodes[x as usize].label.as_bytes());
    for &node in down.iter().rev() {
        hash = mix(hash, b"v");
        hash = mix(hash, tree.nodes[node as usize].label.as_bytes());
    }
    hash = mix_terminal(hash, tree.nodes[b as usize].terminal);
    Some(hash)
}

/// Mix a terminal in as what it *is* rather than which one it is.
///
/// Which local a leaf refers to is deliberately dropped: the loose bag exists to measure
/// similarity between fragments that use different variables, so keeping the identity would
/// make every renamed copy score as unrelated. Item paths keep their `DefId`, because calling a
/// different function is a real difference and not a renaming.
///
/// Written as a mix rather than as a `String` the caller hashes — it runs twice per leaf pair,
/// so an allocation here is an allocation per path context.
fn mix_terminal(hash: u64, terminal: Option<Terminal>) -> u64 {
    match terminal {
        Some(Terminal::Local(_)) => mix(hash, b"local"),
        Some(Terminal::Item(def_id)) => {
            let hash = mix(hash, b"item");
            let hash = mix(hash, &def_id.krate.as_u32().to_le_bytes());
            mix(hash, &def_id.index.as_u32().to_le_bytes())
        }
        Some(Terminal::Lit(class)) => mix(mix(hash, b"lit"), class.as_bytes()),
        Some(Terminal::Named(name)) => mix(mix(hash, b"named"), name.as_str().as_bytes()),
        Some(Terminal::Other) => mix(hash, b"other"),
        None => mix(hash, b"node"),
    }
}

pub const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

/// FNV-1a, written out rather than taken from `DefaultHasher`.
///
/// `DefaultHasher`'s algorithm is explicitly allowed to change between releases, and these
/// hashes decide which spans get reported — a silent reshuffle would change the lint's output
/// with no other symptom.
pub fn mix(mut hash: u64, bytes: &[u8]) -> u64 {
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    // Terminate, so that ["ab","c"] and ["a","bc"] cannot hash alike.
    hash ^= 0xff;
    hash.wrapping_mul(FNV_PRIME)
}
