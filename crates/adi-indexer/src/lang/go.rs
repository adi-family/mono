//! Go language analyzer implementation.

use tree_sitter::{Node, Tree};

use super::common::{node_location, node_text, tree_walking_analyzer};
use crate::parser::treesitter::analyzers::LanguageAnalyzer;
use crate::types::{ParsedReference, ParsedSymbol, ReferenceKind, SymbolKind, Visibility};

/// The grammar this module analyses.
#[must_use]
pub fn language() -> tree_sitter::Language {
    tree_sitter_go::LANGUAGE.into()
}

#[derive(Debug)]
pub struct GoAnalyzer;

tree_walking_analyzer!(
    GoAnalyzer,
    symbols: extract_go_symbols,
    references: collect_go_references,
);

fn detect_visibility(name: &str) -> Visibility {
    if name
        .chars()
        .next()
        .is_some_and(char::is_uppercase)
    {
        Visibility::Public
    } else {
        Visibility::Private
    }
}

fn extract_go_symbols(node: Node, source: &str, symbols: &mut Vec<ParsedSymbol>) {
    match node.kind() {
        "function_declaration" => {
            if let Some(name) = node.child_by_field_name("name") {
                let name_text = node_text(name, source);
                let sig = extract_function_signature(node, source);
                symbols.push(
                    ParsedSymbol::new(name_text.clone(), SymbolKind::Function, node_location(node))
                        .with_signature(sig)
                        .with_visibility(detect_visibility(&name_text)),
                );
            }
        }
        "method_declaration" => {
            if let Some(name) = node.child_by_field_name("name") {
                let name_text = node_text(name, source);
                let receiver = node
                    .child_by_field_name("receiver")
                    .and_then(|r| r.child(1))
                    .map(|n| node_text(n, source))
                    .unwrap_or_default();
                let full_name = if receiver.is_empty() {
                    name_text.clone()
                } else {
                    format!("{receiver}.{name_text}")
                };
                symbols.push(
                    ParsedSymbol::new(full_name, SymbolKind::Method, node_location(node))
                        .with_visibility(detect_visibility(&name_text)),
                );
            }
        }
        "type_declaration" => {
            for i in 0..node.child_count() {
                if let Some(child) = node.child(i)
                    && child.kind() == "type_spec"
                        && let Some(name) = child.child_by_field_name("name") {
                            let name_text = node_text(name, source);
                            let type_node = child.child_by_field_name("type");
                            let kind = match type_node.map(|t| t.kind()) {
                                Some("struct_type") => SymbolKind::Struct,
                                Some("interface_type") => SymbolKind::Interface,
                                _ => SymbolKind::Type,
                            };
                            let mut children = Vec::new();
                            if let Some(type_def) = type_node {
                                if type_def.kind() == "struct_type" {
                                    if let Some(fields) = type_def.child_by_field_name("fields") {
                                        for j in 0..fields.child_count() {
                                            if let Some(field) = fields.child(j)
                                                && field.kind() == "field_declaration"
                                                    && let Some(field_name) =
                                                        field.child_by_field_name("name")
                                                    {
                                                        children.push(ParsedSymbol::new(
                                                            node_text(field_name, source),
                                                            SymbolKind::Field,
                                                            node_location(field),
                                                        ));
                                                    }
                                        }
                                    }
                                } else if type_def.kind() == "interface_type" {
                                    for j in 0..type_def.child_count() {
                                        if let Some(member) = type_def.child(j)
                                            && member.kind() == "method_spec"
                                                && let Some(method_name) =
                                                    member.child_by_field_name("name")
                                                {
                                                    children.push(ParsedSymbol::new(
                                                        node_text(method_name, source),
                                                        SymbolKind::Method,
                                                        node_location(member),
                                                    ));
                                                }
                                    }
                                }
                            }
                            symbols.push(
                                ParsedSymbol::new(name_text.clone(), kind, node_location(child))
                                    .with_visibility(detect_visibility(&name_text))
                                    .with_children(children),
                            );
                        }
            }
        }
        "const_declaration" | "var_declaration" => {
            for i in 0..node.child_count() {
                if let Some(child) = node.child(i)
                    && (child.kind() == "const_spec" || child.kind() == "var_spec")
                        && let Some(name) = child.child_by_field_name("name") {
                            let name_text = node_text(name, source);
                            let kind = if node.kind() == "const_declaration" {
                                SymbolKind::Constant
                            } else {
                                SymbolKind::Variable
                            };
                            symbols.push(
                                ParsedSymbol::new(name_text.clone(), kind, node_location(child))
                                    .with_visibility(detect_visibility(&name_text)),
                            );
                        }
            }
        }
        _ => {
            for i in 0..node.child_count() {
                if let Some(child) = node.child(i) {
                    extract_go_symbols(child, source, symbols);
                }
            }
        }
    }
}

