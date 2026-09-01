// Copyright (c) 2024-2025 Ihor
// SPDX-License-Identifier: BUSL-1.1
// See LICENSE file for details

//! What every analyzer in this directory does the same way.
//!
//! The language modules differ in the node kinds they match, and in how each language spells a
//! doc comment, a visibility keyword or a signature. Everything around that — reading a node's
//! text and span, turning a named declaration into a [`ParsedSymbol`], wiring an analyzer to the
//! two functions that walk its tree — is identical in all of them, and lives here.

use tree_sitter::Node;

use crate::types::{Location, ParsedSymbol, SymbolKind, Visibility};

/// The source text a node spans.
pub(super) fn node_text<'a>(node: Node<'a>, source: &'a str) -> String {
    source[node.byte_range()].to_string()
}

/// A node's span, in the shape the rest of the indexer stores.
pub(super) fn node_location(node: Node) -> Location {
    let start = node.start_position();
    let end = node.end_position();
    Location::new(
        start.row as u32,
        start.column as u32,
        end.row as u32,
        end.column as u32,
        node.start_byte() as u32,
        node.end_byte() as u32,
    )
}

/// A declaration's signature: the node's text cut where its body begins, trimmed.
///
/// `body_starts` are the tokens that open a body in this language, tried in order — a brace for a
/// definition, a semicolon for a bodiless declaration (a Rust trait method, a C++ prototype), a
/// fat arrow for a C# expression-bodied member. The first one that occurs anywhere in the text
/// wins, so the order is a priority, not a search for the earliest position.
///
/// Which tokens those are is the caller's to say rather than one shared set, because a token that
/// opens a body in one language is ordinary syntax in another: `=>` ends a C# member, but in PHP
/// it separates an array key from its value, so cutting a PHP signature at one would truncate
/// `function f($a = ['x' => 1])` mid-parameter.
///
/// A declaration carrying none of them — a Swift protocol requirement — falls back to its first
/// line.
pub(super) fn signature_before(node: Node, source: &str, body_starts: &[&str]) -> String {
    cut_before_body(&node_text(node, source), body_starts)
}

/// The rule itself, over the text alone — split out from [`signature_before`] so it can be tested
/// without standing up a parser and a grammar for every language that uses it.
fn cut_before_body(text: &str, body_starts: &[&str]) -> String {
    body_starts
        .iter()
        .find_map(|token| text.find(token))
        .map_or_else(
            || text.lines().next().unwrap_or("").to_string(),
            |cut| text[..cut].trim().to_string(),
        )
}

/// [`ParsedSymbol::with_doc_comment`] for the doc-comment extractors, which return `None` when
/// nothing precedes the declaration.
pub(super) trait WithDocCommentOpt {
    fn with_doc_comment_opt(self, doc: Option<String>) -> Self;
}

impl WithDocCommentOpt for ParsedSymbol {
    fn with_doc_comment_opt(self, doc: Option<String>) -> Self {
        match doc {
            Some(d) => self.with_doc_comment(d),
            None => self,
        }
    }
}

/// The symbol a declaration node names: the text of its `name` field, `kind`, and the node's own
/// span. `None` when the node carries no name — a broken parse, or a node kind matched by
/// mistake.
///
/// `doc`, `visibility` and `signature` are the three parts each language spells its own way, so
/// they arrive already extracted; a language with no notion of one passes `None` rather than a
/// stand-in, and the field is left at whatever [`ParsedSymbol::new`] defaults it to.
pub(super) fn declaration(
    node: Node,
    source: &str,
    kind: SymbolKind,
    doc: Option<String>,
    visibility: Option<Visibility>,
    signature: Option<String>,
) -> Option<ParsedSymbol> {
    let name = node_text(node.child_by_field_name("name")?, source);
    let mut symbol = ParsedSymbol::new(name, kind, node_location(node));

    if let Some(signature) = signature {
        symbol = symbol.with_signature(signature);
    }
    if let Some(visibility) = visibility {
        symbol = symbol.with_visibility(visibility);
    }

    Some(symbol.with_doc_comment_opt(doc))
}

/// Implement [`LanguageAnalyzer`](crate::parser::treesitter::analyzers::LanguageAnalyzer) for an
/// analyzer that does nothing but walk the tree twice, with one free function per pass.
///
/// That is every analyzer here except C++ — whose walkers speak its own intermediate types and
/// need converting on the way out — and Ruby, whose symbol walk threads the visibility a bare
/// `private` switched on for the rest of the body.
///
/// The body resolves against the calling module's imports, which is what lets each module keep
/// naming its own walkers.
macro_rules! tree_walking_analyzer {
    ($analyzer:ty, symbols: $symbols:ident, references: $references:ident $(,)?) => {
        impl LanguageAnalyzer for $analyzer {
            fn extract_symbols(&self, source: &str, tree: &Tree) -> Vec<ParsedSymbol> {
                let mut symbols = Vec::new();
                $symbols(tree.root_node(), source, &mut symbols);
                symbols
            }

            fn extract_references(&self, source: &str, tree: &Tree) -> Vec<ParsedReference> {
                let mut refs = Vec::new();
                $references(tree.root_node(), source, &mut refs);
                refs
            }
        }
    };
}

pub(super) use tree_walking_analyzer;

#[cfg(test)]
mod tests {
    use super::cut_before_body;

    #[test]
    fn the_body_is_cut_off_at_the_first_token_that_occurs() {
        assert_eq!(
            cut_before_body("fn f(x: u8) -> u8 {\n    x\n}", &["{", ";"]),
            "fn f(x: u8) -> u8"
        );
        // A bodiless declaration: the second token carries it, and the terminator goes too.
        assert_eq!(
            cut_before_body("fn f(&self) -> u8;", &["{", ";"]),
            "fn f(&self) -> u8"
        );
    }

    #[test]
    fn the_tokens_are_a_priority_not_a_race_for_the_earliest() {
        // The `;` of an array length sits before the `{`, but `{` is listed first and still wins.
        // Cutting at whichever token came earliest would truncate the parameter list instead.
        assert_eq!(
            cut_before_body("fn f(x: [u8; 4]) {\n}", &["{", ";"]),
            "fn f(x: [u8; 4])"
        );
        // And a token further down the list carries the cut when the ones before it don't occur.
        assert_eq!(cut_before_body("int P => f;", &["{", "=>"]), "int P");
    }

    #[test]
    fn a_token_a_language_never_lists_is_left_alone() {
        // The PHP case the per-language token lists exist for: `=>` inside a default array value
        // is ordinary syntax, and PHP does not list it.
        assert_eq!(
            cut_before_body("function f($a = ['x' => 1]) {\n}", &["{"]),
            "function f($a = ['x' => 1])"
        );
    }

    #[test]
    fn text_with_no_body_token_falls_back_to_its_first_line() {
        assert_eq!(
            cut_before_body("func f() -> Int\nnext line", &["{"]),
            "func f() -> Int"
        );
        assert_eq!(cut_before_body("", &["{"]), "");
    }
}
