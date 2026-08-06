//! C/C++ language analyzer implementation.

use tree_sitter::{Node, Tree};

use crate::parser::treesitter::analyzers::LanguageAnalyzer;
use crate::types::{
    Location, ParsedReference, ParsedSymbol, ReferenceKind, SymbolKind, Visibility,
};

/// Internal symbol kind (ABI-independent)
#[derive(Debug, Clone, Copy)]
pub enum InternalSymbolKind {
    Function,
    Method,
    Class,
    Struct,
    Enum,
    Namespace,
    Field,
}

/// Internal reference kind (ABI-independent)
#[derive(Debug, Clone, Copy)]
pub enum InternalReferenceKind {
    Call,
    Import,
    TypeReference,
    FieldAccess,
    Inheritance,
}

/// Internal location (ABI-independent)
#[derive(Debug, Clone)]
pub struct InternalLocation {
    pub start_line: u32,
    pub start_col: u32,
    pub end_line: u32,
    pub end_col: u32,
    pub start_byte: u32,
    pub end_byte: u32,
}

/// Internal symbol (ABI-independent)
#[derive(Debug, Clone)]
pub struct InternalSymbol {
    pub name: String,
    pub kind: InternalSymbolKind,
    pub location: InternalLocation,
    pub signature: Option<String>,
    pub children: Vec<InternalSymbol>,
}

/// Internal reference (ABI-independent)
#[derive(Debug, Clone)]
pub struct InternalReference {
    pub name: String,
    pub kind: InternalReferenceKind,
    pub location: InternalLocation,
}

/// The C++ grammar.
#[must_use]
pub fn cpp_language() -> tree_sitter::Language {
    tree_sitter_cpp::LANGUAGE.into()
}

/// The C grammar. C and C++ share every node kind this module reads, so one walk serves both.
#[must_use]
pub fn c_language() -> tree_sitter::Language {
    tree_sitter_c::LANGUAGE.into()
}

#[derive(Debug)]
pub struct CppAnalyzer;

impl LanguageAnalyzer for CppAnalyzer {
    fn extract_symbols(&self, source: &str, tree: &Tree) -> Vec<ParsedSymbol> {
        let mut symbols = Vec::new();
        extract_cpp_symbols(tree.root_node(), source, &mut symbols);
        symbols.into_iter().map(convert_symbol).collect()
    }

    fn extract_references(&self, source: &str, tree: &Tree) -> Vec<ParsedReference> {
        let mut refs = Vec::new();
        collect_cpp_references(tree.root_node(), source, &mut refs);
        refs.into_iter().map(convert_reference).collect()
    }
}

fn convert_location(loc: InternalLocation) -> Location {
    Location {
        start_line: loc.start_line,
        start_col: loc.start_col,
        end_line: loc.end_line,
        end_col: loc.end_col,
        start_byte: loc.start_byte,
        end_byte: loc.end_byte,
    }
}

fn convert_symbol(sym: InternalSymbol) -> ParsedSymbol {
    ParsedSymbol {
        name: sym.name,
        kind: convert_symbol_kind(sym.kind),
        location: convert_location(sym.location),
        signature: sym.signature,
        doc_comment: None,
        children: sym.children.into_iter().map(convert_symbol).collect(),
        visibility: Visibility::Unknown,
    }
}

fn convert_reference(r: InternalReference) -> ParsedReference {
    ParsedReference {
        name: r.name,
        kind: convert_reference_kind(r.kind),
        location: convert_location(r.location),
        containing_symbol_index: None,
    }
}

fn convert_symbol_kind(kind: InternalSymbolKind) -> SymbolKind {
    match kind {
        InternalSymbolKind::Function => SymbolKind::Function,
        InternalSymbolKind::Method => SymbolKind::Method,
        InternalSymbolKind::Class => SymbolKind::Class,
        InternalSymbolKind::Struct => SymbolKind::Struct,
        InternalSymbolKind::Enum => SymbolKind::Enum,
        InternalSymbolKind::Namespace => SymbolKind::Namespace,
        InternalSymbolKind::Field => SymbolKind::Field,
    }
}