fn extract_function_signature(node: Node, source: &str) -> String {
    let name = node
        .child_by_field_name("name")
        .map(|n| node_text(n, source))
        .unwrap_or_default();
    let params = node
        .child_by_field_name("parameters").map_or_else(|| "()".to_string(), |n| node_text(n, source));
    let result = node
        .child_by_field_name("result")
        .map(|n| format!(" {}", node_text(n, source)))
        .unwrap_or_default();
    format!("func {name}{params}{result}")
}

fn collect_go_references(node: Node, source: &str, refs: &mut Vec<ParsedReference>) {
    match node.kind() {
        "call_expression" => {
            if let Some(func) = node.child_by_field_name("function") {
                let name = node_text(func, source);
                if !is_builtin(&name) {
                    refs.push(ParsedReference::new(
                        name,
                        ReferenceKind::Call,
                        node_location(func),
                    ));
                }
            }
        }
        "import_declaration" => {
            for i in 0..node.child_count() {
                if let Some(child) = node.child(i) {
                    if child.kind() == "import_spec" {
                        if let Some(path) = child.child_by_field_name("path") {
                            let import_path = node_text(path, source).trim_matches('"').to_string();
                            refs.push(ParsedReference::new(
                                import_path,
                                ReferenceKind::Import,
                                node_location(path),
                            ));
                        }
                    } else if child.kind() == "interpreted_string_literal" {
                        let import_path = node_text(child, source).trim_matches('"').to_string();
                        refs.push(ParsedReference::new(
                            import_path,
                            ReferenceKind::Import,
                            node_location(child),
                        ));
                    }
                }
            }
        }
        "type_identifier" => {
            let name = node_text(node, source);
            if !is_primitive(&name) {
                refs.push(ParsedReference::new(
                    name,
                    ReferenceKind::TypeReference,
                    node_location(node),
                ));
            }
        }
        "selector_expression" => {
            if let Some(field) = node.child_by_field_name("field") {
                refs.push(ParsedReference::new(
                    node_text(field, source),
                    ReferenceKind::FieldAccess,
                    node_location(field),
                ));
            }
        }
        _ => {}
    }
    for i in 0..node.child_count() {
        if let Some(child) = node.child(i) {
            collect_go_references(child, source, refs);
        }
    }
}

fn is_builtin(name: &str) -> bool {
    matches!(
        name,
        "fmt.Println"
            | "fmt.Printf"
            | "fmt.Sprintf"
            | "make"
            | "new"
            | "append"
            | "len"
            | "cap"
            | "copy"
            | "delete"
            | "close"
            | "panic"
            | "recover"
            | "print"
            | "println"
    )
}

fn is_primitive(name: &str) -> bool {
    matches!(
        name,
        "int"
            | "int8"
            | "int16"
            | "int32"
            | "int64"
            | "uint"
            | "uint8"
            | "uint16"
            | "uint32"
            | "uint64"
            | "uintptr"
            | "float32"
            | "float64"
            | "complex64"
            | "complex128"
            | "string"
            | "bool"
            | "byte"
            | "rune"
            | "error"
            | "any"
    )
}
