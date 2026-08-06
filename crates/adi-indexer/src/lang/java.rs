//! Java language analyzer implementation.

use tree_sitter::{Node, Tree};

use super::common::{
    declaration, node_location, node_text, tree_walking_analyzer, WithDocCommentOpt,
};
use crate::parser::treesitter::analyzers::LanguageAnalyzer;
use crate::types::{ParsedReference, ParsedSymbol, ReferenceKind, SymbolKind, Visibility};

/// The grammar this module analyses.
#[must_use]
pub fn language() -> tree_sitter::Language {
    tree_sitter_java::LANGUAGE.into()
}

#[derive(Debug)]
pub struct JavaAnalyzer;

tree_walking_analyzer!(
    JavaAnalyzer,
    symbols: extract_java_symbols,
    references: collect_java_references,
);

fn extract_doc_comment(node: Node, source: &str) -> Option<String> {
    let mut prev = node.prev_sibling();
    while let Some(sibling) = prev {
        match sibling.kind() {
            "block_comment" => {
                let text = node_text(sibling, source);
                if text.starts_with("/**") {
                    return Some(
                        text.trim_start_matches("/**")
                            .trim_end_matches("*/")
                            .lines()
                            .map(|l| l.trim().trim_start_matches('*').trim())
                            .filter(|l| !l.is_empty())
                            .collect::<Vec<_>>()
                            .join("\n"),
                    );
                }
            }
            "line_comment" => {}
            "modifiers" | "marker_annotation" | "annotation" => {}
            _ => break,
        }
        prev = sibling.prev_sibling();
    }
    None
}

fn extract_visibility(node: Node, source: &str) -> Visibility {
    if let Some(modifiers) = node.child_by_field_name("modifiers") {
        for i in 0..modifiers.child_count() {
            if let Some(child) = modifiers.child(i) {
                let text = node_text(child, source);
                match text.as_str() {
                    "public" => return Visibility::Public,
                    "private" => return Visibility::Private,
                    "protected" => return Visibility::Protected,
                    _ => {}
                }
            }
        }
    }
    Visibility::Internal // package-private
}

fn extract_method_signature(node: Node, source: &str) -> String {
    let text = node_text(node, source);
    if let Some(brace_pos) = text.find('{') {
        text[..brace_pos].trim().to_string()
    } else {
        text.lines().next().unwrap_or("").to_string()
    }
}

fn extract_java_symbols(node: Node, source: &str, symbols: &mut Vec<ParsedSymbol>) {
    let parsed = match node.kind() {
        "class_declaration" => parse_java_declaration(node, source, SymbolKind::Class),
        "interface_declaration" => parse_java_declaration(node, source, SymbolKind::Interface),
        "enum_declaration" => parse_java_declaration(node, source, SymbolKind::Enum),
        "method_declaration" => parse_java_callable(node, source, SymbolKind::Method),
        "constructor_declaration" => parse_java_callable(node, source, SymbolKind::Constructor),
        "field_declaration" => {
            parse_java_declarators(node, source, symbols, SymbolKind::Field);
            None
        }
        "constant_declaration" => {
            parse_java_declarators(node, source, symbols, SymbolKind::Constant);
            None
        }
        _ => None,
    };
    symbols.extend(parsed);

    for i in 0..node.child_count() {
        if let Some(child) = node.child(i) {
            extract_java_symbols(child, source, symbols);
        }
    }
}

/// A Java type: named by its `name` field, preceded by a `/** … */` block and modifiers.
fn parse_java_declaration(node: Node, source: &str, kind: SymbolKind) -> Option<ParsedSymbol> {
    declaration(
        node,
        source,
        kind,
        extract_doc_comment(node, source),
        Some(extract_visibility(node, source)),
        None,
    )
}

/// A Java method or constructor — the same, plus the signature its body is stripped down to.
fn parse_java_callable(node: Node, source: &str, kind: SymbolKind) -> Option<ParsedSymbol> {
    declaration(
        node,
        source,
        kind,
        extract_doc_comment(node, source),
        Some(extract_visibility(node, source)),
        Some(extract_method_signature(node, source)),
    )
}

/// The declarators of a `field_declaration` or a `constant_declaration` — one statement can name
/// several, and they share the modifiers and doc comment written once in front.
fn parse_java_declarators(
    node: Node,
    source: &str,
    symbols: &mut Vec<ParsedSymbol>,
    kind: SymbolKind,
) {
    let visibility = extract_visibility(node, source);
    let doc_comment = extract_doc_comment(node, source);

    for i in 0..node.child_count() {
        if let Some(child) = node.child(i)
            && child.kind() == "variable_declarator"
                && let Some(name) = child.child_by_field_name("name") {
                    symbols.push(
                        ParsedSymbol::new(node_text(name, source), kind, node_location(child))
                            .with_visibility(visibility)
                            .with_doc_comment_opt(doc_comment.clone()),
                    );
                }
    }
}

fn collect_java_references(node: Node, source: &str, refs: &mut Vec<ParsedReference>) {
    match node.kind() {
        "method_invocation" => {
            if let Some(name) = node.child_by_field_name("name") {
                let name_text = node_text(name, source);
                if !is_common_method(&name_text) {
                    refs.push(ParsedReference::new(
                        name_text,
                        ReferenceKind::Call,
                        node_location(name),
                    ));
                }
            }
        }
        "object_creation_expression" => {
            if let Some(type_node) = node.child_by_field_name("type") {
                refs.push(ParsedReference::new(
                    node_text(type_node, source),
                    ReferenceKind::Call,
                    node_location(type_node),
                ));
            }
        }
        "type_identifier" => {
            let name = node_text(node, source);
            if !is_primitive_type(&name) {
                refs.push(ParsedReference::new(
                    name,
                    ReferenceKind::TypeReference,
                    node_location(node),
                ));
            }
        }
        "field_access" => {
            if let Some(field) = node.child_by_field_name("field") {
                refs.push(ParsedReference::new(
                    node_text(field, source),
                    ReferenceKind::FieldAccess,
                    node_location(field),
                ));
            }
        }
        "import_declaration" => {
            for i in 0..node.child_count() {
                if let Some(child) = node.child(i)
                    && child.kind() == "scoped_identifier" {
                        refs.push(ParsedReference::new(
                            node_text(child, source),
                            ReferenceKind::Import,
                            node_location(child),
                        ));
                    }
            }
        }
        "superclass" | "super_interfaces" => {
            for i in 0..node.child_count() {
                if let Some(child) = node.child(i)
                    && (child.kind() == "type_identifier" || child.kind() == "generic_type") {
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

    for i in 0..node.child_count() {
        if let Some(child) = node.child(i) {
            collect_java_references(child, source, refs);
        }
    }
}

fn is_primitive_type(name: &str) -> bool {
    matches!(
        name,
        "int"
            | "long"
            | "short"
            | "byte"
            | "float"
            | "double"
            | "boolean"
            | "char"
            | "void"
            | "String"
            | "Object"
            | "Integer"
            | "Long"
            | "Short"
            | "Byte"
            | "Float"
            | "Double"
            | "Boolean"
            | "Character"
            | "Void"
    )
}

fn is_common_method(name: &str) -> bool {
    matches!(
        name,
        "toString"
            | "equals"
            | "hashCode"
            | "getClass"
            | "clone"
            | "notify"
            | "notifyAll"
            | "wait"
            | "println"
            | "print"
            | "printf"
            | "format"
    )
}

