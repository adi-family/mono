//! C# language analyzer implementation.

use tree_sitter::{Node, Tree};

use super::common::{
    declaration, node_location, node_text, tree_walking_analyzer, WithDocCommentOpt,
};
use crate::parser::treesitter::analyzers::LanguageAnalyzer;
use crate::types::{ParsedReference, ParsedSymbol, ReferenceKind, SymbolKind, Visibility};

/// The grammar this module analyses.
#[must_use]
pub fn language() -> tree_sitter::Language {
    tree_sitter_c_sharp::LANGUAGE.into()
}

#[derive(Debug)]
pub struct CSharpAnalyzer;

tree_walking_analyzer!(
    CSharpAnalyzer,
    symbols: extract_csharp_symbols,
    references: collect_csharp_references,
);

fn extract_doc_comment(node: Node, source: &str) -> Option<String> {
    let mut prev = node.prev_sibling();
    let mut comments = Vec::new();

    while let Some(sibling) = prev {
        match sibling.kind() {
            "comment" => {
                let text = node_text(sibling, source);
                if text.starts_with("///") {
                    comments.push(text.trim_start_matches("///").trim().to_string());
                } else {
                    break;
                }
            }
            "attribute_list" => {}
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
        if let Some(child) = node.child(i) {
            let text = node_text(child, source);
            match text.as_str() {
                "public" => return Visibility::Public,
                "private" => return Visibility::Private,
                "protected" => return Visibility::Protected,
                "internal" => return Visibility::Internal,
                _ => {}
            }
        }
    }
    Visibility::Private
}

fn extract_method_signature(node: Node, source: &str) -> String {
    let text = node_text(node, source);
    if let Some(brace_pos) = text.find('{') {
        text[..brace_pos].trim().to_string()
    } else if let Some(arrow_pos) = text.find("=>") {
        text[..arrow_pos].trim().to_string()
    } else {
        text.lines().next().unwrap_or("").to_string()
    }
}

fn extract_csharp_symbols(node: Node, source: &str, symbols: &mut Vec<ParsedSymbol>) {
    let parsed = match node.kind() {
        "class_declaration" => parse_csharp_declaration(node, source, SymbolKind::Class),
        "struct_declaration" => parse_csharp_declaration(node, source, SymbolKind::Struct),
        "interface_declaration" => parse_csharp_declaration(node, source, SymbolKind::Interface),
        "enum_declaration" => parse_csharp_declaration(node, source, SymbolKind::Enum),
        "property_declaration" => parse_csharp_declaration(node, source, SymbolKind::Property),
        "method_declaration" => parse_csharp_callable(node, source, SymbolKind::Method),
        "constructor_declaration" => parse_csharp_callable(node, source, SymbolKind::Constructor),
        // A namespace carries neither a doc comment of its own nor a visibility keyword.
        "namespace_declaration" => {
            declaration(node, source, SymbolKind::Namespace, None, None, None)
        }
        "field_declaration" => {
            parse_csharp_fields(node, source, symbols);
            None
        }
        _ => None,
    };
    symbols.extend(parsed);

    for i in 0..node.child_count() as u32 {
        if let Some(child) = node.child(i) {
            extract_csharp_symbols(child, source, symbols);
        }
    }
}

/// A C# type or property: named by its `name` field, preceded by a `///` block and a visibility
/// keyword.
fn parse_csharp_declaration(node: Node, source: &str, kind: SymbolKind) -> Option<ParsedSymbol> {
    declaration(
        node,
        source,
        kind,
        extract_doc_comment(node, source),
        Some(extract_visibility(node, source)),
        None,
    )
}

/// A C# method or constructor — the same, plus the signature its body is stripped down to.
fn parse_csharp_callable(node: Node, source: &str, kind: SymbolKind) -> Option<ParsedSymbol> {
    declaration(
        node,
        source,
        kind,
        extract_doc_comment(node, source),
        Some(extract_visibility(node, source)),
        Some(extract_method_signature(node, source)),
    )
}

fn parse_csharp_fields(node: Node, source: &str, symbols: &mut Vec<ParsedSymbol>) {
    let visibility = extract_visibility(node, source);
    let doc_comment = extract_doc_comment(node, source);

    for i in 0..node.child_count() as u32 {
        if let Some(child) = node.child(i)
            && child.kind() == "variable_declaration" {
                for j in 0..child.child_count() as u32 {
                    if let Some(declarator) = child.child(j)
                        && declarator.kind() == "variable_declarator"
                            && let Some(name) = declarator.child_by_field_name("name") {
                                symbols.push(
                                    ParsedSymbol::new(
                                        node_text(name, source),
                                        SymbolKind::Field,
                                        node_location(declarator),
                                    )
                                    .with_visibility(visibility)
                                    .with_doc_comment_opt(doc_comment.clone()),
                                );
                            }
                }
            }
    }
}

fn collect_csharp_references(node: Node, source: &str, refs: &mut Vec<ParsedReference>) {
    match node.kind() {
        "invocation_expression" => {
            if let Some(expr) = node.child(0) {
                let name = extract_invocation_name(expr, source);
                if !name.is_empty() && !is_common_method(&name) {
                    refs.push(ParsedReference::new(
                        name,
                        ReferenceKind::Call,
                        node_location(expr),
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
        "identifier" | "generic_name" => {
            let parent = node.parent();
            if let Some(p) = parent
                && (p.kind() == "type" || p.kind() == "base_list") {
                    let name = node_text(node, source);
                    if !is_primitive_type(&name) {
                        refs.push(ParsedReference::new(
                            name,
                            ReferenceKind::TypeReference,
                            node_location(node),
                        ));
                    }
                }
        }
        "member_access_expression" => {
            if let Some(name) = node.child_by_field_name("name") {
                refs.push(ParsedReference::new(
                    node_text(name, source),
                    ReferenceKind::FieldAccess,
                    node_location(name),
                ));
            }
        }
        "using_directive" => {
            if let Some(name) = node.child_by_field_name("name") {
                refs.push(ParsedReference::new(
                    node_text(name, source),
                    ReferenceKind::Import,
                    node_location(name),
                ));
            }
        }
        "base_list" => {
            for i in 0..node.child_count() as u32 {
                if let Some(child) = node.child(i)
                    && (child.kind() == "identifier" || child.kind() == "generic_name") {
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
            collect_csharp_references(child, source, refs);
        }
    }
}

fn extract_invocation_name(node: Node, source: &str) -> String {
    match node.kind() {
        "identifier" => node_text(node, source),
        "member_access_expression" => {
            if let Some(name) = node.child_by_field_name("name") {
                node_text(name, source)
            } else {
                String::new()
            }
        }
        "generic_name" => {
            if let Some(name) = node.child(0) {
                node_text(name, source)
            } else {
                String::new()
            }
        }
        _ => String::new(),
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
            | "decimal"
            | "bool"
            | "char"
            | "string"
            | "object"
            | "void"
            | "dynamic"
            | "var"
            | "Int32"
            | "Int64"
            | "Int16"
            | "Byte"
            | "Single"
            | "Double"
            | "Decimal"
            | "Boolean"
            | "Char"
            | "String"
            | "Object"
            | "Void"
    )
}

fn is_common_method(name: &str) -> bool {
    matches!(
        name,
        "ToString" | "Equals" | "GetHashCode" | "GetType" | "WriteLine" | "Write" | "ReadLine"
    )
}

