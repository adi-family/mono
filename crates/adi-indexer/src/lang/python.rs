//! Python language analyzer implementation.

use tree_sitter::{Node, Tree};

use crate::parser::treesitter::analyzers::LanguageAnalyzer;
use crate::types::{Location, ParsedReference, ParsedSymbol, ReferenceKind, SymbolKind, Visibility};

/// The grammar this module analyses.
#[must_use]
pub fn language() -> tree_sitter::Language {
    tree_sitter_python::LANGUAGE.into()
}

#[derive(Debug)]
pub struct PythonAnalyzer;

impl LanguageAnalyzer for PythonAnalyzer {
    fn extract_symbols(&self, source: &str, tree: &Tree) -> Vec<ParsedSymbol> {
        let mut symbols = Vec::new();
        extract_python_symbols(tree.root_node(), source, &mut symbols);
        symbols
    }

    fn extract_references(&self, source: &str, tree: &Tree) -> Vec<ParsedReference> {
        let mut refs = Vec::new();
        collect_python_references(tree.root_node(), source, &mut refs);
        refs
    }
}

fn node_text(node: Node, source: &str) -> String {
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

fn detect_visibility(name: &str) -> Visibility {
    if name.starts_with("__") && !name.ends_with("__") {
        Visibility::Private
    } else if name.starts_with('_') {
        Visibility::Protected
    } else {
        Visibility::Public
    }
}

fn extract_python_symbols(node: Node, source: &str, symbols: &mut Vec<ParsedSymbol>) {
    match node.kind() {
        "function_definition" => {
            if let Some(name) = node.child_by_field_name("name") {
                let name_text = node_text(name, source);
                let is_async = node.children(&mut node.walk()).any(|c| c.kind() == "async");
                let sig = extract_function_signature(node, source, is_async);
                symbols.push(
                    ParsedSymbol::new(name_text.clone(), SymbolKind::Function, node_location(node))
                        .with_signature(sig)
                        .with_visibility(detect_visibility(&name_text)),
                );
            }
        }
        "class_definition" => {
            if let Some(name) = node.child_by_field_name("name") {
                let name_text = node_text(name, source);
                let mut children = Vec::new();
                if let Some(body) = node.child_by_field_name("body") {
                    for i in 0..body.child_count() {
                        if let Some(child) = body.child(i)
                            && child.kind() == "function_definition"
                                && let Some(method_name) = child.child_by_field_name("name") {
                                    let method_name_text = node_text(method_name, source);
                                    children.push(
                                        ParsedSymbol::new(
                                            method_name_text.clone(),
                                            SymbolKind::Method,
                                            node_location(child),
                                        )
                                        .with_visibility(detect_visibility(&method_name_text)),
                                    );
                                }
                    }
                }
                symbols.push(
                    ParsedSymbol::new(name_text, SymbolKind::Class, node_location(node))
                        .with_children(children),
                );
            }
        }
        "decorated_definition" => {
            if let Some(def) = node.child_by_field_name("definition") {
                extract_python_symbols(def, source, symbols);
            }
        }
        _ => {
            for i in 0..node.child_count() {
                if let Some(child) = node.child(i) {
                    extract_python_symbols(child, source, symbols);
                }
            }
        }
    }
}

fn extract_function_signature(node: Node, source: &str, is_async: bool) -> String {
    let name = node
        .child_by_field_name("name")
        .map(|n| node_text(n, source))
        .unwrap_or_default();
    let params = node
        .child_by_field_name("parameters").map_or_else(|| "()".to_string(), |n| node_text(n, source));
    let ret = node
        .child_by_field_name("return_type")
        .map(|n| format!(" -> {}", node_text(n, source)))
        .unwrap_or_default();
    let prefix = if is_async { "async def " } else { "def " };
    format!("{prefix}{name}{params}{ret}")
}

fn collect_python_references(node: Node, source: &str, refs: &mut Vec<ParsedReference>) {
    match node.kind() {
        "call" => {
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
        "import_statement" | "import_from_statement" => {
            if let Some(module) = node.child_by_field_name("module_name") {
                refs.push(ParsedReference::new(
                    node_text(module, source),
                    ReferenceKind::Import,
                    node_location(module),
                ));
            }
        }
        "class_definition" => {
            if let Some(superclasses) = node.child_by_field_name("superclasses") {
                for i in 0..superclasses.child_count() {
                    if let Some(child) = superclasses.child(i)
                        && (child.kind() == "identifier" || child.kind() == "attribute") {
                            refs.push(ParsedReference::new(
                                node_text(child, source),
                                ReferenceKind::Inheritance,
                                node_location(child),
                            ));
                        }
                }
            }
        }
        "attribute" => {
            if let Some(attr) = node.child_by_field_name("attribute") {
                refs.push(ParsedReference::new(
                    node_text(attr, source),
                    ReferenceKind::FieldAccess,
                    node_location(attr),
                ));
            }
        }
        _ => {}
    }
    for i in 0..node.child_count() {
        if let Some(child) = node.child(i) {
            collect_python_references(child, source, refs);
        }
    }
}

fn is_builtin(name: &str) -> bool {
    matches!(
        name,
        "print"
            | "len"
            | "range"
            | "enumerate"
            | "zip"
            | "map"
            | "filter"
            | "sum"
            | "min"
            | "max"
            | "abs"
            | "sorted"
            | "reversed"
            | "open"
            | "input"
            | "int"
            | "float"
            | "str"
            | "bool"
            | "list"
            | "dict"
            | "set"
            | "tuple"
            | "type"
            | "isinstance"
            | "hasattr"
            | "getattr"
            | "setattr"
            | "super"
    )
}
