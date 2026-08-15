// Copyright (c) 2024-2025 Ihor
// SPDX-License-Identifier: BUSL-1.1
// See LICENSE file for details

//! Turning a pile of fragments into the handful of findings worth printing.
//!
//! Three passes, and the last two are what make the output readable rather than technically
//! correct:
//!
//! 1. **Exact** — group on the alpha-equivalence hash. These are Type-2 clones: the same code
//!    with the variables renamed, and nothing weaker.
//! 2. **Near** — for what is left, MinHash bands propose candidate pairs and Jaccard over the
//!    path bags decides. Greedy single-pass clustering, so a fragment lands in one group only.
//! 3. **Maximalize** — drop any group that is a smaller retelling of one already kept. Without
//!    this, one duplicated forty-line function reports as several hundred nested findings: every
//!    sub-run of a duplicated run is itself duplicated, and all of them are true.
//!
//! Overlapping members are dropped for the same reason. In `a; a; a;` the runs `[1,2]` and
//! `[2,3]` really are clones of each other, but reporting a pair that shares a statement tells
//! the reader nothing they can act on.

use rustc_span::Span;

use crate::fragment::Fragment;

/// Fragments found to be duplicates of each other.
#[derive(Debug, Clone)]
pub struct CloneGroup {
    /// Indices into the fragment list, largest first.
    pub members: Vec<usize>,
    /// True when the members are alpha-equivalent rather than merely similar.
    pub exact: bool,
    /// Estimated overlap, `1.0` for an exact group.
    pub similarity: f64,
}

/// Group fragments into findings.
pub fn group(fragments: &[Fragment], min_similarity: f64, min_paths: u32) -> Vec<CloneGroup> {
    let mut groups = exact(fragments);
    let mut claimed = vec![false; fragments.len()];
    for group in &groups {
        for &member in &group.members {
            claimed[member] = true;
        }
    }

    groups.extend(near(fragments, &claimed, min_similarity, min_paths));

    // Sort members before dropping overlaps, not after: `drop_overlapping` keeps whichever
    // member it sees first, and `exact` builds its groups by draining a `HashMap`. Ordering
    // afterwards would sort a set that was already chosen at random.
    order(fragments, &mut groups);
    for group in &mut groups {
        drop_overlapping(fragments, group);
    }
    groups.retain(|group| group.members.len() > 1);
    order(fragments, &mut groups);
    let groups = prefer_exact(fragments, groups);
    maximalize(fragments, groups)
}

/// How much of a near finding an exact one must account for before it replaces it.
///
/// Measured in nodes rather than lines. Lines are too coarse to decide this on: the pair that
/// prompted the threshold sat at six lines against nine, which rounds to either side of any
/// round number you pick, while in nodes — the thing actually being compared — it is 36 against
/// 46 and clearly most of it.
const COVERAGE: f64 = 0.7;

/// Drop near findings that an exact finding already explains.
///
/// Both strengths are often raised over the same region: a fragment matches something exactly,
/// and the slightly larger fragment around it — the exact part plus the differing tail — matches
/// the same thing at ninety-odd percent. Ranking those by size alone keeps the *weaker* of the
/// two, which is backwards. An alpha-equivalence match is a proof and states the duplication's
/// real extent; an overlap estimate is neither.
///
/// The coverage test is what keeps this from going too far the other way. A forty-line near
/// clone almost always contains some small exact core, and reporting only the core would throw
/// away the larger finding — so an exact group displaces a near one only when it accounts for
/// most of it, and otherwise both stand and [`maximalize`] settles it on size.
fn prefer_exact(fragments: &[Fragment], groups: Vec<CloneGroup>) -> Vec<CloneGroup> {
    let exact_members: Vec<usize> = groups
        .iter()
        .filter(|group| group.exact)
        .flat_map(|group| group.members.iter().copied())
        .collect();

    groups
        .into_iter()
        .filter(|group| {
            if group.exact {
                return true;
            }
            !group.members.iter().all(|&member| {
                let near = &fragments[member];
                exact_members.iter().any(|&other| {
                    let exact = &fragments[other];
                    overlaps(exact.span, near.span)
                        && f64::from(exact.node_count) >= COVERAGE * f64::from(near.node_count)
                })
            })
        })
        .collect()
}

/// Group on alpha-equivalence.
fn exact(fragments: &[Fragment]) -> Vec<CloneGroup> {
    let mut by_hash: std::collections::HashMap<u64, Vec<usize>> = std::collections::HashMap::new();
    for (index, fragment) in fragments.iter().enumerate() {
        by_hash.entry(fragment.canonical).or_default().push(index);
    }

    by_hash
        .into_values()
        .filter(|members| members.len() > 1)
        .map(|members| CloneGroup {
            members,
            exact: true,
            similarity: 1.0,
        })
        .collect()
}

