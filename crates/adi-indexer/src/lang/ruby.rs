//! Ruby language analyzer implementation.

use tree_sitter::{Node, Tree};

use crate::parser::treesitter::analyzers::LanguageAnalyzer;
use crate::types::{Location, ParsedReference, ParsedSymbol, ReferenceKind, SymbolKind, Visibility};

/// The grammar this module analyses.
#[must_use]
pub fn language() -> tree_sitter::Language {
    tree_sitter_ruby::LANGUAGE.into()
}

#[derive(Debug)]
pub struct RubyAnalyzer;

impl LanguageAnalyzer for RubyAnalyzer {
    fn extract_symbols(&self, source: &str, tree: &Tree) -> Vec<ParsedSymbol> {
        let mut symbols = Vec::new();
        let mut visibility = Visibility::Public;
        extract_ruby_symbols(tree.root_node(), source, &mut symbols, &mut visibility);
        symbols
    }

    fn extract_references(&self, source: &str, tree: &Tree) -> Vec<ParsedReference> {
        let mut refs = Vec::new();
        collect_ruby_references(tree.root_node(), source, &mut refs);
        refs
    }
}

fn node_text<'a>(node: Node<'a>, source: &'a str) -> String {
    source[node.byte_range()].to_string()
}

fn node_location(node: Node) -> Location {
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

fn extract_doc_comment(node: Node, source: &str) -> Option<String> {
    let mut prev = node.prev_sibling();
    let mut comments = Vec::new();

    while let Some(sibling) = prev {
        if sibling.kind() == "comment" {
            let text = node_text(sibling, source);
            comments.push(text.trim_start_matches('#').trim().to_string());
        } else {
            break;
        }
        prev = sibling.prev_sibling();
    }

    if comments.is_empty() {
        None
    } else {
        comments.reverse();
        Some(comments.join("\n"))
    }
}

fn extract_method_signature(node: Node, source: &str) -> String {
    let text = node_text(node, source);
    if let Some(newline_pos) = text.find('\n') {
        text[..newline_pos].trim().to_string()
    } else {
        text.trim().to_string()
    }
}

fn extract_ruby_symbols(
    node: Node,
    source: &str,
    symbols: &mut Vec<ParsedSymbol>,
    current_visibility: &mut Visibility,
) {
    match node.kind() {
        "class" => {
            if let Some(symbol) = parse_ruby_class(node, source) {
                symbols.push(symbol);
            }
            let mut class_visibility = Visibility::Public;
            for i in 0..node.child_count() {
                if let Some(child) = node.child(i) {
                    extract_ruby_symbols(child, source, symbols, &mut class_visibility);
                }
            }
            return;
        }
        "module" => {
            if let Some(symbol) = parse_ruby_module(node, source) {
                symbols.push(symbol);
            }
            let mut mod_visibility = Visibility::Public;
            for i in 0..node.child_count() {
                if let Some(child) = node.child(i) {
                    extract_ruby_symbols(child, source, symbols, &mut mod_visibility);
                }
            }
            return;
        }
        "method" => {
            if let Some(symbol) = parse_ruby_method(node, source, *current_visibility) {
                symbols.push(symbol);
            }
        }
        "singleton_method" => {
            if let Some(symbol) = parse_ruby_singleton_method(node, source) {
                symbols.push(symbol);
            }
        }
        "identifier" => {
            let text = node_text(node, source);
            match text.as_str() {
                "private" => *current_visibility = Visibility::Private,
                "protected" => *current_visibility = Visibility::Protected,
                "public" => *current_visibility = Visibility::Public,
                _ => {}
            }
        }
        "assignment" => {
            if let Some(symbol) = parse_ruby_constant(node, source) {
                symbols.push(symbol);
            }
        }
        _ => {}
    }

    for i in 0..node.child_count() {
        if let Some(child) = node.child(i) {
            extract_ruby_symbols(child, source, symbols, current_visibility);
        }
    }
}

fn parse_ruby_class(node: Node, source: &str) -> Option<ParsedSymbol> {
    let name = node.child_by_field_name("name")?;
    let name_text = node_text(name, source);
    let doc_comment = extract_doc_comment(node, source);

    Some(
        ParsedSymbol::new(name_text, SymbolKind::Class, node_location(node))
            .with_visibility(Visibility::Public)
            .with_doc_comment_opt(doc_comment),
    )
}

fn parse_ruby_module(node: Node, source: &str) -> Option<ParsedSymbol> {
    let name = node.child_by_field_name("name")?;
    let name_text = node_text(name, source);
    let doc_comment = extract_doc_comment(node, source);

    Some(
        ParsedSymbol::new(name_text, SymbolKind::Module, node_location(node))
            .with_visibility(Visibility::Public)
            .with_doc_comment_opt(doc_comment),
    )
}

fn parse_ruby_method(node: Node, source: &str, visibility: Visibility) -> Option<ParsedSymbol> {
    let name = node.child_by_field_name("name")?;
    let name_text = node_text(name, source);
    let doc_comment = extract_doc_comment(node, source);
    let signature = extract_method_signature(node, source);

    Some(
        ParsedSymbol::new(name_text, SymbolKind::Method, node_location(node))
            .with_signature(signature)
            .with_visibility(visibility)
            .with_doc_comment_opt(doc_comment),
    )
}

fn parse_ruby_singleton_method(node: Node, source: &str) -> Option<ParsedSymbol> {
    let name = node.child_by_field_name("name")?;
    let name_text = node_text(name, source);
    let doc_comment = extract_doc_comment(node, source);
    let signature = extract_method_signature(node, source);

    Some(
        ParsedSymbol::new(
            format!("self.{name_text}"),
            SymbolKind::Method,
            node_location(node),
        )
        .with_signature(signature)
        .with_visibility(Visibility::Public)
        .with_doc_comment_opt(doc_comment),
    )
}

fn parse_ruby_constant(node: Node, source: &str) -> Option<ParsedSymbol> {
    let left = node.child_by_field_name("left")?;
    if left.kind() == "constant" {
        let name_text = node_text(left, source);
        let doc_comment = extract_doc_comment(node, source);

        return Some(
            ParsedSymbol::new(name_text, SymbolKind::Constant, node_location(node))
                .with_visibility(Visibility::Public)
                .with_doc_comment_opt(doc_comment),
        );
    }
    None
}

fn collect_ruby_references(node: Node, source: &str, refs: &mut Vec<ParsedReference>) {
    match node.kind() {
        "call" | "method_call" => {
            if let Some(method) = node.child_by_field_name("method") {
                let name = node_text(method, source);
                // Handle require/require_relative as imports
                if name == "require" || name == "require_relative" {
                    if let Some(arg) = node.child_by_field_name("arguments") {
                        refs.push(ParsedReference::new(
                            node_text(arg, source),
                            ReferenceKind::Import,
                            node_location(arg),
                        ));
                    }
                } else if !is_common_method(&name) {
                    refs.push(ParsedReference::new(
                        name,
                        ReferenceKind::Call,
                        node_location(method),
                    ));
                }
            }
        }
        "constant" => {
            let name = node_text(node, source);
            let parent = node.parent();
            if let Some(p) = parent
                && p.kind() != "class" && p.kind() != "module" {
                    refs.push(ParsedReference::new(
                        name,
                        ReferenceKind::TypeReference,
                        node_location(node),
                    ));
                }
        }
        "scope_resolution" => {
            let name = node_text(node, source);
            refs.push(ParsedReference::new(
                name,
                ReferenceKind::TypeReference,
                node_location(node),
            ));
        }
        "superclass" => {
            let name = node_text(node, source);
            refs.push(ParsedReference::new(
                name,
                ReferenceKind::Inheritance,
                node_location(node),
            ));
        }
        _ => {}
    }

    for i in 0..node.child_count() {
        if let Some(child) = node.child(i) {
            collect_ruby_references(child, source, refs);
        }
    }
}

fn is_common_method(name: &str) -> bool {
    matches!(
        name,
        "new"
            | "initialize"
            | "to_s"
            | "to_i"
            | "to_a"
            | "to_h"
            | "inspect"
            | "class"
            | "is_a?"
            | "kind_of?"
            | "instance_of?"
            | "respond_to?"
            | "send"
            | "puts"
            | "print"
            | "p"
            | "raise"
            | "fail"
            | "attr_reader"
            | "attr_writer"
            | "attr_accessor"
    )
}

trait WithDocCommentOpt {
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