fn convert_reference_kind(kind: InternalReferenceKind) -> ReferenceKind {
    match kind {
        InternalReferenceKind::Call => ReferenceKind::Call,
        InternalReferenceKind::Import => ReferenceKind::Import,
        InternalReferenceKind::TypeReference => ReferenceKind::TypeReference,
        InternalReferenceKind::FieldAccess => ReferenceKind::FieldAccess,
        InternalReferenceKind::Inheritance => ReferenceKind::Inheritance,
    }
}

fn node_text(node: Node, source: &str) -> String {
    source[node.byte_range()].to_string()
}

fn node_location(node: Node) -> InternalLocation {
    let start = node.start_position();
    let end = node.end_position();
    InternalLocation {
        start_line: start.row as u32,
        start_col: start.column as u32,
        end_line: end.row as u32,
        end_col: end.column as u32,
        start_byte: node.start_byte() as u32,
        end_byte: node.end_byte() as u32,
    }
}

fn extract_cpp_symbols(node: Node, source: &str, symbols: &mut Vec<InternalSymbol>) {
    match node.kind() {
        "function_definition" => {
            if let Some(declarator) = node.child_by_field_name("declarator")
                && let Some(name) = extract_function_name(declarator, source) {
                    let sig = extract_signature(node, source);
                    symbols.push(InternalSymbol {
                        name,
                        kind: InternalSymbolKind::Function,
                        location: node_location(node),
                        signature: Some(sig),
                        children: vec![],
                    });
                }
        }
        "class_specifier" => {
            if let Some(name) = node.child_by_field_name("name") {
                let mut children = Vec::new();
                if let Some(body) = node.child_by_field_name("body") {
                    collect_class_members(body, source, &mut children);
                }
                symbols.push(InternalSymbol {
                    name: node_text(name, source),
                    kind: InternalSymbolKind::Class,
                    location: node_location(node),
                    signature: None,
                    children,
                });
            }
        }
        "struct_specifier" => {
            if let Some(name) = node.child_by_field_name("name") {
                let mut children = Vec::new();
                if let Some(body) = node.child_by_field_name("body") {
                    collect_class_members(body, source, &mut children);
                }
                symbols.push(InternalSymbol {
                    name: node_text(name, source),
                    kind: InternalSymbolKind::Struct,
                    location: node_location(node),
                    signature: None,
                    children,
                });
            }
        }
        "enum_specifier" => {
            if let Some(name) = node.child_by_field_name("name") {
                symbols.push(InternalSymbol {
                    name: node_text(name, source),
                    kind: InternalSymbolKind::Enum,
                    location: node_location(node),
                    signature: None,
                    children: vec![],
                });
            }
        }
        "namespace_definition" => {
            if let Some(name) = node.child_by_field_name("name") {
                symbols.push(InternalSymbol {
                    name: node_text(name, source),
                    kind: InternalSymbolKind::Namespace,
                    location: node_location(node),
                    signature: None,
                    children: vec![],
                });
            }
        }
        "template_declaration" => {
            if let Some(declaration) = node.child_by_field_name("declaration") {
                extract_cpp_symbols(declaration, source, symbols);
            }
        }
        _ => {
            for i in 0..node.child_count() {
                if let Some(child) = node.child(i) {
                    extract_cpp_symbols(child, source, symbols);
                }
            }
        }
    }
}

fn collect_class_members(body: Node, source: &str, children: &mut Vec<InternalSymbol>) {
    for i in 0..body.child_count() {
        if let Some(child) = body.child(i) {
            match child.kind() {
                "function_definition" | "declaration" => {
                    if let Some(declarator) = child.child_by_field_name("declarator")
                        && let Some(name) = extract_function_name(declarator, source) {
                            children.push(InternalSymbol {
                                name,
                                kind: InternalSymbolKind::Method,
                                location: node_location(child),
                                signature: None,
                                children: vec![],
                            });
                        }
                }
                "field_declaration" => {
                    for j in 0..child.child_count() {
                        if let Some(field) = child.child(j)
                            && field.kind() == "field_identifier" {
                                children.push(InternalSymbol {
                                    name: node_text(field, source),
                                    kind: InternalSymbolKind::Field,
                                    location: node_location(field),
                                    signature: None,
                                    children: vec![],
                                });
                            }
                    }
                }
                _ => {}
            }
        }
    }
}

