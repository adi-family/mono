//! PHP language analyzer implementation.

use tree_sitter::{Node, Tree};

use crate::parser::treesitter::analyzers::LanguageAnalyzer;
use crate::types::{Location, ParsedReference, ParsedSymbol, ReferenceKind, SymbolKind, Visibility};

/// The grammar this module analyses.
#[must_use]
pub fn language() -> tree_sitter::Language {
    tree_sitter_php::LANGUAGE_PHP.into()
}

#[derive(Debug)]
pub struct PhpAnalyzer;

impl LanguageAnalyzer for PhpAnalyzer {
    fn extract_symbols(&self, source: &str, tree: &Tree) -> Vec<ParsedSymbol> {
        let mut symbols = Vec::new();
        extract_php_symbols(tree.root_node(), source, &mut symbols);
        symbols
    }

    fn extract_references(&self, source: &str, tree: &Tree) -> Vec<ParsedReference> {
        let mut refs = Vec::new();
        collect_php_references(tree.root_node(), source, &mut refs);
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
    for i in 0..node.child_count() {
        if let Some(child) = node.child(i)
            && child.kind() == "visibility_modifier" {
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

fn extract_function_signature(node: Node, source: &str) -> String {
    let text = node_text(node, source);
    if let Some(brace_pos) = text.find('{') {
        text[..brace_pos].trim().to_string()
    } else {
        text.lines().next().unwrap_or("").to_string()
    }
}

fn extract_php_symbols(node: Node, source: &str, symbols: &mut Vec<ParsedSymbol>) {
    match node.kind() {
        "class_declaration" => {
            if let Some(symbol) = parse_php_class(node, source) {
                symbols.push(symbol);
            }
        }
        "interface_declaration" => {
            if let Some(symbol) = parse_php_interface(node, source) {
                symbols.push(symbol);
            }
        }
        "trait_declaration" => {
            if let Some(symbol) = parse_php_trait(node, source) {
                symbols.push(symbol);
            }
        }
        "enum_declaration" => {
            if let Some(symbol) = parse_php_enum(node, source) {
                symbols.push(symbol);
            }
        }
        "function_definition" => {
            if let Some(symbol) = parse_php_function(node, source) {
                symbols.push(symbol);
            }
        }
        "method_declaration" => {
            if let Some(symbol) = parse_php_method(node, source) {
                symbols.push(symbol);
            }
        }
        "property_declaration" => {
            parse_php_properties(node, source, symbols);
        }
        "const_declaration" => {
            parse_php_constants(node, source, symbols);
        }
        "namespace_definition" => {
            if let Some(symbol) = parse_php_namespace(node, source) {
                symbols.push(symbol);
            }
        }
        _ => {}
    }

    for i in 0..node.child_count() {
        if let Some(child) = node.child(i) {
            extract_php_symbols(child, source, symbols);
        }
    }
}

fn parse_php_class(node: Node, source: &str) -> Option<ParsedSymbol> {
    let name = node.child_by_field_name("name")?;
    let name_text = node_text(name, source);
    let doc_comment = extract_doc_comment(node, source);

    Some(
        ParsedSymbol::new(name_text, SymbolKind::Class, node_location(node))
            .with_visibility(Visibility::Public)
            .with_doc_comment_opt(doc_comment),
    )
}

fn parse_php_interface(node: Node, source: &str) -> Option<ParsedSymbol> {
    let name = node.child_by_field_name("name")?;
    let name_text = node_text(name, source);
    let doc_comment = extract_doc_comment(node, source);

    Some(
        ParsedSymbol::new(name_text, SymbolKind::Interface, node_location(node))
            .with_visibility(Visibility::Public)
            .with_doc_comment_opt(doc_comment),
    )
}

fn parse_php_trait(node: Node, source: &str) -> Option<ParsedSymbol> {
    let name = node.child_by_field_name("name")?;
    let name_text = node_text(name, source);
    let doc_comment = extract_doc_comment(node, source);

    Some(
        ParsedSymbol::new(name_text, SymbolKind::Trait, node_location(node))
            .with_visibility(Visibility::Public)
            .with_doc_comment_opt(doc_comment),
    )
}

fn parse_php_enum(node: Node, source: &str) -> Option<ParsedSymbol> {
    let name = node.child_by_field_name("name")?;
    let name_text = node_text(name, source);
    let doc_comment = extract_doc_comment(node, source);

    Some(
        ParsedSymbol::new(name_text, SymbolKind::Enum, node_location(node))
            .with_visibility(Visibility::Public)
            .with_doc_comment_opt(doc_comment),
    )
}

fn parse_php_function(node: Node, source: &str) -> Option<ParsedSymbol> {
    let name = node.child_by_field_name("name")?;
    let name_text = node_text(name, source);
    let doc_comment = extract_doc_comment(node, source);
    let signature = extract_function_signature(node, source);

    Some(
        ParsedSymbol::new(name_text, SymbolKind::Function, node_location(node))
            .with_signature(signature)
            .with_visibility(Visibility::Public)
            .with_doc_comment_opt(doc_comment),
    )
}

fn parse_php_method(node: Node, source: &str) -> Option<ParsedSymbol> {
    let name = node.child_by_field_name("name")?;
    let name_text = node_text(name, source);
    let doc_comment = extract_doc_comment(node, source);
    let visibility = extract_visibility(node, source);
    let signature = extract_function_signature(node, source);

    Some(
        ParsedSymbol::new(name_text, SymbolKind::Method, node_location(node))
            .with_signature(signature)
            .with_visibility(visibility)
            .with_doc_comment_opt(doc_comment),
    )
}

fn parse_php_properties(node: Node, source: &str, symbols: &mut Vec<ParsedSymbol>) {
    let visibility = extract_visibility(node, source);
    let doc_comment = extract_doc_comment(node, source);

    for i in 0..node.child_count() {
        if let Some(child) = node.child(i)
            && child.kind() == "property_element"
                && let Some(name) = child.child_by_field_name("name") {
                    let name_text = node_text(name, source);
                    symbols.push(
                        ParsedSymbol::new(name_text, SymbolKind::Property, node_location(child))
                            .with_visibility(visibility)
                            .with_doc_comment_opt(doc_comment.clone()),
                    );
                }
    }
}

fn parse_php_constants(node: Node, source: &str, symbols: &mut Vec<ParsedSymbol>) {
    let visibility = extract_visibility(node, source);
    let doc_comment = extract_doc_comment(node, source);

    for i in 0..node.child_count() {
        if let Some(child) = node.child(i)
            && child.kind() == "const_element"
                && let Some(name) = child.child_by_field_name("name") {
                    let name_text = node_text(name, source);
                    symbols.push(
                        ParsedSymbol::new(name_text, SymbolKind::Constant, node_location(child))
                            .with_visibility(visibility)
                            .with_doc_comment_opt(doc_comment.clone()),
                    );
                }
    }
}

fn parse_php_namespace(node: Node, source: &str) -> Option<ParsedSymbol> {
    let name = node.child_by_field_name("name")?;
    let name_text = node_text(name, source);

    Some(ParsedSymbol::new(
        name_text,
        SymbolKind::Namespace,
        node_location(node),
    ))
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
            for i in 0..node.child_count() {
                if let Some(child) = node.child(i)
                    && (child.kind() == "name" || child.kind() == "qualified_name") {
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
            for i in 0..node.child_count() {
                if let Some(child) = node.child(i)
                    && child.kind() == "namespace_use_clause"
                        && let Some(name) = child.child_by_field_name("name") {
                            refs.push(ParsedReference::new(
                                node_text(name, source),
                                ReferenceKind::Import,
                                node_location(name),
                            ));
                        }
            }
        }
        "base_clause" | "class_interface_clause" => {
            for i in 0..node.child_count() {
                if let Some(child) = node.child(i)
                    && (child.kind() == "name" || child.kind() == "qualified_name") {
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
