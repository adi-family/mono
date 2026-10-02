//! TypeScript/JavaScript language analyzer implementation.

use std::collections::HashSet;
use tree_sitter::{Node, Tree};

use super::common::{node_location, node_text, tree_walking_analyzer};
use crate::parser::treesitter::analyzers::LanguageAnalyzer;
use crate::types::{ParsedReference, ParsedSymbol, ReferenceKind, SymbolKind, Visibility};

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
        "program" => {
            let start = symbols.len();
            extract_children(node, source, symbols);
            // An export list can precede its declaration. Resolve only names declared in this
            // module; reexports from another module must not mark a same-named local public.
            let names = local_export_names(node, source);
            for symbol in &mut symbols[start..] {
                if names.contains(&symbol.name) {
                    symbol.visibility = Visibility::Public;
                }
            }
        }
        "function_declaration"
        | "generator_function_declaration"
        | "function_signature"
        | "function" => {
            if let Some(name) = node.child_by_field_name("name") {
                symbols.push(callable_symbol(
                    node,
                    node,
                    &node_text(name, source),
                    SymbolKind::Function,
                    source,
                ));
            }
        }
        "variable_declarator" => {
            // Arrow functions and function expressions are named by the binding, not an
            // optional internal function-expression name (`const run = function inner() {}`).
            if let Some(name) = node.child_by_field_name("name")
                && name.kind() == "identifier"
            {
                let name = node_text(name, source);
                let value = node.child_by_field_name("value").map(unwrap_expression);
                if let Some(function) = value.and_then(function_value) {
                    symbols.push(callable_symbol(
                        function,
                        node,
                        &name,
                        SymbolKind::Function,
                        source,
                    ));
                } else if let Some(class) = value.filter(|value| value.kind() == "class") {
                    let mut symbol = class_symbol(class, &name, source);
                    symbol.location = node_location(node);
                    symbols.push(symbol);
                } else {
                    let kind = if node
                        .parent()
                        .and_then(|parent| parent.child_by_field_name("kind"))
                        .is_some_and(|kind| kind.kind() == "const")
                    {
                        SymbolKind::Constant
                    } else {
                        SymbolKind::Variable
                    };
                    let mut children = Vec::new();
                    if let Some(value) = value {
                        extract_ts_symbols(value, source, &mut children);
                    }
                    symbols.push(
                        ParsedSymbol::new(name, kind, node_location(node)).with_children(children),
                    );
                }
            } else {
                extract_children(node, source, symbols);
            }
        }
        "class_declaration" | "abstract_class_declaration" | "class" => {
            if let Some(name) = node.child_by_field_name("name") {
                symbols.push(class_symbol(node, &node_text(name, source), source));
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
                let start = symbols.len();
                if let Some(value) = node.child_by_field_name("value") {
                    let value = unwrap_expression(value);
                    // A default identifier exports an existing binding. The program pass
                    // marks it public; inventing another symbol would duplicate its identity.
                    if value.kind() != "identifier" {
                        let name = value
                            .child_by_field_name("name")
                            .map_or_else(|| "default".to_string(), |name| node_text(name, source));
                        if let Some(function) = function_value(value) {
                            symbols.push(callable_symbol(
                                function,
                                value,
                                &name,
                                SymbolKind::Function,
                                source,
                            ));
                        } else if value.kind() == "class" {
                            symbols.push(class_symbol(value, &name, source));
                        } else {
                            let mut children = Vec::new();
                            extract_ts_symbols(value, source, &mut children);
                            symbols.push(
                                ParsedSymbol::new(
                                    "default",
                                    SymbolKind::Constant,
                                    node_location(value),
                                )
                                .with_children(children),
                            );
                        }
                    }
                } else {
                    extract_children(node, source, symbols);
                }
                for symbol in &mut symbols[start..] {
                    symbol.visibility = Visibility::Public;
                }
            }
        }
        _ => extract_children(node, source, symbols),
    }
}

fn extract_children(node: Node, source: &str, symbols: &mut Vec<ParsedSymbol>) {
    for child in node.named_children(&mut node.walk()) {
        extract_ts_symbols(child, source, symbols);
    }
}

