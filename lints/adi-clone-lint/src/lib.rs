// Copyright (c) 2024-2025 Ihor
// SPDX-License-Identifier: BUSL-1.1
// See LICENSE file for details

//! A duplication lint that compares **AST paths** rather than text or token streams.
//!
//! Every function body is lowered into an arena ([`tree`]), and each stretch of it is described
//! by the set of walks between nearby leaves of that arena — up to a common ancestor and back
//! down ([`paths`]). Two stretches of code are then duplicates to the degree those sets agree.
//!
//! What that buys over comparing a flat sequence of node kinds is that the comparison degrades
//! gracefully. Inserting a statement perturbs only the paths that touch it; reordering
//! independent statements perturbs none, because a set has no order; and partial overlap is a
//! number rather than a yes or no, which is what makes "ten lines of this function also appear
//! in that one" expressible at all.
//!
//! Findings come in two strengths:
//!
//! * **Exact** — the fragments are alpha-equivalent. rustc has already resolved every
//!   identifier, so [`fingerprint::canonical`] renames locals by the order they are first seen
//!   and two fragments hash alike only if their bindings correspond one for one. That is a
//!   proof, not a heuristic, and it is what lets the diagnostic name the correspondence.
//! * **Near** — the path bags overlap by at least `min_similarity`, estimated by MinHash and
//!   screened by LSH bands so the search is not all-pairs.
//!
//! **This lint warns and never denies.** Duplication is evidence, not a verdict: the ~37 Leptos
//! `view!` functions in this tree are identical in shape because that is what the macro expands
//! to, and merging them would be actively wrong. The tool finds candidates; judgement stays
//! with the reader.
//!
//! ## The limitation to know
//!
//! rustc lints run **per crate**. A duplicate spanning two crates of the workspace is invisible
//! here, because the two halves are never in the same compilation. For cross-crate duplication
//! use `adi-mono indexer clones`, which works over the whole tree at once from the shared index.

#![feature(rustc_private)]
#![warn(unused_extern_crates)]

// `rustc_lint` and `rustc_session` are declared by `impl_late_lint!` below; declaring them
// here as well is a duplicate-definition error rather than a no-op.
extern crate rustc_ast;
extern crate rustc_hir;
extern crate rustc_span;

// Public so the crate-level documentation can link to them. Nothing outside a `cdylib` lint
// can depend on this crate anyway, so there is no API surface to keep narrow.
pub mod fingerprint;
pub mod fragment;
pub mod group;
pub mod paths;
pub mod report;
pub mod tree;

use rustc_hir::Body;
use rustc_lint::{LateContext, LateLintPass, LintContext};
use serde::Deserialize;

use fragment::{Fragment, Limits};
use paths::Paths;
use tree::Tree;

/// Tunables, read from `dylint.toml` at the workspace root under `[adi-clone-lint]`.
#[derive(Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct Config {
    /// Floor on a fragment's size in arena nodes. Load-bearing: below roughly this, every
    /// codebase has thousands of accessors that share a shape and mean nothing by it.
    min_nodes: u32,
    /// Floor on a fragment's height in source lines, so a long one-liner is not a finding.
    min_lines: u32,
    /// Longest run of consecutive statements considered as one fragment. Caps the `O(k²)` runs
    /// a block of `k` statements would otherwise contribute.
    max_run: usize,
    /// How far apart two leaves may be, in leaf positions, to have a path between them.
    max_width: u32,
    /// Longest path, counted in labels passed through.
    max_length: u32,
    /// Path-bag overlap a near match must reach, in `0.0..=1.0`.
    min_similarity: f64,
    /// Paths a fragment needs before its similarity score means anything.
    min_paths: u32,
    /// Whether to report near matches at all, or exact ones only.
    report_near: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            // 25 nodes is roughly a five-line stretch of HIR. It was 40 to begin with, which
            // sounds conservative and quietly cost the main case this lint is for: the shared
            // part of two functions is smaller than either, so a floor set for whole functions
            // never sees it.
            min_nodes: 25,
            min_lines: 5,
            max_run: 12,
            max_width: 4,
            max_length: 12,
            min_similarity: 0.85,
            min_paths: 20,
            report_near: true,
        }
    }
}

impl Config {
    fn limits(&self) -> Limits {
        Limits {
            min_nodes: self.min_nodes,
            min_lines: self.min_lines,
            max_run: self.max_run,
            max_width: self.max_width,
            max_length: self.max_length,
        }
    }
}

