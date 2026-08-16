//! Rust language analyzer implementation.

use tree_sitter::{Node, Tree};

use super::common::{
    declaration, node_location, node_text, signature_before, tree_walking_analyzer,
    WithDocCommentOpt,
};
use crate::parser::treesitter::analyzers::LanguageAnalyzer;
use crate::types::{ParsedReference, ParsedSymbol, ReferenceKind, SymbolKind, Visibility};

/// The grammar this module analyses.
#[must_use]
pub fn language() -> tree_sitter::Language {
    tree_sitter_rust::LANGUAGE.into()
}

#[derive(Debug)]
pub struct RustAnalyzer;

tree_walking_analyzer!(
    RustAnalyzer,
    symbols: extract_rust_symbols,
    references: collect_rust_references,
);

fn extract_doc_comment(node: Node, source: &str) -> Option<String> {
    let mut prev = node.prev_sibling();
    let mut comments = Vec::new();

    while let Some(sibling) = prev {
        if sibling.kind() == "line_comment" || sibling.kind() == "block_comment" {
            let text = node_text(sibling, source);
            if text.starts_with("///") || text.starts_with("//!") || text.starts_with("/**") {
                comments.push(text);
            } else {
                break;
            }
        } else if sibling.kind() == "attribute_item" || sibling.kind() == "inner_attribute_item" {
            // Skip attributes
        } else {
            break;
        }
        prev = sibling.prev_sibling();
    }

    if comments.is_empty() {
        None
    } else {
        comments.reverse();
        Some(
            comments
                .iter()
                .map(|c| c.trim_start_matches("///").trim_start_matches("//!").trim())
                .collect::<Vec<_>>()
                .join("\n"),
        )
    }
}

/// A brace opens a definition; a semicolon ends a trait method, a `const`, or a `use`.
fn extract_function_signature(node: Node, source: &str) -> String {
    signature_before(node, source, &["{", ";"])
}

fn extract_rust_symbols(node: Node, source: &str, symbols: &mut Vec<ParsedSymbol>) {
    let parsed = match node.kind() {
        "function_item" => parse_rust_callable(node, source, SymbolKind::Function),
        "struct_item" => parse_rust_struct(node, source),
        "trait_item" => parse_rust_trait(node, source),
        "enum_item" => parse_rust_declaration(node, source, SymbolKind::Enum),
        "mod_item" => parse_rust_declaration(node, source, SymbolKind::Module),
        "const_item" | "static_item" => parse_rust_declaration(node, source, SymbolKind::Constant),
        "type_item" => parse_rust_declaration(node, source, SymbolKind::Type),
        "macro_definition" => parse_rust_declaration(node, source, SymbolKind::Macro),
        "impl_item" => {
            parse_rust_impl(node, source, symbols);
            None
        }
        // Unlike the other analyzers here, only a node that names nothing is descended into: an
        // item's own body is read by the parser that matched it, not walked again from here.
        _ => {
            for i in 0..node.child_count() as u32 {
                if let Some(child) = node.child(i) {
                    extract_rust_symbols(child, source, symbols);
                }
            }
            None
        }
    };
    symbols.extend(parsed);
}

/// A Rust item named by its `name` field, preceded by its `///` block.
///
/// Visibility stays [`Visibility::Unknown`]: `pub` here is relative to a module path this walker
/// doesn't track, so recording `pub` as "public" would claim more than the tree says.
fn parse_rust_declaration(node: Node, source: &str, kind: SymbolKind) -> Option<ParsedSymbol> {
    declaration(
        node,
        source,
        kind,
        extract_doc_comment(node, source),
        Some(Visibility::Unknown),
        None,
    )
}

/// A Rust function — the same, plus the signature its body is stripped down to.
fn parse_rust_callable(node: Node, source: &str, kind: SymbolKind) -> Option<ParsedSymbol> {
    declaration(
        node,
        source,
        kind,
        extract_doc_comment(node, source),
        Some(Visibility::Unknown),
        Some(extract_function_signature(node, source)),
    )
}

