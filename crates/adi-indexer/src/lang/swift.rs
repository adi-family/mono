//! Swift language analyzer implementation.

use tree_sitter::{Node, Tree};

use super::common::{
    WithDocCommentOpt, declaration, node_location, node_text, signature_before,
    tree_walking_analyzer,
};
use crate::parser::treesitter::analyzers::LanguageAnalyzer;
use crate::types::{ParsedReference, ParsedSymbol, ReferenceKind, SymbolKind, Visibility};

/// The grammar this module analyses.
#[must_use]
pub fn language() -> tree_sitter::Language {
    tree_sitter_swift::LANGUAGE.into()
}

#[derive(Debug)]
pub struct SwiftAnalyzer;

tree_walking_analyzer!(
    SwiftAnalyzer,
    symbols: extract_swift_symbols,
    references: collect_swift_references,
);

fn extract_doc_comment(node: Node, source: &str) -> Option<String> {
    let mut prev = node.prev_sibling();
    let mut comments = Vec::new();

    while let Some(sibling) = prev {
        match sibling.kind() {
            "comment" | "multiline_comment" => {
                let text = node_text(sibling, source);
                if text.starts_with("///") || text.starts_with("/**") {
                    comments.push(
                        text.trim_start_matches("///")
                            .trim_start_matches("/**")
                            .trim_end_matches("*/")
                            .lines()
                            .map(|l| l.trim().trim_start_matches('*').trim())
                            .filter(|l| !l.is_empty())
                            .collect::<Vec<_>>()
                            .join("\n"),
                    );
                } else {
                    break;
                }
            }
            "attribute" => {}
            _ => break,
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

fn extract_visibility(node: Node, source: &str) -> Visibility {
    for i in 0..node.child_count() as u32 {
        if let Some(child) = node.child(i)
            && child.kind() == "modifiers"
        {
            for j in 0..child.child_count() as u32 {
                if let Some(modifier) = child.child(j) {
                    let text = node_text(modifier, source);
                    match text.as_str() {
                        "public" => return Visibility::Public,
                        "private" => return Visibility::Private,
                        "fileprivate" => return Visibility::Private,
                        "internal" => return Visibility::Internal,
                        "open" => return Visibility::Public,
                        _ => {}
                    }
                }
            }
        }
    }
    Visibility::Internal
}

/// Only a brace — Swift ends no declaration in a semicolon, so a protocol requirement is already
/// nothing but its first line.
fn extract_function_signature(node: Node, source: &str) -> String {
    signature_before(node, source, &["{"])
}

fn extract_swift_symbols(node: Node, source: &str, symbols: &mut Vec<ParsedSymbol>) {
    let parsed = match node.kind() {
        "class_declaration" => parse_swift_declaration(node, source, SymbolKind::Class),
        "struct_declaration" => parse_swift_declaration(node, source, SymbolKind::Struct),
        "protocol_declaration" => parse_swift_declaration(node, source, SymbolKind::Interface),
        "enum_declaration" => parse_swift_declaration(node, source, SymbolKind::Enum),
        "typealias_declaration" => parse_swift_declaration(node, source, SymbolKind::Type),
        "function_declaration" => parse_swift_callable(node, source, SymbolKind::Function),
        "property_declaration" => parse_swift_property(node, source),
        "init_declaration" => Some(parse_swift_init(node, source)),
        "deinit_declaration" => Some(parse_swift_deinit(node)),
        "extension_declaration" => parse_swift_extension(node, source),
        _ => None,
    };
    symbols.extend(parsed);

    for i in 0..node.child_count() as u32 {
        if let Some(child) = node.child(i) {
            extract_swift_symbols(child, source, symbols);
        }
    }
}

/// A Swift type or typealias: named by its `name` field, preceded by a `///` block and a
/// visibility keyword.
fn parse_swift_declaration(node: Node, source: &str, kind: SymbolKind) -> Option<ParsedSymbol> {
    declaration(
        node,
        source,
        kind,
        extract_doc_comment(node, source),
        Some(extract_visibility(node, source)),
        None,
    )
}

/// A Swift function — the same, plus the signature its body is stripped down to.
fn parse_swift_callable(node: Node, source: &str, kind: SymbolKind) -> Option<ParsedSymbol> {
    declaration(
        node,
        source,
        kind,
        extract_doc_comment(node, source),
        Some(extract_visibility(node, source)),
        Some(extract_function_signature(node, source)),
    )
}

fn parse_swift_property(node: Node, source: &str) -> Option<ParsedSymbol> {
    for i in 0..node.child_count() as u32 {
        if let Some(child) = node.child(i)
            && child.kind() == "pattern"
        {
            let name_text = node_text(child, source);
            let doc_comment = extract_doc_comment(node, source);
            let visibility = extract_visibility(node, source);

            return Some(
                ParsedSymbol::new(name_text, SymbolKind::Property, node_location(node))
                    .with_visibility(visibility)
                    .with_doc_comment_opt(doc_comment),
            );
        }
    }
    None
}

/// `init` and `deinit` name themselves, so neither goes through
/// [`declaration`] — there is no `name` field to read.
fn parse_swift_init(node: Node, source: &str) -> ParsedSymbol {
    ParsedSymbol::new(
        "init".to_string(),
        SymbolKind::Constructor,
        node_location(node),
    )
    .with_signature(extract_function_signature(node, source))
    .with_visibility(extract_visibility(node, source))
    .with_doc_comment_opt(extract_doc_comment(node, source))
}

fn parse_swift_deinit(node: Node) -> ParsedSymbol {
    ParsedSymbol::new(
        "deinit".to_string(),
        SymbolKind::Destructor,
        node_location(node),
    )
    .with_visibility(Visibility::Internal)
}

fn parse_swift_extension(node: Node, source: &str) -> Option<ParsedSymbol> {
    for i in 0..node.child_count() as u32 {
        if let Some(child) = node.child(i)
            && (child.kind() == "user_type" || child.kind() == "type_identifier")
        {
            let name_text = format!("extension {}", node_text(child, source));
            return Some(ParsedSymbol::new(
                name_text,
                SymbolKind::Class,
                node_location(node),
            ));
        }
    }
    None
}

fn collect_swift_references(node: Node, source: &str, refs: &mut Vec<ParsedReference>) {
    match node.kind() {
        "call_expression" => {
            if let Some(func) = node.child(0) {
                let name = extract_call_name(func, source);
                if !name.is_empty() && !is_common_function(&name) {
                    refs.push(ParsedReference::new(
                        name,
                        ReferenceKind::Call,
                        node_location(func),
                    ));
                }
            }
        }
        "navigation_expression" => {
            if let Some(suffix) = node.child_by_field_name("suffix") {
                refs.push(ParsedReference::new(
                    node_text(suffix, source),
                    ReferenceKind::FieldAccess,
                    node_location(suffix),
                ));
            }
        }
        "user_type" | "type_identifier" => {
            let name = node_text(node, source);
            if !is_primitive_type(&name) {
                refs.push(ParsedReference::new(
                    name,
                    ReferenceKind::TypeReference,
                    node_location(node),
                ));
            }
        }
        "import_declaration" => {
            for i in 0..node.child_count() as u32 {
                if let Some(child) = node.child(i)
                    && child.kind() == "identifier"
                {
                    refs.push(ParsedReference::new(
                        node_text(child, source),
                        ReferenceKind::Import,
                        node_location(child),
                    ));
                }
            }
        }
        "inheritance_specifier" => {
            for i in 0..node.child_count() as u32 {
                if let Some(child) = node.child(i)
                    && (child.kind() == "user_type" || child.kind() == "type_identifier")
                {
                    refs.push(ParsedReference::new(
                        node_text(child, source),
                        ReferenceKind::Inheritance,
                        node_location(child),
                    ));
                }
            }
        }
        _ => {}
    }

    for i in 0..node.child_count() as u32 {
        if let Some(child) = node.child(i) {
            collect_swift_references(child, source, refs);
        }
    }
}

fn extract_call_name(node: Node, source: &str) -> String {
    match node.kind() {
        "simple_identifier" => node_text(node, source),
        "navigation_expression" => {
            if let Some(suffix) = node.child_by_field_name("suffix") {
                node_text(suffix, source)
            } else {
                String::new()
            }
        }
        _ => node_text(node, source),
    }
}

fn is_primitive_type(name: &str) -> bool {
    matches!(
        name,
        "Int"
            | "Int8"
            | "Int16"
            | "Int32"
            | "Int64"
            | "UInt"
            | "UInt8"
            | "UInt16"
            | "UInt32"
            | "UInt64"
            | "Float"
            | "Double"
            | "Bool"
            | "String"
            | "Character"
            | "Void"
            | "Never"
            | "Any"
            | "AnyObject"
            | "Self"
            | "Optional"
            | "Array"
            | "Dictionary"
            | "Set"
    )
}

fn is_common_function(name: &str) -> bool {
    matches!(
        name,
        "print"
            | "debugPrint"
            | "dump"
            | "fatalError"
            | "precondition"
            | "preconditionFailure"
            | "assert"
            | "assertionFailure"
    )
}
