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
