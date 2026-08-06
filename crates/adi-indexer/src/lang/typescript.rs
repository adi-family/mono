//! TypeScript/JavaScript language analyzer implementation.

use tree_sitter::{Node, Tree};

use super::common::{node_location, node_text, tree_walking_analyzer};
use crate::parser::treesitter::analyzers::LanguageAnalyzer;
use crate::types::{ParsedReference, ParsedSymbol, ReferenceKind, SymbolKind};

/// The grammar this module analyses.
#[must_use]
pub fn language() -> tree_sitter::Language {
    tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into()
}

#[derive(Debug)]
pub struct TypeScriptAnalyzer;

tree_walking_analyzer!(
    TypeScriptAnalyzer,
    symbols: extract_ts_symbols,
    references: collect_ts_references,
);

fn extract_ts_symbols(node: Node, source: &str, symbols: &mut Vec<ParsedSymbol>) {
    match node.kind() {
        "function_declaration" | "function" => {
            if let Some(name) = node.child_by_field_name("name") {
                let name_text = node_text(name, source);
                let sig = extract_function_signature(node, source);
                symbols.push(
                    ParsedSymbol::new(name_text, SymbolKind::Function, node_location(node))
                        .with_signature(sig),
                );
            }
        }
        "class_declaration" | "class" => {
            if let Some(name) = node.child_by_field_name("name") {
                let name_text = node_text(name, source);
                let mut children = Vec::new();
                if let Some(body) = node.child_by_field_name("body") {
                    for i in 0..body.child_count() {
                        if let Some(child) = body.child(i)
                            && child.kind() == "method_definition"
                                && let Some(method_name) = child.child_by_field_name("name") {
                                    children.push(ParsedSymbol::new(
                                        node_text(method_name, source),
                                        SymbolKind::Method,
                                        node_location(child),
                                    ));
                                }
                    }
                }
                symbols.push(
                    ParsedSymbol::new(name_text, SymbolKind::Class, node_location(node))
                        .with_children(children),
                );
            }
        }
        "interface_declaration" => {
            if let Some(name) = node.child_by_field_name("name") {
                symbols.push(ParsedSymbol::new(
                    node_text(name, source),
                    SymbolKind::Interface,
                    node_location(node),
                ));
            }
        }
        "type_alias_declaration" => {
            if let Some(name) = node.child_by_field_name("name") {
                symbols.push(ParsedSymbol::new(
                    node_text(name, source),
                    SymbolKind::Type,
                    node_location(node),
                ));
            }
        }
        "enum_declaration" => {
            if let Some(name) = node.child_by_field_name("name") {
                symbols.push(ParsedSymbol::new(
                    node_text(name, source),
                    SymbolKind::Enum,
                    node_location(node),
                ));
            }
        }
        // Module-level imports / re-exports. Lets `whois(file, line)`
        // answer "this line is inside an import" so consumers like
        // `search` can drop import lines from results when they only
        // want use-sites.
        //
        // Covered shapes (tree-sitter-typescript node kinds):
        //   - `import { x } from '…'`           → import_statement
        //   - `import x from '…'` / `import * as x from '…'`
        //   - `import '…'` (side-effect)
        //   - `import type { X } from '…'`
        //   - `import x = require('…')`         → import_alias
        //   - `export { x } from '…'`           → export_statement w/
        //                                          `source` field set
        //   - `export * from '…'` / `export type { X } from '…'`
        //
        // Name: prefer the source module string ('next-translate') so
        // consumers can do "who imports X?". Falls back to the raw
        // node text when we can't extract one.
        "import_statement" | "import_alias" => {
            symbols.push(ParsedSymbol::new(
                import_source_or_text(node, source),
                SymbolKind::Import,
                node_location(node),
            ));
        }
        "export_statement" => {
            // Only mark `export … from '…'` (re-export). Plain
            // `export const x = …` is real code, not an import.
            if node.child_by_field_name("source").is_some() {
                symbols.push(ParsedSymbol::new(
                    import_source_or_text(node, source),
                    SymbolKind::Import,
                    node_location(node),
                ));
            } else {
                // Recurse into the export so e.g. `export class Foo`
                // still emits a Class symbol.
                for i in 0..node.child_count() {
                    if let Some(child) = node.child(i) {
                        extract_ts_symbols(child, source, symbols);
                    }
                }
            }
        }
        _ => {
            for i in 0..node.child_count() {
                if let Some(child) = node.child(i) {
                    extract_ts_symbols(child, source, symbols);
                }
            }
        }
    }
}

/// Pull the source module string out of an `import_statement` or
/// `export_statement` node — that's the most useful name for a
/// later "who imports X?" query. Falls back to the raw declaration
/// text when no `source` child is present (e.g. `import './foo';`
/// where the source IS the literal already, but tree-sitter still
/// gives us a `source` field — handled).
fn import_source_or_text(node: Node, source: &str) -> String {
    if let Some(src) = node.child_by_field_name("source") {
        let raw = node_text(src, source);
        // Strip surrounding quotes so the stored name is the module
        // path itself (`next-translate`, not `'next-translate'`).
        return raw.trim_matches(|c| c == '\'' || c == '"').to_string();
    }
    node_text(node, source)
}

fn extract_function_signature(node: Node, source: &str) -> String {
    let mut parts = Vec::new();
    if let Some(name) = node.child_by_field_name("name") {
        parts.push(node_text(name, source));
    }
    if let Some(params) = node.child_by_field_name("parameters") {
        parts.push(node_text(params, source));
    }
    if let Some(ret) = node.child_by_field_name("return_type") {
        parts.push(format!(": {}", node_text(ret, source)));
    }
    parts.join("")
}

fn collect_ts_references(node: Node, source: &str, refs: &mut Vec<ParsedReference>) {
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
        "import_statement" => {
            if let Some(source_node) = node.child_by_field_name("source") {
                let module = node_text(source_node, source)
                    .trim_matches(|c| c == '"' || c == '\'')
                    .to_string();
                refs.push(ParsedReference::new(
                    module,
                    ReferenceKind::Import,
                    node_location(source_node),
                ));
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
        "member_expression" => {
            if let Some(prop) = node.child_by_field_name("property") {
                refs.push(ParsedReference::new(
                    node_text(prop, source),
                    ReferenceKind::FieldAccess,
                    node_location(prop),
                ));
            }
        }
        _ => {}
    }
    for i in 0..node.child_count() {
        if let Some(child) = node.child(i) {
            collect_ts_references(child, source, refs);
        }
    }
}

fn is_builtin(name: &str) -> bool {
    matches!(
        name,
        "console.log"
            | "console.error"
            | "JSON.parse"
            | "JSON.stringify"
            | "Object.keys"
            | "Array.isArray"
            | "Promise.all"
            | "Promise.resolve"
    )
}

fn is_primitive(name: &str) -> bool {
    matches!(
        name,
        "string"
            | "number"
            | "boolean"
            | "any"
            | "void"
            | "null"
            | "undefined"
            | "never"
            | "unknown"
            | "object"
    )
}
