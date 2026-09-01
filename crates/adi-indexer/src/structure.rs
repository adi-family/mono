// Copyright (c) 2024-2025 Ihor
// SPDX-License-Identifier: BUSL-1.1
// See LICENSE file for details

//! Structural fingerprints: what a symbol's syntax looks like once the names are taken out.
//!
//! The parse tree already knows everything needed to recognise copy-paste. Walking a symbol's
//! subtree and keeping only the *kinds* of its named nodes discards every identifier and every
//! literal value along the way — `fn a(x: u32) { g(1) }` and `fn b(y: u64) { h(7) }` both reduce
//! to the same sequence — so a function that was copied and renamed fingerprints identically to
//! its original. That is Type-2 clone detection, and it falls out of the tree for free.
//!
//! Two numbers come off that sequence, answering two different questions:
//!
//! * [`Structure::hash`] — *the same shape, or not.* Exact, and cheap to `GROUP BY` in SQL.
//! * [`Structure::simhash`] — *how close two shapes are.* A copy that drifted (a line added, a
//!   branch dropped) keeps most of its k-grams, so its simhash stays within a few bits of the
//!   original's; [`hamming`] counts them.
//!
//! [`Structure::node_count`] is the third field because neither number means much alone: a
//! three-node getter has thousands of exact twins in any codebase, and reporting them is noise.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tree_sitter::Node;

/// Node kinds per shingle. Three is small enough that a short function still yields several
/// grams, and large enough that the order of the kinds inside one is what distinguishes it.
const SHINGLE: usize = 3;

/// The fingerprint of one symbol's syntax.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Structure {
    /// Hex of a SHA-256 over the node-kind sequence, truncated to 64 bits. Equal hashes mean
    /// identical shape — at 64 bits, a false pair needs on the order of a billion symbols.
    pub hash: String,
    /// Simhash over the sequence's k-grams. Near-identical shapes differ in only a few bits.
    pub simhash: u64,
    /// Named nodes in the subtree. Every query filters on this; see the module docs.
    pub node_count: u32,
}

/// Fingerprint the subtree rooted at `node`.
#[must_use]
pub fn fingerprint(node: Node) -> Structure {
    let kinds = kind_sequence(node);
    Structure {
        hash: hash_kinds(&kinds),
        simhash: simhash(&kinds),
        node_count: kinds.len() as u32,
    }
}

/// Bits that differ between two simhashes — 0 for identical shapes, ~32 for unrelated ones.
#[must_use]
pub fn hamming(a: u64, b: u64) -> u32 {
    (a ^ b).count_ones()
}

/// The node that a symbol's `[start, end)` byte range names.
///
/// Analyzers record a symbol's span from the node they found it on, so the range round-trips
/// back to that node — but only after two corrections: `descendant_for_byte_range` wants an
/// inclusive end, and it answers with the *innermost* node covering the range, while the symbol
/// is the outermost one still inside it.
#[must_use]
pub fn node_for_range(root: Node<'_>, start: u32, end: u32) -> Option<Node<'_>> {
    let (start, end) = (start as usize, end as usize);
    if end <= start {
        return None;
    }

    let mut node = root.descendant_for_byte_range(start, end - 1)?;
    while let Some(parent) = node.parent() {
        if parent.start_byte() >= start && parent.end_byte() <= end {
            node = parent;
        } else {
            break;
        }
    }
    Some(node)
}

/// Named node kinds in pre-order, comments left out.
///
/// Iterative rather than recursive: this runs over every symbol of every file, and a generated
/// source file can nest deeply enough to matter.
fn kind_sequence(root: Node) -> Vec<&'static str> {
    let mut kinds = Vec::new();
    let mut stack = vec![root];
    let mut children = Vec::new();

    while let Some(node) = stack.pop() {
        let kind = node.kind();
        // A comment is not structure — the same code with and without one is the same shape.
        if kind.contains("comment") {
            continue;
        }
        kinds.push(kind);

        children.clear();
        let mut cursor = node.walk();
        children.extend(node.named_children(&mut cursor));
        // Popped last-in-first-out, so push reversed to keep the walk left to right.
        stack.extend(children.iter().rev().copied());
    }

    kinds
}