fn parse_rust_struct(node: Node, source: &str) -> Option<ParsedSymbol> {
    let mut children = Vec::new();
    if let Some(body) = node.child_by_field_name("body") {
        for i in 0..body.child_count() as u32 {
            if let Some(child) = body.child(i)
                && child.kind() == "field_declaration"
                    && let Some(field_name) = child.child_by_field_name("name") {
                        children.push(
                            ParsedSymbol::new(
                                node_text(field_name, source),
                                SymbolKind::Field,
                                node_location(child),
                            )
                            .with_doc_comment_opt(extract_doc_comment(child, source)),
                        );
                    }
        }
    }

    Some(parse_rust_declaration(node, source, SymbolKind::Struct)?.with_children(children))
}

fn parse_rust_trait(node: Node, source: &str) -> Option<ParsedSymbol> {
    let mut children = Vec::new();
    if let Some(body) = node.child_by_field_name("body") {
        for i in 0..body.child_count() as u32 {
            if let Some(child) = body.child(i)
                && (child.kind() == "function_signature_item" || child.kind() == "function_item")
                    && let Some(method_name) = child.child_by_field_name("name") {
                        children.push(
                            ParsedSymbol::new(
                                node_text(method_name, source),
                                SymbolKind::Method,
                                node_location(child),
                            )
                            .with_signature(extract_function_signature(child, source))
                            .with_doc_comment_opt(extract_doc_comment(child, source)),
                        );
                    }
        }
    }

    Some(parse_rust_declaration(node, source, SymbolKind::Trait)?.with_children(children))
}

fn parse_rust_impl(node: Node, source: &str, symbols: &mut Vec<ParsedSymbol>) {
    let type_name = if let Some(trait_node) = node.child_by_field_name("trait") {
        if let Some(type_node) = node.child_by_field_name("type") {
            format!(
                "{} for {}",
                node_text(trait_node, source),
                node_text(type_node, source)
            )
        } else {
            node_text(trait_node, source)
        }
    } else if let Some(type_node) = node.child_by_field_name("type") {
        node_text(type_node, source)
    } else {
        return;
    };

    if let Some(body) = node.child_by_field_name("body") {
        for i in 0..body.child_count() as u32 {
            if let Some(child) = body.child(i)
                && child.kind() == "function_item"
                    && let Some(method_name) = child.child_by_field_name("name") {
                        symbols.push(
                            ParsedSymbol::new(
                                format!("{}::{}", type_name, node_text(method_name, source)),
                                SymbolKind::Method,
                                node_location(child),
                            )
                            .with_signature(extract_function_signature(child, source))
                            .with_doc_comment_opt(extract_doc_comment(child, source)),
                        );
                    }
        }
    }
}

