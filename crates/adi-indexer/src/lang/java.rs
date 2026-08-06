//! Java language analyzer implementation.

use tree_sitter::{Node, Tree};

use crate::parser::treesitter::analyzers::LanguageAnalyzer;
use crate::types::{Location, ParsedReference, ParsedSymbol, ReferenceKind, SymbolKind, Visibility};

/// The grammar this module analyses.
#[must_use]
pub fn language() -> tree_sitter::Language {
    tree_sitter_java::LANGUAGE.into()
}

#[derive(Debug)]
pub struct JavaAnalyzer;

impl LanguageAnalyzer for JavaAnalyzer {
    fn extract_symbols(&self, source: &str, tree: &Tree) -> Vec<ParsedSymbol> {
        let mut symbols = Vec::new();
        extract_java_symbols(tree.root_node(), source, &mut symbols);
        symbols
    }

    fn extract_references(&self, source: &str, tree: &Tree) -> Vec<ParsedReference> {
        let mut refs = Vec::new();
        collect_java_references(tree.root_node(), source, &mut refs);
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
    match node.kind() {
        "class_declaration" => {
            if let Some(symbol) = parse_java_class(node, source) {
                symbols.push(symbol);
            }
        }
        "interface_declaration" => {
            if let Some(symbol) = parse_java_interface(node, source) {
                symbols.push(symbol);
            }
        }
        "enum_declaration" => {
            if let Some(symbol) = parse_java_enum(node, source) {
                symbols.push(symbol);
            }
        }
        "method_declaration" => {
            if let Some(symbol) = parse_java_method(node, source) {
                symbols.push(symbol);
            }
        }
        "constructor_declaration" => {
            if let Some(symbol) = parse_java_constructor(node, source) {
                symbols.push(symbol);
            }
        }
        "field_declaration" => {
            parse_java_fields(node, source, symbols);
        }
        "constant_declaration" => {
            parse_java_constants(node, source, symbols);
        }
        _ => {}
    }

    for i in 0..node.child_count() {
        if let Some(child) = node.child(i) {
            extract_java_symbols(child, source, symbols);
        }
    }
}

fn parse_java_class(node: Node, source: &str) -> Option<ParsedSymbol> {
    let name = node.child_by_field_name("name")?;
    let name_text = node_text(name, source);
    let doc_comment = extract_doc_comment(node, source);
    let visibility = extract_visibility(node, source);

    Some(
        ParsedSymbol::new(name_text, SymbolKind::Class, node_location(node))
            .with_visibility(visibility)
            .with_doc_comment_opt(doc_comment),
    )
}

fn parse_java_interface(node: Node, source: &str) -> Option<ParsedSymbol> {
    let name = node.child_by_field_name("name")?;
    let name_text = node_text(name, source);
    let doc_comment = extract_doc_comment(node, source);
    let visibility = extract_visibility(node, source);

    Some(
        ParsedSymbol::new(name_text, SymbolKind::Interface, node_location(node))
            .with_visibility(visibility)
            .with_doc_comment_opt(doc_comment),
    )
}

fn parse_java_enum(node: Node, source: &str) -> Option<ParsedSymbol> {
    let name = node.child_by_field_name("name")?;
    let name_text = node_text(name, source);
    let doc_comment = extract_doc_comment(node, source);
    let visibility = extract_visibility(node, source);

    Some(
        ParsedSymbol::new(name_text, SymbolKind::Enum, node_location(node))
            .with_visibility(visibility)
            .with_doc_comment_opt(doc_comment),
    )
}

fn parse_java_method(node: Node, source: &str) -> Option<ParsedSymbol> {
    let name = node.child_by_field_name("name")?;
    let name_text = node_text(name, source);
    let doc_comment = extract_doc_comment(node, source);
    let visibility = extract_visibility(node, source);
    let signature = extract_method_signature(node, source);

    Some(
        ParsedSymbol::new(name_text, SymbolKind::Method, node_location(node))
            .with_signature(signature)
            .with_visibility(visibility)
            .with_doc_comment_opt(doc_comment),
    )
}

fn parse_java_constructor(node: Node, source: &str) -> Option<ParsedSymbol> {
    let name = node.child_by_field_name("name")?;
    let name_text = node_text(name, source);
    let doc_comment = extract_doc_comment(node, source);
    let visibility = extract_visibility(node, source);
    let signature = extract_method_signature(node, source);

    Some(
        ParsedSymbol::new(name_text, SymbolKind::Constructor, node_location(node))
            .with_signature(signature)
            .with_visibility(visibility)
            .with_doc_comment_opt(doc_comment),
    )
}

fn parse_java_fields(node: Node, source: &str, symbols: &mut Vec<ParsedSymbol>) {
    let visibility = extract_visibility(node, source);
    let doc_comment = extract_doc_comment(node, source);

    for i in 0..node.child_count() {
        if let Some(child) = node.child(i)
            && child.kind() == "variable_declarator"
                && let Some(name) = child.child_by_field_name("name") {
                    symbols.push(
                        ParsedSymbol::new(
                            node_text(name, source),
                            SymbolKind::Field,
                            node_location(child),
                        )
                        .with_visibility(visibility)
                        .with_doc_comment_opt(doc_comment.clone()),
                    );
                }
    }
}

fn parse_java_constants(node: Node, source: &str, symbols: &mut Vec<ParsedSymbol>) {
    let visibility = extract_visibility(node, source);
    let doc_comment = extract_doc_comment(node, source);

    for i in 0..node.child_count() {
        if let Some(child) = node.child(i)
            && child.kind() == "variable_declarator"
                && let Some(name) = child.child_by_field_name("name") {
                    symbols.push(
                        ParsedSymbol::new(
                            node_text(name, source),
                            SymbolKind::Constant,
                            node_location(child),
                        )
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
