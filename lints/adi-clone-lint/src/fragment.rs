// Copyright (c) 2024-2025 Ihor
// SPDX-License-Identifier: BUSL-1.1
// See LICENSE file for details

//! Which pieces of a function are candidates for being a duplicate of something else.
//!
//! Whole functions are the easy case and the least useful one — real duplication is usually a
//! stretch of a larger function that also does other things. So the unit here is a **run of
//! consecutive statements in a block**, from one statement up to [`Limits::max_run`] of them,
//! which is what "these ten lines appear twice" means in tree terms. A single-statement run
//! covers the whole-construct case for free, because an `if`, a `match` and a `for` are each
//! one statement of their enclosing block.
//!
//! Two bounds keep this from exploding. A block with `k` statements has `O(k²)` runs, so the
//! run length is capped; and a run is costed — its node count summed from children the tree
//! already measured — *before* anything hashes it, so the ones under the size floor never get
//! walked at all.

use rustc_hir::HirId;
use rustc_span::source_map::SourceMap;
use rustc_span::Span;

use crate::fingerprint::{self, Signature};
use crate::paths::Paths;
use crate::tree::{NodeId, Tree};

/// The knobs that decide how much gets looked at.
#[derive(Debug, Clone, Copy)]
pub struct Limits {
    pub min_nodes: u32,
    pub min_lines: u32,
    pub max_run: usize,
    pub max_width: u32,
    pub max_length: u32,
}

/// One candidate stretch of code.
#[derive(Debug, Clone)]
pub struct Fragment {
    /// The HIR node the diagnostic is attached to, so a local `#[allow]` is honoured.
    pub hir_id: Option<HirId>,
    pub span: Span,
    pub node_count: u32,
    pub line_count: u32,
    /// Alpha-equivalence hash — see [`crate::fingerprint::canonical`].
    pub canonical: u64,
    /// Locals in renaming order, for the diagnostic's mapping.
    pub bindings: Vec<HirId>,
    /// Literal spans in pre-order, likewise.
    pub literals: Vec<Span>,
    pub signature: Signature,
    /// Paths in the bag. A fragment with too few is not measurable by overlap and is only ever
    /// reported on an exact match.
    pub path_count: u32,
    /// The enclosing item, named for the report.
    pub owner: String,
}

/// Every fragment of one lowered body that clears the size floor.
pub fn collect(
    tree: &Tree,
    paths: &Paths,
    limits: Limits,
    source_map: &SourceMap,
    owner: &str,
) -> Vec<Fragment> {
    let mut fragments = Vec::new();
    if tree.nodes.is_empty() {
        return fragments;
    }

    for (id, node) in tree.nodes.iter().enumerate() {
        if node.label != "block" {
            continue;
        }
        let children = &node.children;

        for start in 0..children.len() {
            let mut node_count = 0u32;
            let upper = (start + limits.max_run).min(children.len());

            for end in start..upper {
                node_count += tree.nodes[children[end] as usize].size;
                if node_count < limits.min_nodes {
                    continue;
                }

                let roots = &children[start..=end];
                if let Some(fragment) =
                    build(tree, paths, limits, source_map, owner, roots, node_count)
                {
                    fragments.push(fragment);
                }
            }
        }
        let _ = id;
    }

    fragments
}

fn build(
    tree: &Tree,
    paths: &Paths,
    limits: Limits,
    source_map: &SourceMap,
    owner: &str,
    roots: &[NodeId],
    node_count: u32,
) -> Option<Fragment> {
    let first = &tree.nodes[*roots.first()? as usize];
    let last = &tree.nodes[*roots.last()? as usize];

    // Normalise to the call site before doing anything else with the span. A desugared node —
    // every `for`, `?` and `.await` — carries a non-root `SyntaxContext`, and two spans with
    // different contexts compare as neither containing nor overlapping the other. Left
    // unnormalised, a `for` loop is never recognised as sitting inside the block that holds it,
    // so nested findings survive maximalization and a fragment gets reported against the run
    // that contains it.
    let span = first.span.source_callsite().to(last.span.source_callsite());
    if crate::tree::from_macro(span) {
        return None;
    }

    let line_count = lines(source_map, span)?;
    if line_count < limits.min_lines {
        return None;
    }

    // Every root of a run is a sibling, so the run's leaves are the first root's through the
    // last's with nothing in between — the contiguity the whole path-range scheme rests on.
    let (leaf_lo, leaf_hi) = (first.leaf_lo, last.leaf_hi);
    let (signature, path_count) = fingerprint::signature(paths, leaf_lo, leaf_hi);
    let canonical = fingerprint::canonical(tree, roots, true);

    Some(Fragment {
        hir_id: first.hir_id,
        span,
        node_count,
        line_count,
        canonical: canonical.hash,
        bindings: canonical.bindings,
        literals: canonical.literals,
        signature,
        path_count,
        owner: owner.to_string(),
    })
}

/// Source lines a span covers, or `None` if it is not in real source.
fn lines(source_map: &SourceMap, span: Span) -> Option<u32> {
    let lo = source_map.lookup_char_pos(span.lo());
    let hi = source_map.lookup_char_pos(span.hi());
    Some((hi.line.saturating_sub(lo.line) as u32) + 1)
}