fn hash_kinds(kinds: &[&str]) -> String {
    let mut hasher = Sha256::new();
    for kind in kinds {
        hasher.update(kind.as_bytes());
        // Separate the kinds, so ["ab", "c"] and ["a", "bc"] cannot collide.
        hasher.update([0u8]);
    }
    hex::encode(&hasher.finalize()[..8])
}

fn simhash(kinds: &[&str]) -> u64 {
    if kinds.is_empty() {
        return 0;
    }

    // A symbol shorter than one shingle still gets a fingerprint — of its whole sequence.
    let window = SHINGLE.min(kinds.len());
    let mut columns = [0i32; 64];
    for gram in kinds.windows(window) {
        let gram_hash = fnv1a(gram);
        for (bit, column) in columns.iter_mut().enumerate() {
            if (gram_hash >> bit) & 1 == 1 {
                *column += 1;
            } else {
                *column -= 1;
            }
        }
    }

    let mut out = 0u64;
    for (bit, column) in columns.iter().enumerate() {
        if *column > 0 {
            out |= 1u64 << bit;
        }
    }
    out
}

/// FNV-1a, written out rather than taken from `DefaultHasher`.
///
/// These values are stored in the index and compared against ones written months earlier, and
/// `DefaultHasher`'s algorithm is explicitly allowed to change between Rust releases — which
/// would silently reshuffle every simhash on disk without invalidating a single one of them.
fn fnv1a(gram: &[&str]) -> u64 {
    const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;

    let mut hash = OFFSET;
    for kind in gram {
        for byte in kind.as_bytes() {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(PRIME);
        }
        // Terminate each kind, for the same reason `hash_kinds` separates them.
        hash ^= 0xff;
        hash = hash.wrapping_mul(PRIME);
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What the fingerprint claims, checked against a real parse rather than a hand-written
    /// kind sequence.
    #[cfg(feature = "lang-rust")]
    mod over_real_source {
        use crate::parser::{Parser as _, TreeSitterParser};
        use crate::structure::hamming;
        use crate::types::{Language, ParsedSymbol};

        fn symbols(source: &str) -> Vec<ParsedSymbol> {
            TreeSitterParser::new()
                .parse(source, Language::Rust)
                .expect("rust source parses")
                .symbols
        }

        fn only(source: &str) -> ParsedSymbol {
            let mut parsed = symbols(source);
            assert_eq!(parsed.len(), 1, "expected one symbol in {source:?}");
            parsed.remove(0)
        }

        #[test]
        fn a_renamed_copy_fingerprints_identically() {
            let original = only(
                "fn collect(items: &[u32]) -> u32 { let mut total = 0; for i in items { total += i; } total }",
            );
            let renamed = only(
                "fn tally(values: &[u64]) -> u64 { let mut sum = 0; for v in values { sum += v; } sum }",
            );

            let (a, b) = (
                original.structure.expect("fingerprinted"),
                renamed.structure.expect("fingerprinted"),
            );
            assert_eq!(a.hash, b.hash, "renaming changed the shape");
            assert_eq!(a.simhash, b.simhash);
        }

        #[test]
        fn different_code_fingerprints_differently() {
            let loop_fn =
                only("fn a(xs: &[u32]) -> u32 { let mut t = 0; for x in xs { t += x; } t }");
            let match_fn =
                only("fn b(x: Option<u32>) -> u32 { match x { Some(v) => v, None => 0 } }");

            assert_ne!(
                loop_fn.structure.expect("fingerprinted").hash,
                match_fn.structure.expect("fingerprinted").hash
            );
        }

        #[test]
        fn a_comment_is_not_structure() {
            let bare = only("fn a() { let x = 1; }");
            let commented = only("fn a() { // why\n let x = 1; }");

            assert_eq!(
                bare.structure.expect("fingerprinted").hash,
                commented.structure.expect("fingerprinted").hash
            );
        }

        #[test]
        fn a_drifted_copy_stays_near_but_not_equal() {
            // Function-sized, because that is what `min_nodes` holds reports to and what the
            // distance defaults are tuned against — a four-line function's fingerprint moves
            // far on any edit at all, which `a_short_sequence_is_why_reports_have_a_size_floor`
            // pins separately.
            let original = only(
                "fn total(rows: &[Row]) -> u32 {
                    let mut sum = 0;
                    for row in rows {
                        if row.enabled {
                            sum += row.weight * row.count;
                        } else {
                            sum += row.weight;
                        }
                    }
                    if sum > 100 { sum = 100; }
                    sum
                }",
            );
            let drifted = only(
                "fn tally(items: &[Item]) -> u64 {
                    let mut acc = 0;
                    for item in items {
                        if item.active {
                            acc += item.factor * item.qty;
                        } else {
                            acc += item.factor;
                        }
                    }
                    if acc > 200 { acc = 200; }
                    let capped = acc;
                    capped
                }",
            );
            let unrelated = only(
                "fn describe(kind: Kind) -> String {
                    match kind {
                        Kind::One => String::from(\"one\"),
                        Kind::Two => String::from(\"two\"),
                        Kind::Three => String::from(\"three\"),
                        _ => String::new(),
                    }
                }",
            );

            let (a, b, c) = (
                original.structure.expect("fingerprinted"),
                drifted.structure.expect("fingerprinted"),
                unrelated.structure.expect("fingerprinted"),
            );

            assert_ne!(a.hash, b.hash, "the copy did drift");

            let near = hamming(a.simhash, b.simhash);
            let far = hamming(a.simhash, c.simhash);
            assert!(near < far, "drifted copy {near} vs unrelated {far}");
            assert!(near <= 8, "a drifted copy sat {near} bits away");
        }

        #[test]
        fn the_fingerprint_covers_the_symbol_and_not_the_file() {
            // Two functions in one file must fingerprint separately — if `node_for_range`
            // climbed to the root, both would come back identical and every symbol in a file
            // would look like a clone of every other.
            let parsed = symbols("fn a() { let x = 1; }\nfn b() { for i in 0..3 { drop(i); } }");
            assert_eq!(parsed.len(), 2);
            assert_ne!(
                parsed[0].structure.as_ref().expect("fingerprinted").hash,
                parsed[1].structure.as_ref().expect("fingerprinted").hash
            );
        }

        #[test]
        fn a_method_is_fingerprinted_apart_from_its_type() {
            // Rust's analyzer reports methods at the top level as `S::one` rather than as
            // children of the impl — so this checks the flat case, and the nested case lives
            // in `nested_symbols_are_fingerprinted_too` against a language that nests.
            let parsed = symbols("struct S; impl S { fn one(&self) -> u32 { 1 } }");
            let method = parsed
                .iter()
                .find(|s| s.name.contains("one"))
                .expect("the method was indexed");
            let type_symbol = parsed
                .iter()
                .find(|s| s.name == "S")
                .expect("the struct was indexed");

            assert_ne!(
                method.structure.as_ref().expect("fingerprinted").hash,
                type_symbol.structure.as_ref().expect("fingerprinted").hash
            );
        }
    }

    /// Languages whose analyzers nest symbols — that the fingerprint recurses into children is
    /// only observable here.
    #[cfg(feature = "lang-python")]
    #[test]
    fn nested_symbols_are_fingerprinted_too() {
        use crate::parser::{Parser as _, TreeSitterParser};
        use crate::types::Language;

        let parsed = TreeSitterParser::new()
            .parse(
                "class C:\n    def m(self):\n        return 1\n",
                Language::Python,
            )
            .expect("python source parses");

        let class = parsed.symbols.first().expect("the class was indexed");
        let method = class.children.first().expect("the method was indexed");

        assert!(class.structure.is_some(), "the class was not fingerprinted");
        let method_structure = method.structure.as_ref().expect("child not fingerprinted");
        assert_ne!(
            method_structure.hash,
            class.structure.as_ref().unwrap().hash,
            "the method took its enclosing class's shape"
        );
    }

    #[test]
    fn hamming_counts_differing_bits() {
        assert_eq!(hamming(0b1011, 0b1011), 0);
        assert_eq!(hamming(0b1011, 0b1010), 1);
        assert_eq!(hamming(0, u64::MAX), 64);
    }

    #[test]
    fn the_hash_separates_kinds() {
        // Without a separator these two sequences would hash the same.
        assert_ne!(hash_kinds(&["ab", "c"]), hash_kinds(&["a", "bc"]));
    }

    #[test]
    fn the_hash_is_order_sensitive() {
        assert_ne!(hash_kinds(&["if", "call"]), hash_kinds(&["call", "if"]));
    }

    #[test]
    fn an_empty_sequence_fingerprints_without_panicking() {
        assert_eq!(simhash(&[]), 0);
        assert_eq!(hash_kinds(&[]), hash_kinds(&[]));
    }

    #[test]
    fn a_sequence_shorter_than_one_shingle_still_hashes() {
        // `windows(3)` on a 2-element slice yields nothing, which would leave every short
        // symbol at simhash 0 and so "identical" to every other short symbol.
        assert_ne!(simhash(&["function_item", "block"]), 0);
        assert_ne!(
            simhash(&["function_item", "block"]),
            simhash(&["struct_item", "field"])
        );
    }

    /// A function-sized sequence — the scale `min_nodes` exists to hold reports to.
    fn realistic_sequence() -> Vec<&'static str> {
        let body = [
            "let_declaration",
            "identifier",
            "call_expression",
            "field_expression",
            "arguments",
            "if_expression",
            "binary_expression",
            "identifier",
            "integer_literal",
            "block",
            "return_expression",
        ];
        let mut kinds = vec!["function_item", "identifier", "parameters", "block"];
        for _ in 0..6 {
            kinds.extend_from_slice(&body);
        }
        kinds
    }

    #[test]
    fn near_identical_sequences_stay_close() {
        let original = realistic_sequence();

        let mut drifted = original.clone();
        drifted.insert(20, "let_declaration");

        let unrelated: Vec<&str> = std::iter::repeat_n(
            [
                "struct_item",
                "field_declaration_list",
                "field_declaration",
                "type_identifier",
            ],
            18,
        )
        .flatten()
        .collect();

        let near = hamming(simhash(&original), simhash(&drifted));
        let far = hamming(simhash(&original), simhash(&unrelated));

        assert!(near < far, "near {near} should beat far {far}");
        assert!(
            near <= 6,
            "one node inserted into a {}-node symbol moved {near} bits",
            original.len()
        );
    }

    #[test]
    fn a_short_sequence_is_why_reports_have_a_size_floor() {
        // The same single insertion that barely moves a function-sized fingerprint dominates a
        // tiny one: with only a handful of shingles, one of them changing is a large fraction
        // of the signal. This is what `min_nodes` guards against, so pin the behaviour rather
        // than let a report quietly fill up with noise from three-node accessors.
        let short = vec!["function_item", "identifier", "block", "return_expression"];
        let mut drifted = short.clone();
        drifted.insert(2, "let_declaration");

        let long = realistic_sequence();
        let mut long_drifted = long.clone();
        long_drifted.insert(2, "let_declaration");

        let short_shift = hamming(simhash(&short), simhash(&drifted));
        let long_shift = hamming(simhash(&long), simhash(&long_drifted));
        assert!(
            short_shift > long_shift,
            "short moved {short_shift}, long moved {long_shift}"
        );
    }
}