fn extract_function_name(declarator: Node, source: &str) -> Option<String> {
    match declarator.kind() {
        "function_declarator" => {
            if let Some(decl) = declarator.child_by_field_name("declarator") {
                return extract_function_name(decl, source);
            }
        }
        "pointer_declarator" | "reference_declarator" => {
            if let Some(decl) = declarator.child_by_field_name("declarator") {
                return extract_function_name(decl, source);
            }
        }
        "qualified_identifier" => {
            if let Some(name) = declarator.child_by_field_name("name") {
                return Some(node_text(name, source));
            }
        }
        "identifier" | "field_identifier" | "destructor_name" | "operator_name" => {
            return Some(node_text(declarator, source));
        }
        _ => {}
    }
    None
}

fn extract_signature(node: Node, source: &str) -> String {
    let text = node_text(node, source);
    if let Some(brace) = text.find('{') {
        text[..brace].trim().to_string()
    } else if let Some(semi) = text.find(';') {
        text[..semi].trim().to_string()
    } else {
        text.lines()
            .take(3)
            .collect::<Vec<_>>()
            .join(" ")
            .trim()
            .to_string()
    }
}

fn collect_cpp_references(node: Node, source: &str, refs: &mut Vec<InternalReference>) {
    match node.kind() {
        "call_expression" => {
            if let Some(func) = node.child_by_field_name("function") {
                let name = node_text(func, source);
                if !is_builtin(&name) {
                    refs.push(InternalReference {
                        name,
                        kind: InternalReferenceKind::Call,
                        location: node_location(func),
                    });
                }
            }
        }
        "preproc_include" => {
            if let Some(path) = node.child_by_field_name("path") {
                let include = node_text(path, source)
                    .trim_matches(|c| c == '"' || c == '<' || c == '>')
                    .to_string();
                refs.push(InternalReference {
                    name: include,
                    kind: InternalReferenceKind::Import,
                    location: node_location(path),
                });
            }
        }
        "type_identifier" => {
            let name = node_text(node, source);
            if !is_primitive(&name) {
                refs.push(InternalReference {
                    name,
                    kind: InternalReferenceKind::TypeReference,
                    location: node_location(node),
                });
            }
        }
        "field_expression" => {
            if let Some(field) = node.child_by_field_name("field") {
                refs.push(InternalReference {
                    name: node_text(field, source),
                    kind: InternalReferenceKind::FieldAccess,
                    location: node_location(field),
                });
            }
        }
        "class_specifier" | "struct_specifier" => {
            if let Some(base) = node.child_by_field_name("base_clause") {
                for i in 0..base.child_count() {
                    if let Some(child) = base.child(i)
                        && child.kind() == "base_class_clause"
                            && let Some(type_node) = child.child_by_field_name("type") {
                                refs.push(InternalReference {
                                    name: node_text(type_node, source),
                                    kind: InternalReferenceKind::Inheritance,
                                    location: node_location(type_node),
                                });
                            }
                }
            }
        }
        _ => {}
    }
    for i in 0..node.child_count() {
        if let Some(child) = node.child(i) {
            collect_cpp_references(child, source, refs);
        }
    }
}

fn is_builtin(name: &str) -> bool {
    matches!(
        name,
        "std::cout" | "std::cerr" | "std::endl" | "printf" | "malloc" | "free" | "sizeof"
    )
}

fn is_primitive(name: &str) -> bool {
    matches!(
        name,
        "int"
            | "char"
            | "float"
            | "double"
            | "bool"
            | "void"
            | "long"
            | "short"
            | "unsigned"
            | "signed"
            | "size_t"
            | "auto"
    )
}
