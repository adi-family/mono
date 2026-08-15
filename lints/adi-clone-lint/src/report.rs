// Copyright (c) 2024-2025 Ihor
// SPDX-License-Identifier: BUSL-1.1
// See LICENSE file for details

//! Turning a group into a diagnostic that says something actionable.
//!
//! "This is duplicated" is the least useful half of the finding. What a reader needs in order to
//! decide whether to merge two fragments is *what differs between them*, and for an exact group
//! that is answerable precisely: the two fragments hashed alike because their bindings lined up
//! one for one, so the k-th local of each is the k-th local of the other, and the same holds of
//! their literals. So the diagnostic can state the correspondence outright — `sum` stands where
//! `acc` stands, `100` where `200` does — which is the whole difference between the two, proven
//! rather than guessed.
//!
//! For a near group there is no such correspondence: the fragments matched on how much their
//! path bags overlap, not on an alignment, so the diagnostic reports the score and leaves the
//! reading to the human.

use rustc_hir::{HirId, Node, PatKind};
use rustc_lint::{LateContext, LintContext};
use rustc_span::Span;

use crate::fragment::Fragment;
use crate::group::CloneGroup;
use crate::ADI_DUPLICATE_CODE;

/// Names listed before the note gives up and says "and N more" — a diagnostic nobody finishes
/// reading is a diagnostic nobody acts on.
const MAX_LISTED: usize = 6;

pub fn emit(cx: &LateContext<'_>, fragments: &[Fragment], groups: &[CloneGroup]) {
    for group in groups {
        let Some((&first, rest)) = group.members.split_first() else {
            continue;
        };
        let original = &fragments[first];

        for &member in rest {
            let duplicate = &fragments[member];
            let Some(hir_id) = duplicate.hir_id else {
                continue;
            };

            let renames = if group.exact {
                rename_note(cx, original, duplicate)
            } else {
                None
            };
            let literals = if group.exact {
                literal_note(cx, original, duplicate)
            } else {
                None
            };

            // Say which of the three it is. "With the variables renamed" on a copy that renamed
            // nothing reads as a tool that did not actually look.
            let headline = match (group.exact, renames.is_some()) {
                (true, true) => format!(
                    "duplicated logic: {} lines, the same as `{}` with the variables renamed",
                    duplicate.line_count, original.owner
                ),
                (true, false) => format!(
                    "duplicated logic: {} lines, identical to `{}`",
                    duplicate.line_count, original.owner
                ),
                (false, _) => format!(
                    "duplicated logic: {} lines, about {:.0}% the same as `{}`",
                    duplicate.line_count,
                    group.similarity * 100.0,
                    original.owner
                ),
            };

            cx.tcx.node_span_lint(ADI_DUPLICATE_CODE, hir_id, duplicate.span, |diag| {
                diag.primary_message(headline.clone());
                diag.span_note(original.span, "first written here");
                if let Some(renames) = &renames {
                    diag.note(renames.clone());
                }
                if let Some(literals) = &literals {
                    diag.note(literals.clone());
                }
                diag.help("extract the shared part into a function, or say why the copies must stay apart");
            });
        }
    }
}

/// The binding correspondence between two alpha-equivalent fragments.
///
/// `None` when every local is spelled the same in both — then there is nothing to report beyond
/// the duplication itself.
fn rename_note(cx: &LateContext<'_>, a: &Fragment, b: &Fragment) -> Option<String> {
    // Equal canonical hashes guarantee equal length; a mismatch would mean the hash lied, and
    // zipping is the safe reading of that rather than indexing.
    let pairs: Vec<String> = a
        .bindings
        .iter()
        .zip(b.bindings.iter())
        .filter_map(|(&left, &right)| {
            let (left, right) = (binding_name(cx, left)?, binding_name(cx, right)?);
            (left != right).then(|| format!("`{right}` stands where `{left}` does"))
        })
        .collect();

    summarize("variables renamed: ", pairs)
}

/// The literal values that differ. Type-2 clones are allowed to differ in them, so these are
/// exactly the constants a merged version would have to take as arguments.
fn literal_note(cx: &LateContext<'_>, a: &Fragment, b: &Fragment) -> Option<String> {
    let pairs: Vec<String> = a
        .literals
        .iter()
        .zip(b.literals.iter())
        .filter_map(|(&left, &right)| {
            let (left, right) = (snippet(cx, left)?, snippet(cx, right)?);
            (left != right).then(|| format!("`{left}` becomes `{right}`"))
        })
        .collect();

    summarize("literals that differ: ", pairs)
}

fn summarize(prefix: &str, mut pairs: Vec<String>) -> Option<String> {
    if pairs.is_empty() {
        return None;
    }
    let total = pairs.len();
    pairs.truncate(MAX_LISTED);
    let mut note = format!("{prefix}{}", pairs.join(", "));
    if total > MAX_LISTED {
        note.push_str(&format!(", and {} more", total - MAX_LISTED));
    }
    Some(note)
}

/// The source name of a binding, looked up from the `HirId` its pattern minted.
fn binding_name(cx: &LateContext<'_>, hir_id: HirId) -> Option<String> {
    match cx.tcx.hir_node(hir_id) {
        Node::Pat(pat) => match pat.kind {
            PatKind::Binding(_, _, ident, _) => Some(ident.name.to_string()),
            _ => None,
        },
        _ => None,
    }
}

fn snippet(cx: &LateContext<'_>, span: Span) -> Option<String> {
    cx.sess().source_map().span_to_snippet(span).ok()
}