fn callable_symbol(
    function: Node,
    location: Node,
    name: &str,
    kind: SymbolKind,
    source: &str,
) -> ParsedSymbol {
    let mut children = Vec::new();
    if let Some(body) = function.child_by_field_name("body") {
        extract_ts_symbols(body, source, &mut children);
    }
    ParsedSymbol::new(name, kind, node_location(location))
        .with_signature(extract_function_signature(function, source, name))
        .with_children(children)
}

fn class_symbol(node: Node, name: &str, source: &str) -> ParsedSymbol {
    let mut children = Vec::new();
    if let Some(body) = node.child_by_field_name("body") {
        for member in body.named_children(&mut body.walk()) {
            let Some(name) = member
                .child_by_field_name("name")
                .or_else(|| member.child_by_field_name("property"))
            else {
                continue;
            };
            let function = match member.kind() {
                "method_definition" | "method_signature" | "abstract_method_signature" => {
                    Some(member)
                }
                "public_field_definition" | "field_definition" => {
                    member.child_by_field_name("value").and_then(function_value)
                }
                _ => None,
            };
            if let Some(function) = function {
                children.push(
                    callable_symbol(
                        function,
                        member,
                        &node_text(name, source),
                        SymbolKind::Method,
                        source,
                    )
                    .with_visibility(member_visibility(member, name, source)),
                );
            }
        }
    }
    ParsedSymbol::new(name, SymbolKind::Class, node_location(node)).with_children(children)
}

fn member_visibility(node: Node, name: Node, source: &str) -> Visibility {
    if name.kind() == "private_property_identifier" {
        return Visibility::Private;
    }
    for modifier in node.named_children(&mut node.walk()) {
        if modifier.kind() == "accessibility_modifier" {
            return match node_text(modifier, source).as_str() {
                "private" => Visibility::Private,
                "protected" => Visibility::Protected,
                _ => Visibility::Public,
            };
        }
    }
    Visibility::Public
}

fn local_export_names(program: Node, source: &str) -> HashSet<String> {
    let mut names = HashSet::new();
    for export in program.named_children(&mut program.walk()) {
        if export.kind() != "export_statement" || export.child_by_field_name("source").is_some() {
            continue;
        }
        if let Some(value) = export.child_by_field_name("value") {
            let value = unwrap_expression(value);
            if value.kind() == "identifier" {
                names.insert(node_text(value, source));
            }
        }
        for clause in export.named_children(&mut export.walk()) {
            if clause.kind() == "export_clause" {
                for specifier in clause.named_children(&mut clause.walk()) {
                    if let Some(name) = specifier.child_by_field_name("name") {
                        names.insert(node_text(name, source));
                    }
                }
            }
        }
    }
    names
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

fn unwrap_expression(mut node: Node) -> Node {
    loop {
        let child = match node.kind() {
            "parenthesized_expression"
            | "as_expression"
            | "satisfies_expression"
            | "non_null_expression" => node.named_child(0),
            // Angle-bracket assertions put their type arguments before the expression.
            "type_assertion" => node.named_child(1),
            _ => None,
        };
        match child {
            Some(child) => node = child,
            None => return node,
        }
    }
}

fn function_value(node: Node) -> Option<Node> {
    let node = unwrap_expression(node);
    matches!(
        node.kind(),
        "arrow_function" | "function_expression" | "generator_function"
    )
    .then_some(node)
}

fn extract_function_signature(node: Node, source: &str, name: &str) -> String {
    let mut parts = vec![name.to_string()];
    if let Some(generics) = node.child_by_field_name("type_parameters") {
        parts.push(node_text(generics, source));
    }
    if let Some(params) = node.child_by_field_name("parameters") {
        parts.push(node_text(params, source));
    } else if let Some(param) = node.child_by_field_name("parameter") {
        parts.push(format!("({})", node_text(param, source)));
    }
    if let Some(ret) = node.child_by_field_name("return_type") {
        // The type annotation already includes its leading colon.
        parts.push(node_text(ret, source));
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
        "import_statement" | "export_statement" => {
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
    for i in 0..node.child_count() as u32 {
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
