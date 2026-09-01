// Copyright (c) 2024-2025 Ihor
// SPDX-License-Identifier: BUSL-1.1
// See LICENSE file for details

//! Grouping symbols that share a shape, over the fingerprints [`crate::structure`] produced.
//!
//! Two questions, two functions. [`exact`] answers *which symbols are the same shape* by
//! grouping on the fingerprint hash — copy-paste with the names changed, and nothing else.
//! [`near`] answers *which symbols are nearly the same shape* by clustering on Hamming distance
//! between simhashes, which also catches the copy that has since drifted a line or two.
//!
//! Both run in memory over the whole set rather than in SQL. Near-duplicate search is
//! inherently all-pairs — no index answers "within 4 bits of this" — and at a few thousand
//! symbols an XOR and a popcount per pair costs less than the round trips would.

use std::collections::HashMap;

use crate::storage::StructureRow;
use crate::structure::hamming;
use crate::types::SymbolKind;

/// The symbol kinds a duplication report is about.
///
/// Containers are deliberately absent. A module's fingerprint is the concatenation of its
/// children's, so reporting it says nothing its children did not already say — and says it
/// loudly, because container symbols are the largest ones in any index and sort to the top.
/// Left in, this repository's near-duplicate report was 18 of its first 18 rows `module tests`,
/// which is true (all large test modules do look alike) and useless.
///
/// The same argument applies to a class or a trait, whose methods are indexed separately. What
/// remains is the code units a reader would actually go and deduplicate.
#[must_use]
pub fn is_reportable(kind: SymbolKind) -> bool {
    matches!(
        kind,
        SymbolKind::Function
            | SymbolKind::Method
            | SymbolKind::Constructor
            | SymbolKind::Destructor
            | SymbolKind::Operator
            | SymbolKind::Macro
    )
}

/// Symbols found to share a shape.
#[derive(Debug, Clone, serde::Serialize)]
pub struct CloneGroup {
    /// The members, largest first. Always two or more.
    pub members: Vec<StructureRow>,
    /// How far apart the members are: 0 when they are exactly the same shape, otherwise the
    /// widest Hamming distance from the group's first member.
    pub distance: u32,
}

impl CloneGroup {
    /// Lines the group's largest member spans — what ranking a report by "worth looking at"
    /// comes down to.
    #[must_use]
    pub fn lines(&self) -> u32 {
        self.members.first().map_or(0, StructureRow::line_count)
    }
}

/// Group symbols by identical fingerprint.
///
/// A group is two or more symbols whose syntax differs in nothing but names and literal
/// values. Groups come back largest-symbol first, so what is worth reading is at the top.
#[must_use]
pub fn exact(rows: Vec<StructureRow>) -> Vec<CloneGroup> {
    let mut by_hash: HashMap<String, Vec<StructureRow>> = HashMap::new();
    for row in rows {
        by_hash
            .entry(row.structure.hash.clone())
            .or_default()
            .push(row);
    }

    let mut groups: Vec<CloneGroup> = by_hash
        .into_values()
        .filter(|members| members.len() > 1)
        .map(|mut members| {
            members.sort_by(order_within_group);
            CloneGroup {
                members,
                distance: 0,
            }
        })
        .collect();

    groups.sort_by(order_groups);
    groups
}

/// Cluster symbols whose shapes are within `max_distance` bits of each other.
///
/// Greedy single-pass clustering: each unclaimed symbol seeds a group and takes every unclaimed
/// symbol within range of it. That is not the same as finding maximal clusters — a symbol close
/// to two seeds lands with whichever came first — but it is deterministic given the input order
/// and it never reports the same symbol twice, which matters more in a report meant to be read.
///
/// Exact duplicates are excluded: they are [`exact`]'s answer, and leaving them in would bury
/// the drifted copies this is for.
#[must_use]
pub fn near(rows: Vec<StructureRow>, max_distance: u32) -> Vec<CloneGroup> {
    let mut rows = rows;
    rows.sort_by(order_within_group);

    let mut claimed = vec![false; rows.len()];
    let mut groups = Vec::new();

    for seed in 0..rows.len() {
        if claimed[seed] {
            continue;
        }

        let mut members = vec![seed];
        let mut widest = 0;
        for candidate in (seed + 1)..rows.len() {
            if claimed[candidate] {
                continue;
            }
            let distance = hamming(
                rows[seed].structure.simhash,
                rows[candidate].structure.simhash,
            );
            if distance <= max_distance {
                members.push(candidate);
                widest = widest.max(distance);
            }
        }

        if members.len() < 2 {
            continue;
        }
        // Every member identical to the seed means `exact` already reported this group.
        if members
            .iter()
            .all(|&i| rows[i].structure.hash == rows[seed].structure.hash)
        {
            for &i in &members {
                claimed[i] = true;
            }
            continue;
        }

        for &i in &members {
            claimed[i] = true;
        }
        groups.push(CloneGroup {
            members: members.into_iter().map(|i| rows[i].clone()).collect(),
            distance: widest,
        });
    }

    groups.sort_by(order_groups);
    groups
}