/// Cluster the rest by path-bag overlap, with LSH proposing the candidates.
fn near(
    fragments: &[Fragment],
    claimed: &[bool],
    min_similarity: f64,
    min_paths: u32,
) -> Vec<CloneGroup> {
    // A fragment with a handful of paths has a signature dominated by which few it happens to
    // hold, and its similarity score is noise. Those are reported on an exact match or not at
    // all.
    let eligible: Vec<usize> = (0..fragments.len())
        .filter(|&i| !claimed[i] && fragments[i].path_count >= min_paths)
        .collect();

    let mut buckets: std::collections::HashMap<u64, Vec<usize>> = std::collections::HashMap::new();
    for &index in &eligible {
        for key in fragments[index].signature.bands() {
            buckets.entry(key).or_default().push(index);
        }
    }

    let mut order: Vec<usize> = eligible.clone();
    order.sort_by_key(|&i| std::cmp::Reverse(fragments[i].node_count));

    let mut taken = vec![false; fragments.len()];
    let mut groups = Vec::new();

    for &seed in &order {
        if taken[seed] {
            continue;
        }

        let mut candidates: Vec<usize> = Vec::new();
        for key in fragments[seed].signature.bands() {
            if let Some(bucket) = buckets.get(&key) {
                candidates.extend(bucket.iter().copied());
            }
        }
        candidates.sort_unstable();
        candidates.dedup();

        let mut members = vec![seed];
        let mut lowest = 1.0f64;
        for candidate in candidates {
            if candidate == seed || taken[candidate] {
                continue;
            }
            let score = fragments[seed]
                .signature
                .similarity(&fragments[candidate].signature);
            if score >= min_similarity {
                members.push(candidate);
                lowest = lowest.min(score);
            }
        }

        if members.len() < 2 {
            continue;
        }
        for &member in &members {
            taken[member] = true;
        }
        groups.push(CloneGroup {
            members,
            exact: false,
            similarity: lowest,
        });
    }

    groups
}

/// Remove members that overlap an earlier member of the same group.
fn drop_overlapping(fragments: &[Fragment], group: &mut CloneGroup) {
    let mut kept: Vec<usize> = Vec::new();
    for &member in &group.members {
        let span = fragments[member].span;
        if kept
            .iter()
            .any(|&other| overlaps(span, fragments[other].span))
        {
            continue;
        }
        kept.push(member);
    }
    group.members = kept;
}

/// Biggest finding first, and deterministically so — the order must not depend on how the
/// hash maps happened to iterate.
fn order(fragments: &[Fragment], groups: &mut [CloneGroup]) {
    for group in groups.iter_mut() {
        group.members.sort_by(|&a, &b| {
            fragments[b]
                .node_count
                .cmp(&fragments[a].node_count)
                .then_with(|| span_key(fragments[a].span).cmp(&span_key(fragments[b].span)))
        });
    }

    groups.sort_by(|a, b| {
        let (a_first, b_first) = (a.members[0], b.members[0]);
        fragments[b_first]
            .line_count
            .cmp(&fragments[a_first].line_count)
            .then_with(|| b.members.len().cmp(&a.members.len()))
            .then_with(|| span_key(fragments[a_first].span).cmp(&span_key(fragments[b_first].span)))
    });
}

/// Drop groups that restate a bigger one.
///
/// `groups` must already be sorted biggest-first, so the first statement of a duplication is
/// the one kept and every sub-run of it falls away.
fn maximalize(fragments: &[Fragment], groups: Vec<CloneGroup>) -> Vec<CloneGroup> {
    let mut kept: Vec<CloneGroup> = Vec::new();

    'candidate: for group in groups {
        for existing in &kept {
            if existing.members.len() < group.members.len() {
                continue;
            }
            // Overlap, not containment. A run and the slightly different run beside it are the
            // same finding told twice, and neither contains the other; requiring containment
            // reported both.
            let subsumed = group.members.iter().all(|&member| {
                existing
                    .members
                    .iter()
                    .any(|&other| overlaps(fragments[other].span, fragments[member].span))
            });
            if subsumed {
                continue 'candidate;
            }
        }
        kept.push(group);
    }

    kept
}

fn overlaps(a: Span, b: Span) -> bool {
    a.ctxt() == b.ctxt() && a.lo() < b.hi() && b.lo() < a.hi()
}

