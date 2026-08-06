//! Swift language analyzer implementation.

use tree_sitter::{Node, Tree};

use crate::parser::treesitter::analyzers::LanguageAnalyzer;
use crate::types::{Location, ParsedReference, ParsedSymbol, ReferenceKind, SymbolKind, Visibility};

/// The grammar this module analyses.
#[must_use]
pub fn language() -> tree_sitter::Language {
    tree_sitter_swift::LANGUAGE.into()
}

#[derive(Debug)]
pub struct SwiftAnalyzer;

impl LanguageAnalyzer for SwiftAnalyzer {
    fn extract_symbols(&self, source: &str, tree: &Tree) -> Vec<ParsedSymbol> {
        let mut symbols = Vec::new();
        extract_swift_symbols(tree.root_node(), source, &mut symbols);
        symbols
    }

    fn extract_references(&self, source: &str, tree: &Tree) -> Vec<ParsedReference> {
        let mut refs = Vec::new();
        collect_swift_references(tree.root_node(), source, &mut refs);
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
    for i in 0..node.child_count() {
        if let Some(child) = node.child(i)
            && child.kind() == "modifiers" {
                for j in 0..child.child_count() {
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

fn extract_function_signature(node: Node, source: &str) -> String {
    let text = node_text(node, source);
    if let Some(brace_pos) = text.find('{') {
        text[..brace_pos].trim().to_string()
    } else {
        text.lines().next().unwrap_or("").to_string()
    }
}

fn extract_swift_symbols(node: Node, source: &str, symbols: &mut Vec<ParsedSymbol>) {
    match node.kind() {
        "class_declaration" => {
            if let Some(symbol) = parse_swift_class(node, source) {
                symbols.push(symbol);
            }
        }
        "struct_declaration" => {
            if let Some(symbol) = parse_swift_struct(node, source) {
                symbols.push(symbol);
            }
        }
        "protocol_declaration" => {
            if let Some(symbol) = parse_swift_protocol(node, source) {
                symbols.push(symbol);
            }
        }
        "enum_declaration" => {
            if let Some(symbol) = parse_swift_enum(node, source) {
                symbols.push(symbol);
            }
        }
        "function_declaration" => {
            if let Some(symbol) = parse_swift_function(node, source) {
                symbols.push(symbol);
            }
        }
        "property_declaration" => {
            if let Some(symbol) = parse_swift_property(node, source) {
                symbols.push(symbol);
            }
        }
        "init_declaration" => {
            if let Some(symbol) = parse_swift_init(node, source) {
                symbols.push(symbol);
            }
        }
        "deinit_declaration" => {
            symbols.push(parse_swift_deinit(node));
        }
        "typealias_declaration" => {
            if let Some(symbol) = parse_swift_typealias(node, source) {
                symbols.push(symbol);
            }
        }
        "extension_declaration" => {
            if let Some(symbol) = parse_swift_extension(node, source) {
                symbols.push(symbol);
            }
        }
        _ => {}
    }

    for i in 0..node.child_count() {
        if let Some(child) = node.child(i) {
            extract_swift_symbols(child, source, symbols);
        }
    }
}

fn parse_swift_class(node: Node, source: &str) -> Option<ParsedSymbol> {
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

fn parse_swift_struct(node: Node, source: &str) -> Option<ParsedSymbol> {
    let name = node.child_by_field_name("name")?;
    let name_text = node_text(name, source);
    let doc_comment = extract_doc_comment(node, source);
    let visibility = extract_visibility(node, source);

    Some(
        ParsedSymbol::new(name_text, SymbolKind::Struct, node_location(node))
            .with_visibility(visibility)
            .with_doc_comment_opt(doc_comment),
    )
}

fn parse_swift_protocol(node: Node, source: &str) -> Option<ParsedSymbol> {
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

fn parse_swift_enum(node: Node, source: &str) -> Option<ParsedSymbol> {
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

fn parse_swift_function(node: Node, source: &str) -> Option<ParsedSymbol> {
    let name = node.child_by_field_name("name")?;
    let name_text = node_text(name, source);
    let doc_comment = extract_doc_comment(node, source);
    let visibility = extract_visibility(node, source);
    let signature = extract_function_signature(node, source);

    Some(
        ParsedSymbol::new(name_text, SymbolKind::Function, node_location(node))
            .with_signature(signature)
            .with_visibility(visibility)
            .with_doc_comment_opt(doc_comment),
    )
}

fn parse_swift_property(node: Node, source: &str) -> Option<ParsedSymbol> {
    for i in 0..node.child_count() {
        if let Some(child) = node.child(i)
            && child.kind() == "pattern" {
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

fn parse_swift_init(node: Node, source: &str) -> Option<ParsedSymbol> {
    let doc_comment = extract_doc_comment(node, source);
    let visibility = extract_visibility(node, source);
    let signature = extract_function_signature(node, source);

    Some(
        ParsedSymbol::new(
            "init".to_string(),
            SymbolKind::Constructor,
            node_location(node),
        )
        .with_signature(signature)
        .with_visibility(visibility)
        .with_doc_comment_opt(doc_comment),
    )
}

fn parse_swift_deinit(node: Node) -> ParsedSymbol {
    ParsedSymbol::new(
        "deinit".to_string(),
        SymbolKind::Destructor,
        node_location(node),
    )
    .with_visibility(Visibility::Internal)
}

fn parse_swift_typealias(node: Node, source: &str) -> Option<ParsedSymbol> {
    let name = node.child_by_field_name("name")?;
    let name_text = node_text(name, source);
    let doc_comment = extract_doc_comment(node, source);
    let visibility = extract_visibility(node, source);

    Some(
        ParsedSymbol::new(name_text, SymbolKind::Type, node_location(node))
            .with_visibility(visibility)
            .with_doc_comment_opt(doc_comment),
    )
}

fn parse_swift_extension(node: Node, source: &str) -> Option<ParsedSymbol> {
    for i in 0..node.child_count() {
        if let Some(child) = node.child(i)
            && (child.kind() == "user_type" || child.kind() == "type_identifier") {
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
            for i in 0..node.child_count() {
                if let Some(child) = node.child(i)
                    && child.kind() == "identifier" {
                        refs.push(ParsedReference::new(
                            node_text(child, source),
                            ReferenceKind::Import,
                            node_location(child),
                        ));
                    }
            }
        }
        "inheritance_specifier" => {
            for i in 0..node.child_count() {
                if let Some(child) = node.child(i)
                    && (child.kind() == "user_type" || child.kind() == "type_identifier") {
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