dylint_linting::impl_late_lint! {
    /// ### What it does
    ///
    /// Reports stretches of code that duplicate another stretch elsewhere in the same crate,
    /// by comparing the set of AST paths through each.
    ///
    /// ### Why is this bad?
    ///
    /// Duplicated logic has to be found again every time it changes, and the copy that gets
    /// missed is the bug. Copies that survived a rename are the ones a text search cannot find.
    ///
    /// ### Known problems
    ///
    /// Runs per crate, so duplication spanning two crates is invisible. Code that is the same
    /// shape for a good reason — macro-shaped builders, exhaustive `match` arms over parallel
    /// enums — is reported and should be `#[allow]`ed.
    ///
    /// ### Example
    ///
    /// ```ignore
    /// let mut total = 0;
    /// for row in rows { total += row.weight; }
    /// ```
    ///
    /// reported against an earlier
    ///
    /// ```ignore
    /// let mut sum = 0;
    /// for item in items { sum += item.factor; }
    /// ```
    pub ADI_DUPLICATE_CODE,
    Warn,
    "code duplicated elsewhere in the crate, matched on AST paths",
    AdiDuplicateCode::new()
}

pub struct AdiDuplicateCode {
    config: Config,
    /// Every candidate in the crate. Filled as bodies are checked and only grouped once they
    /// all are — a rustc lint sees one body at a time, and duplication is by definition a
    /// statement about two of them.
    fragments: Vec<Fragment>,
}

impl AdiDuplicateCode {
    pub fn new() -> Self {
        Self {
            config: dylint_linting::config_or_default(env!("CARGO_PKG_NAME")),
            fragments: Vec::new(),
        }
    }
}

impl Default for AdiDuplicateCode {
    fn default() -> Self {
        Self::new()
    }
}

impl<'tcx> LateLintPass<'tcx> for AdiDuplicateCode {
    fn check_body(&mut self, cx: &LateContext<'tcx>, body: &Body<'tcx>) {
        let Some(tree) = Tree::build(body.value) else {
            return;
        };
        // Two leaves is the minimum for a single path, and a body with one is not code anyone
        // duplicated on purpose.
        if tree.leaves.len() < 2 {
            return;
        }

        let limits = self.config.limits();
        let paths = Paths::extract(&tree, limits.max_width, limits.max_length);
        if paths.is_empty() {
            return;
        }

        let owner = cx.tcx.hir_body_owner_def_id(body.id());
        let name = cx.tcx.def_path_str(owner.to_def_id());

        self.fragments.extend(fragment::collect(
            &tree,
            &paths,
            limits,
            cx.sess().source_map(),
            &name,
        ));
    }

    fn check_crate_post(&mut self, cx: &LateContext<'tcx>) {
        if self.fragments.is_empty() {
            return;
        }

        let min_similarity = if self.config.report_near {
            self.config.min_similarity
        } else {
            // Above 1.0 nothing can match, which turns the near pass off without giving it a
            // second code path to keep correct.
            f64::INFINITY
        };

        let groups = group::group(&self.fragments, min_similarity, self.config.min_paths);

        // Tuning this lint means arguing with the grouping, and the grouping is invisible from
        // the diagnostics alone — a finding that never appears looks exactly like a finding
        // that was never a candidate. `ADI_CLONE_LINT_DEBUG=1` tells the two apart.
        if std::env::var_os("ADI_CLONE_LINT_DEBUG").is_some() {
            let map = cx.sess().source_map();
            eprintln!("[adi-clone-lint] {} fragments", self.fragments.len());
            for fragment in &self.fragments {
                eprintln!(
                    "  frag {:>4} nodes {:>3} lines {:>3} paths  {:016x}  {}",
                    fragment.node_count,
                    fragment.line_count,
                    fragment.path_count,
                    fragment.canonical,
                    map.span_to_diagnostic_string(fragment.span)
                );
            }
            eprintln!("[adi-clone-lint] {} groups", groups.len());
            for group in &groups {
                eprintln!(
                    "  group exact={} similarity={:.2} members={}",
                    group.exact,
                    group.similarity,
                    group.members.len()
                );
                for &member in &group.members {
                    eprintln!(
                        "    {}",
                        map.span_to_diagnostic_string(self.fragments[member].span)
                    );
                }
            }
        }

        report::emit(cx, &self.fragments, &groups);

        // A crate's findings are reported once; freeing here also keeps the pass from holding
        // every span in the crate alive for the rest of the session.
        self.fragments = Vec::new();
    }
}
