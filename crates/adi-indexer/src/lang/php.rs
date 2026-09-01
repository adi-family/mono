//! PHP language analyzer implementation.

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
    tree_sitter_php::LANGUAGE_PHP.into()
}

#[derive(Debug)]
pub struct PhpAnalyzer;

tree_walking_analyzer!(
    PhpAnalyzer,
    symbols: extract_php_symbols,
    references: collect_php_references,
);

fn extract_doc_comment(node: Node, source: &str) -> Option<String> {
    let mut prev = node.prev_sibling();
    while let Some(sibling) = prev {
        match sibling.kind() {
            "comment" => {
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
            "attribute_list" => {}
            _ => break,
        }
        prev = sibling.prev_sibling();
    }
    None
}

fn extract_visibility(node: Node, source: &str) -> Visibility {
    for i in 0..node.child_count() as u32 {
        if let Some(child) = node.child(i)
            && child.kind() == "visibility_modifier"
        {
            let text = node_text(child, source);
            match text.as_str() {
                "public" => return Visibility::Public,
                "private" => return Visibility::Private,
                "protected" => return Visibility::Protected,
                _ => {}
            }
        }
    }
    Visibility::Public
}

/// Only a brace. Not `=>` — PHP spells an array key with it, so a parameter defaulting to an
/// array literal would be cut mid-list.
fn extract_function_signature(node: Node, source: &str) -> String {
    signature_before(node, source, &["{"])
}

fn extract_php_symbols(node: Node, source: &str, symbols: &mut Vec<ParsedSymbol>) {
    let parsed = match node.kind() {
        "class_declaration" => parse_php_declaration(node, source, SymbolKind::Class),
        "interface_declaration" => parse_php_declaration(node, source, SymbolKind::Interface),
        "trait_declaration" => parse_php_declaration(node, source, SymbolKind::Trait),
        "enum_declaration" => parse_php_declaration(node, source, SymbolKind::Enum),
        // A top-level function is always public; only a method carries a visibility keyword.
        "function_definition" => {
            parse_php_callable(node, source, SymbolKind::Function, Visibility::Public)
        }
        "method_declaration" => parse_php_callable(
            node,
            source,
            SymbolKind::Method,
            extract_visibility(node, source),
        ),
        "property_declaration" => {
            parse_php_elements(
                node,
                source,
                symbols,
                "property_element",
                SymbolKind::Property,
            );
            None
        }
        "const_declaration" => {
            parse_php_elements(node, source, symbols, "const_element", SymbolKind::Constant);
            None
        }
        // A namespace carries neither a docblock of its own nor a visibility keyword.
        "namespace_definition" => {
            declaration(node, source, SymbolKind::Namespace, None, None, None)
        }
        _ => None,
    };
    symbols.extend(parsed);

    for i in 0..node.child_count() as u32 {
        if let Some(child) = node.child(i) {
            extract_php_symbols(child, source, symbols);
        }
    }
}

/// A PHP type: named by its `name` field, preceded by a docblock. None of the four can be
/// anything but public.
fn parse_php_declaration(node: Node, source: &str, kind: SymbolKind) -> Option<ParsedSymbol> {
    declaration(
        node,
        source,
        kind,
        extract_doc_comment(node, source),
        Some(Visibility::Public),
        None,
    )
}

/// A PHP function or method — the same, plus the signature its body is stripped down to.
fn parse_php_callable(
    node: Node,
    source: &str,
    kind: SymbolKind,
    visibility: Visibility,
) -> Option<ParsedSymbol> {
    declaration(
        node,
        source,
        kind,
        extract_doc_comment(node, source),
        Some(visibility),
        Some(extract_function_signature(node, source)),
    )
}

/// The elements of a `property_declaration` or a `const_declaration` — one statement can name
/// several, and they share the visibility and docblock written once in front.
fn parse_php_elements(
    node: Node,
    source: &str,
    symbols: &mut Vec<ParsedSymbol>,
    element: &str,
    kind: SymbolKind,
) {
    let visibility = extract_visibility(node, source);
    let doc_comment = extract_doc_comment(node, source);

    for i in 0..node.child_count() as u32 {
        if let Some(child) = node.child(i)
            && child.kind() == element
            && let Some(name) = child.child_by_field_name("name")
        {
            let name_text = node_text(name, source);
            symbols.push(
                ParsedSymbol::new(name_text, kind, node_location(child))
                    .with_visibility(visibility)
                    .with_doc_comment_opt(doc_comment.clone()),
            );
        }
    }
}

fn collect_php_references(node: Node, source: &str, refs: &mut Vec<ParsedReference>) {
    match node.kind() {
        "function_call_expression" => {
            if let Some(func) = node.child_by_field_name("function") {
                let name = node_text(func, source);
                if !is_builtin_function(&name) {
                    refs.push(ParsedReference::new(
                        name,
                        ReferenceKind::Call,
                        node_location(func),
                    ));
                }
            }
        }
        "member_call_expression" | "nullsafe_member_call_expression" => {
            if let Some(name) = node.child_by_field_name("name") {
                refs.push(ParsedReference::new(
                    node_text(name, source),
                    ReferenceKind::Call,
                    node_location(name),
                ));
            }
        }
        "scoped_call_expression" => {
            if let Some(name) = node.child_by_field_name("name") {
                refs.push(ParsedReference::new(
                    node_text(name, source),
                    ReferenceKind::Call,
                    node_location(name),
                ));
            }
        }
        "object_creation_expression" => {
            for i in 0..node.child_count() as u32 {
                if let Some(child) = node.child(i)
                    && (child.kind() == "name" || child.kind() == "qualified_name")
                {
                    refs.push(ParsedReference::new(
                        node_text(child, source),
                        ReferenceKind::Call,
                        node_location(child),
                    ));
                }
            }
        }
        "named_type" => {
            let name = node_text(node, source);
            if !is_primitive_type(&name) {
                refs.push(ParsedReference::new(
                    name,
                    ReferenceKind::TypeReference,
                    node_location(node),
                ));
            }
        }
        "member_access_expression" | "nullsafe_member_access_expression" => {
            if let Some(name) = node.child_by_field_name("name") {
                refs.push(ParsedReference::new(
                    node_text(name, source),
                    ReferenceKind::FieldAccess,
                    node_location(name),
                ));
            }
        }
        "namespace_use_declaration" => {
            for i in 0..node.child_count() as u32 {
                if let Some(child) = node.child(i)
                    && child.kind() == "namespace_use_clause"
                    && let Some(name) = child.child_by_field_name("name")
                {
                    refs.push(ParsedReference::new(
                        node_text(name, source),
                        ReferenceKind::Import,
                        node_location(name),
                    ));
                }
            }
        }
        "base_clause" | "class_interface_clause" => {
            for i in 0..node.child_count() as u32 {
                if let Some(child) = node.child(i)
                    && (child.kind() == "name" || child.kind() == "qualified_name")
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
            collect_php_references(child, source, refs);
        }
    }
}

fn is_primitive_type(name: &str) -> bool {
    matches!(
        name,
        "int"
            | "float"
            | "string"
            | "bool"
            | "array"
            | "object"
            | "callable"
            | "iterable"
            | "void"
            | "null"
            | "mixed"
            | "never"
            | "true"
            | "false"
            | "self"
            | "static"
            | "parent"
    )
}

fn is_builtin_function(name: &str) -> bool {
    matches!(
        name,
        "echo"
            | "print"
            | "var_dump"
            | "print_r"
            | "isset"
            | "empty"
            | "unset"
            | "die"
            | "exit"
            | "array"
            | "list"
            | "include"
            | "include_once"
            | "require"
            | "require_once"
    )
}