fn span_key(span: Span) -> (u32, u32) {
    (span.lo().0, span.hi().0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fingerprint::{Signature, SIG_WORDS};
    use rustc_span::{BytePos, SyntaxContext};

    /// A fragment with only the fields grouping reads. Spans are byte ranges in one notional
    /// file, which is all `overlaps` looks at.
    fn fragment(lo: u32, hi: u32, canonical: u64, exact_nodes: u32) -> Fragment {
        Fragment {
            hir_id: None,
            span: Span::new(BytePos(lo), BytePos(hi), SyntaxContext::root(), None),
            node_count: exact_nodes,
            line_count: (hi - lo) / 10,
            canonical,
            bindings: Vec::new(),
            literals: Vec::new(),
            signature: Signature::from_words([0; SIG_WORDS]),
            path_count: 100,
            owner: "owner".to_string(),
        }
    }

    fn near_group(members: Vec<usize>, similarity: f64) -> CloneGroup {
        CloneGroup {
            members,
            exact: false,
            similarity,
        }
    }

    #[test]
    fn exact_groups_only_repeats() {
        rustc_span::create_default_session_globals_then(|| {
            let fragments = vec![
                fragment(0, 100, 0xaaaa, 50),
                fragment(200, 300, 0xaaaa, 50),
                fragment(400, 500, 0xbbbb, 50),
            ];

            let groups = exact(&fragments);
            assert_eq!(groups.len(), 1, "the lone fragment is not a group");
            assert_eq!(groups[0].members.len(), 2);
            assert!(groups[0].exact);
        });
    }

    #[test]
    fn overlapping_members_are_dropped() {
        // In `a; a; a;` the runs [1,2] and [2,3] are genuinely clones of each other, and
        // reporting a pair that shares a statement tells the reader nothing they can act on.
        rustc_span::create_default_session_globals_then(|| {
            let fragments = vec![fragment(0, 100, 0xaaaa, 50), fragment(50, 150, 0xaaaa, 50)];
            let mut group = CloneGroup {
                members: vec![0, 1],
                exact: true,
                similarity: 1.0,
            };

            drop_overlapping(&fragments, &mut group);
            assert_eq!(group.members, vec![0]);
        });
    }

    #[test]
    fn a_nested_restatement_is_dropped() {
        // Every sub-run of a duplicated run is itself duplicated. Without maximalization one
        // finding becomes hundreds, all of them true.
        rustc_span::create_default_session_globals_then(|| {
            let fragments = vec![
                fragment(0, 200, 0xaaaa, 100),
                fragment(1000, 1200, 0xaaaa, 100),
                fragment(50, 150, 0xbbbb, 40),
                fragment(1050, 1150, 0xbbbb, 40),
            ];

            let groups = group(&fragments, 0.85, 20);
            assert_eq!(groups.len(), 1, "got {groups:?}");
            assert_eq!(groups[0].members.len(), 2);
            // The surviving finding is the outer, larger one.
            assert!(groups[0].members.iter().all(|&m| fragments[m].node_count == 100));
        });
    }

    #[test]
    fn an_exact_finding_replaces_the_near_one_that_restates_it() {
        // The regression that prompted `prefer_exact`: ranking purely by size kept a fuzzy 88%
        // match over the alpha-equivalence proof sitting inside it.
        rustc_span::create_default_session_globals_then(|| {
            let fragments = vec![
                fragment(0, 90, 0xaaaa, 36),
                fragment(1000, 1090, 0xaaaa, 36),
                fragment(0, 120, 0xbbbb, 46),
                fragment(1000, 1120, 0xcccc, 46),
            ];
            let groups = vec![
                near_group(vec![2, 3], 0.88),
                CloneGroup {
                    members: vec![0, 1],
                    exact: true,
                    similarity: 1.0,
                },
            ];

            let kept = prefer_exact(&fragments, groups);
            assert_eq!(kept.len(), 1, "got {kept:?}");
            assert!(kept[0].exact, "the proof lost to the estimate");
        });
    }

    #[test]
    fn a_small_exact_core_does_not_displace_a_much_larger_near_finding() {
        // The other direction: a long near clone almost always contains some small exact core,
        // and reporting only the core would throw the larger finding away.
        rustc_span::create_default_session_globals_then(|| {
            let fragments = vec![
                fragment(0, 40, 0xaaaa, 20),
                fragment(1000, 1040, 0xaaaa, 20),
                fragment(0, 400, 0xbbbb, 200),
                fragment(1000, 1400, 0xcccc, 200),
            ];
            let groups = vec![
                near_group(vec![2, 3], 0.9),
                CloneGroup {
                    members: vec![0, 1],
                    exact: true,
                    similarity: 1.0,
                },
            ];

            let kept = prefer_exact(&fragments, groups);
            assert_eq!(kept.len(), 2, "the larger near finding was thrown away");
        });
    }

    #[test]
    fn groups_come_back_biggest_first() {
        rustc_span::create_default_session_globals_then(|| {
            let fragments = vec![
                fragment(0, 50, 0xaaaa, 30),
                fragment(1000, 1050, 0xaaaa, 30),
                fragment(2000, 2400, 0xbbbb, 200),
                fragment(3000, 3400, 0xbbbb, 200),
            ];

            let groups = group(&fragments, 0.85, 20);
            assert_eq!(groups.len(), 2);
            assert!(groups[0].members.iter().all(|&m| fragments[m].node_count == 200));
        });
    }
}
