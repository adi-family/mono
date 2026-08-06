//! Lua language analyzer implementation.

use tree_sitter::{Node, Tree};

use crate::parser::treesitter::analyzers::LanguageAnalyzer;
use crate::types::{Location, ParsedReference, ParsedSymbol, ReferenceKind, SymbolKind, Visibility};

/// The grammar this module analyses.
#[must_use]
pub fn language() -> tree_sitter::Language {
    tree_sitter_lua::LANGUAGE.into()
}

#[derive(Debug)]
pub struct LuaAnalyzer;

impl LanguageAnalyzer for LuaAnalyzer {
    fn extract_symbols(&self, source: &str, tree: &Tree) -> Vec<ParsedSymbol> {
        let mut symbols = Vec::new();
        extract_lua_symbols(tree.root_node(), source, &mut symbols);
        symbols
    }

    fn extract_references(&self, source: &str, tree: &Tree) -> Vec<ParsedReference> {
        let mut refs = Vec::new();
        collect_lua_references(tree.root_node(), source, &mut refs);
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
        if sibling.kind() == "comment" {
            let text = node_text(sibling, source);
            if text.starts_with("---") || text.starts_with("--[[") {
                comments.push(
                    text.trim_start_matches("---")
                        .trim_start_matches("--[[")
                        .trim_end_matches("]]")
                        .trim()
                        .to_string(),
                );
            } else if text.starts_with("--") {
                comments.push(text.trim_start_matches("--").trim().to_string());
            } else {
                break;
            }
        } else {
            break;
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

fn extract_function_signature(node: Node, source: &str) -> String {
    let text = node_text(node, source);
    if let Some(newline_pos) = text.find('\n') {
        text[..newline_pos].trim().to_string()
    } else {
        text.trim().to_string()
    }
}

fn extract_lua_symbols(node: Node, source: &str, symbols: &mut Vec<ParsedSymbol>) {
    match node.kind() {
        "function_declaration" => {
            if let Some(symbol) = parse_lua_function(node, source) {
                symbols.push(symbol);
            }
        }
        "local_function_declaration" => {
            if let Some(symbol) = parse_lua_local_function(node, source) {
                symbols.push(symbol);
            }
        }
        "variable_declaration" | "local_variable_declaration" => {
            parse_lua_variables(node, source, symbols);
        }
        "assignment_statement" => {
            parse_lua_assignment(node, source, symbols);
        }
        _ => {}
    }

    for i in 0..node.child_count() {
        if let Some(child) = node.child(i) {
            extract_lua_symbols(child, source, symbols);
        }
    }
}

fn parse_lua_function(node: Node, source: &str) -> Option<ParsedSymbol> {
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

fn parse_lua_local_function(node: Node, source: &str) -> Option<ParsedSymbol> {
    let name = node.child_by_field_name("name")?;
    let name_text = node_text(name, source);
    let doc_comment = extract_doc_comment(node, source);
    let signature = extract_function_signature(node, source);

    Some(
        ParsedSymbol::new(name_text, SymbolKind::Function, node_location(node))
            .with_signature(signature)
            .with_visibility(Visibility::Private)
            .with_doc_comment_opt(doc_comment),
    )
}

fn parse_lua_variables(node: Node, source: &str, symbols: &mut Vec<ParsedSymbol>) {
    let is_local = node.kind() == "local_variable_declaration";
    let doc_comment = extract_doc_comment(node, source);

    for i in 0..node.child_count() {
        if let Some(child) = node.child(i)
            && (child.kind() == "variable_list" || child.kind() == "identifier") {
                let visibility = if is_local {
                    Visibility::Private
                } else {
                    Visibility::Public
                };

                if child.kind() == "identifier" {
                    let name_text = node_text(child, source);
                    symbols.push(
                        ParsedSymbol::new(name_text, SymbolKind::Variable, node_location(child))
                            .with_visibility(visibility)
                            .with_doc_comment_opt(doc_comment.clone()),
                    );
                } else {
                    for j in 0..child.child_count() {
                        if let Some(var) = child.child(j)
                            && var.kind() == "identifier" {
                                let name_text = node_text(var, source);
                                symbols.push(
                                    ParsedSymbol::new(
                                        name_text,
                                        SymbolKind::Variable,
                                        node_location(var),
                                    )
                                    .with_visibility(visibility)
                                    .with_doc_comment_opt(doc_comment.clone()),
                                );
                            }
                    }
                }
            }
    }
}

fn parse_lua_assignment(node: Node, source: &str, symbols: &mut Vec<ParsedSymbol>) {
    // Check if right side is a function expression - then it's a function assignment
    let mut is_function = false;
    for i in 0..node.child_count() {
        if let Some(child) = node.child(i)
            && child.kind() == "expression_list" {
                for j in 0..child.child_count() {
                    if let Some(expr) = child.child(j)
                        && expr.kind() == "function_definition" {
                            is_function = true;
                            break;
                        }
                }
            }
    }

    if !is_function {
        return;
    }

    let doc_comment = extract_doc_comment(node, source);
    let signature = extract_function_signature(node, source);

    for i in 0..node.child_count() {
        if let Some(child) = node.child(i)
            && child.kind() == "variable_list" {
                for j in 0..child.child_count() {
                    if let Some(var) = child.child(j) {
                        let name_text = node_text(var, source);
                        symbols.push(
                            ParsedSymbol::new(name_text, SymbolKind::Function, node_location(node))
                                .with_signature(signature.clone())
                                .with_visibility(Visibility::Public)
                                .with_doc_comment_opt(doc_comment.clone()),
                        );
                    }
                }
            }
    }
}

fn collect_lua_references(node: Node, source: &str, refs: &mut Vec<ParsedReference>) {
    match node.kind() {
        "function_call" => {
            if let Some(name) = node.child_by_field_name("name") {
                let name_text = extract_function_call_name(name, source);
                if !name_text.is_empty() && !is_builtin_function(&name_text) {
                    refs.push(ParsedReference::new(
                        name_text,
                        ReferenceKind::Call,
                        node_location(name),
                    ));
                }
            }
        }
        "method_index_expression" => {
            if let Some(method) = node.child_by_field_name("method") {
                refs.push(ParsedReference::new(
                    node_text(method, source),
                    ReferenceKind::Call,
                    node_location(method),
                ));
            }
        }
        "dot_index_expression" | "bracket_index_expression" => {
            if let Some(field) = node.child_by_field_name("field") {
                refs.push(ParsedReference::new(
                    node_text(field, source),
                    ReferenceKind::FieldAccess,
                    node_location(field),
                ));
            }
        }
        "identifier" => {
            let parent = node.parent();
            if let Some(p) = parent {
                // Skip if this identifier is being defined
                if p.kind() != "function_declaration"
                    && p.kind() != "local_function_declaration"
                    && p.kind() != "variable_list"
                    && p.kind() != "parameter_list"
                {
                    let name = node_text(node, source);
                    if !is_keyword(&name) {
                        refs.push(ParsedReference::new(
                            name,
                            ReferenceKind::VariableReference,
                            node_location(node),
                        ));
                    }
                }
            }
        }
        _ => {}
    }

    for i in 0..node.child_count() {
        if let Some(child) = node.child(i) {
            collect_lua_references(child, source, refs);
        }
    }
}

fn extract_function_call_name(node: Node, source: &str) -> String {
    match node.kind() {
        "identifier" => node_text(node, source),
        "dot_index_expression" => node_text(node, source),
        "method_index_expression" => node_text(node, source),
        _ => String::new(),
    }
}

fn is_builtin_function(name: &str) -> bool {
    matches!(
        name,
        "print"
            | "type"
            | "pairs"
            | "ipairs"
            | "next"
            | "tostring"
            | "tonumber"
            | "error"
            | "assert"
            | "pcall"
            | "xpcall"
            | "require"
            | "dofile"
            | "loadfile"
            | "load"
            | "loadstring"
            | "setmetatable"
            | "getmetatable"
            | "rawget"
            | "rawset"
            | "rawequal"
            | "select"
            | "unpack"
            | "table.insert"
            | "table.remove"
            | "table.concat"
    )
}

fn is_keyword(name: &str) -> bool {
    matches!(
        name,
        "and"
            | "break"
            | "do"
            | "else"
            | "elseif"
            | "end"
            | "false"
            | "for"
            | "function"
            | "goto"
            | "if"
            | "in"
            | "local"
            | "nil"
            | "not"
            | "or"
            | "repeat"
            | "return"
            | "then"
            | "true"
            | "until"
            | "while"
            | "self"
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