/// Symbols within a group: biggest first, then by location so the order never depends on how
/// SQLite happened to return the rows.
fn order_within_group(a: &StructureRow, b: &StructureRow) -> std::cmp::Ordering {
    b.structure
        .node_count
        .cmp(&a.structure.node_count)
        .then_with(|| a.file_path.cmp(&b.file_path))
        .then_with(|| a.start_line.cmp(&b.start_line))
}

/// Groups against each other: the most duplicated code first — biggest symbol, then most
/// copies of it.
fn order_groups(a: &CloneGroup, b: &CloneGroup) -> std::cmp::Ordering {
    b.lines()
        .cmp(&a.lines())
        .then_with(|| b.members.len().cmp(&a.members.len()))
        .then_with(|| {
            a.members
                .first()
                .map(|m| (&m.file_path, m.start_line))
                .cmp(&b.members.first().map(|m| (&m.file_path, m.start_line)))
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::structure::Structure;
    use crate::types::{SymbolId, SymbolKind};
    use std::path::PathBuf;

    fn row(id: i64, hash: &str, simhash: u64, nodes: u32) -> StructureRow {
        StructureRow {
            id: SymbolId(id),
            name: format!("symbol_{id}"),
            kind: SymbolKind::Function,
            file_path: PathBuf::from(format!("src/f{id}.rs")),
            start_line: 1,
            end_line: 10,
            structure: Structure {
                hash: hash.to_string(),
                simhash,
                node_count: nodes,
            },
        }
    }

    #[test]
    fn exact_groups_only_repeats() {
        let groups = exact(vec![
            row(1, "aaaa", 0b1111, 50),
            row(2, "aaaa", 0b1111, 50),
            row(3, "bbbb", 0b0000, 50),
        ]);

        assert_eq!(groups.len(), 1, "the lone symbol is not a group");
        assert_eq!(groups[0].members.len(), 2);
        assert_eq!(groups[0].distance, 0);
    }

    #[test]
    fn exact_ignores_names_and_locations() {
        // Same hash, different files — that is the whole point.
        let groups = exact(vec![row(1, "aaaa", 1, 80), row(2, "aaaa", 1, 80)]);
        assert_eq!(groups.len(), 1);
    }

    #[test]
    fn near_clusters_within_the_distance_and_not_past_it() {
        // 0b0000 vs 0b0011 is 2 bits; vs 0b1111_1111 is 8.
        let groups = near(
            vec![
                row(1, "aaaa", 0b0000, 60),
                row(2, "bbbb", 0b0011, 60),
                row(3, "cccc", 0b1111_1111, 60),
            ],
            2,
        );

        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].members.len(), 2);
        assert_eq!(groups[0].distance, 2);
    }

    #[test]
    fn near_leaves_exact_duplicates_to_exact() {
        // Identical hashes are 0 bits apart, so a distance filter alone would report them —
        // and drown the drifted pairs this is for.
        let groups = near(
            vec![row(1, "aaaa", 0b101, 60), row(2, "aaaa", 0b101, 60)],
            4,
        );
        assert!(groups.is_empty(), "got {groups:?}");
    }

    #[test]
    fn near_reports_each_symbol_once() {
        // A chain where 1~2, 2~3, but 1 and 3 are 4 apart: greedy claiming must not put
        // symbol 2 in two groups.
        let groups = near(
            vec![
                row(1, "aaaa", 0b0000, 60),
                row(2, "bbbb", 0b0011, 60),
                row(3, "cccc", 0b1111, 60),
            ],
            2,
        );

        let mut seen: Vec<i64> = groups
            .iter()
            .flat_map(|g| g.members.iter().map(|m| m.id.0))
            .collect();
        let total = seen.len();
        seen.sort_unstable();
        seen.dedup();
        assert_eq!(seen.len(), total, "a symbol landed in two groups");
    }

    #[test]
    fn containers_are_not_reportable() {
        // The rule that keeps a duplication report from being a list of test modules.
        assert!(is_reportable(SymbolKind::Function));
        assert!(is_reportable(SymbolKind::Method));
        assert!(!is_reportable(SymbolKind::Module));
        assert!(!is_reportable(SymbolKind::Class));
        assert!(!is_reportable(SymbolKind::Trait));
        assert!(!is_reportable(SymbolKind::Field));
    }

    #[test]
    fn groups_come_back_biggest_first() {
        let mut small = row(1, "aaaa", 0b1, 10);
        let mut small2 = row(2, "aaaa", 0b1, 10);
        small.end_line = 5;
        small2.end_line = 5;

        let mut big = row(3, "bbbb", 0b10, 200);
        let mut big2 = row(4, "bbbb", 0b10, 200);
        big.end_line = 120;
        big2.end_line = 120;

        let groups = exact(vec![small, small2, big, big2]);
        assert_eq!(groups.len(), 2);
        assert!(groups[0].lines() > groups[1].lines());
    }
}