fn collect_rust_references(node: Node, source: &str, refs: &mut Vec<ParsedReference>) {
    match node.kind() {
        "call_expression" => {
            if let Some(func) = node.child_by_field_name("function") {
                let name = extract_call_name(func, source);
                if !name.is_empty() && !is_builtin(&name) {
                    refs.push(ParsedReference::new(
                        name,
                        ReferenceKind::Call,
                        node_location(func),
                    ));
                }
            }
        }
        "method_call_expression" => {
            if let Some(method) = node.child_by_field_name("name") {
                let name = node_text(method, source);
                refs.push(ParsedReference::new(
                    name,
                    ReferenceKind::Call,
                    node_location(method),
                ));
            }
        }
        "macro_invocation" => {
            if let Some(macro_node) = node.child_by_field_name("macro") {
                let name = node_text(macro_node, source);
                if !is_std_macro(&name) {
                    refs.push(ParsedReference::new(
                        name,
                        ReferenceKind::MacroInvocation,
                        node_location(macro_node),
                    ));
                }
            }
        }
        "use_declaration" => {
            extract_use_references(node, source, refs);
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
        "scoped_type_identifier" => {
            let name = node_text(node, source);
            refs.push(ParsedReference::new(
                name,
                ReferenceKind::TypeReference,
                node_location(node),
            ));
        }
        "field_expression" => {
            if let Some(field) = node.child_by_field_name("field") {
                let name = node_text(field, source);
                refs.push(ParsedReference::new(
                    name,
                    ReferenceKind::FieldAccess,
                    node_location(field),
                ));
            }
        }
        "impl_item" => {
            if let Some(trait_node) = node.child_by_field_name("trait") {
                let trait_name = node_text(trait_node, source);
                refs.push(ParsedReference::new(
                    trait_name,
                    ReferenceKind::Inheritance,
                    node_location(trait_node),
                ));
            }
        }
        _ => {}
    }

    for i in 0..node.child_count() as u32 {
        if let Some(child) = node.child(i) {
            collect_rust_references(child, source, refs);
        }
    }
}

fn extract_use_references(node: Node, source: &str, refs: &mut Vec<ParsedReference>) {
    fn collect_use_paths(node: Node, source: &str, prefix: &str, refs: &mut Vec<ParsedReference>) {
        match node.kind() {
            "identifier" | "type_identifier" => {
                let name = node_text(node, source);
                let full_name = if prefix.is_empty() {
                    name
                } else {
                    format!("{prefix}::{name}")
                };
                refs.push(ParsedReference::new(
                    full_name,
                    ReferenceKind::Import,
                    node_location(node),
                ));
            }
            "scoped_identifier" => {
                let full_path = node_text(node, source);
                refs.push(ParsedReference::new(
                    full_path,
                    ReferenceKind::Import,
                    node_location(node),
                ));
            }
            "use_list" => {
                for i in 0..node.child_count() as u32 {
                    if let Some(child) = node.child(i) {
                        collect_use_paths(child, source, prefix, refs);
                    }
                }
            }
            "scoped_use_list" => {
                let new_prefix = if let Some(path) = node.child_by_field_name("path") {
                    let path_str = node_text(path, source);
                    if prefix.is_empty() {
                        path_str
                    } else {
                        format!("{prefix}::{path_str}")
                    }
                } else {
                    prefix.to_string()
                };
                if let Some(list) = node.child_by_field_name("list") {
                    collect_use_paths(list, source, &new_prefix, refs);
                }
            }
            "use_as_clause" => {
                if let Some(path) = node.child_by_field_name("path") {
                    collect_use_paths(path, source, prefix, refs);
                }
            }
            "use_wildcard" => {}
            _ => {
                for i in 0..node.child_count() as u32 {
                    if let Some(child) = node.child(i) {
                        collect_use_paths(child, source, prefix, refs);
                    }
                }
            }
        }
    }

    if let Some(arg) = node.child_by_field_name("argument") {
        collect_use_paths(arg, source, "", refs);
    }
}

fn extract_call_name(node: Node, source: &str) -> String {
    match node.kind() {
        "identifier" => node_text(node, source),
        "scoped_identifier" => node_text(node, source),
        "field_expression" => {
            if let Some(field) = node.child_by_field_name("field") {
                node_text(field, source)
            } else {
                String::new()
            }
        }
        _ => node_text(node, source),
    }
}

fn is_builtin(name: &str) -> bool {
    matches!(
        name,
        "drop" | "clone" | "default" | "from" | "into" | "as_ref" | "as_mut"
    )
}

fn is_std_macro(name: &str) -> bool {
    matches!(
        name,
        "println"
            | "print"
            | "eprintln"
            | "eprint"
            | "format"
            | "panic"
            | "assert"
            | "assert_eq"
            | "assert_ne"
            | "debug_assert"
            | "debug_assert_eq"
            | "debug_assert_ne"
            | "vec"
            | "write"
            | "writeln"
            | "todo"
            | "unimplemented"
            | "unreachable"
            | "cfg"
            | "env"
    )
}

fn is_primitive_type(name: &str) -> bool {
    matches!(
        name,
        "i8" | "i16"
            | "i32"
            | "i64"
            | "i128"
            | "isize"
            | "u8"
            | "u16"
            | "u32"
            | "u64"
            | "u128"
            | "usize"
            | "f32"
            | "f64"
            | "bool"
            | "char"
            | "str"
            | "String"
            | "Self"
            | "()"
            | "!"
    )
}

// Helper trait to add optional doc comment
