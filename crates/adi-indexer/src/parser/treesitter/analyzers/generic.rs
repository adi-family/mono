// Copyright (c) 2024-2025 Ihor
// SPDX-License-Identifier: BUSL-1.1
// See LICENSE file for details

use super::base::AnalyzerBase;
use super::LanguageAnalyzer;
use crate::types::{
    Language, ParsedReference, ParsedSymbol, ReferenceKind, SymbolKind, Visibility,
};
use tree_sitter::{Node, Tree};

#[derive(Debug)]
pub struct GenericAnalyzer {
    base: AnalyzerBase,
    language: Language,
}

impl GenericAnalyzer {
    #[must_use]
    pub fn new(language: Language) -> Self {
        Self {
            base: AnalyzerBase::new(),
            language,
        }
    }

    fn extract_generic_symbols(&self, node: Node, source: &str, symbols: &mut Vec<ParsedSymbol>) {
        let kind = node.kind();

        let symbol_kind = match (self.language, kind) {
            // Python
            (Language::Python, "function_definition") => Some(SymbolKind::Function),
            (Language::Python, "class_definition") => Some(SymbolKind::Class),

            // JavaScript/TypeScript
            (Language::JavaScript | Language::TypeScript, "function_declaration") => {
                Some(SymbolKind::Function)
            }
            (Language::JavaScript | Language::TypeScript, "class_declaration") => {
                Some(SymbolKind::Class)
            }
            (Language::JavaScript | Language::TypeScript, "method_definition") => {
                Some(SymbolKind::Method)
            }
            (Language::TypeScript, "interface_declaration") => Some(SymbolKind::Interface),

            // Go
            (Language::Go, "function_declaration") => Some(SymbolKind::Function),
            (Language::Go, "method_declaration") => Some(SymbolKind::Method),
            (Language::Go, "type_declaration") => Some(SymbolKind::Type),

            // Java
            (Language::Java, "method_declaration") => Some(SymbolKind::Method),
            (Language::Java, "class_declaration") => Some(SymbolKind::Class),
            (Language::Java, "interface_declaration") => Some(SymbolKind::Interface),
            (Language::Java, "enum_declaration") => Some(SymbolKind::Enum),

            // C/C++
            (Language::C | Language::Cpp, "function_definition") => Some(SymbolKind::Function),
            (Language::Cpp, "class_specifier") => Some(SymbolKind::Class),
            (Language::C | Language::Cpp, "struct_specifier") => Some(SymbolKind::Struct),
            (Language::C | Language::Cpp, "enum_specifier") => Some(SymbolKind::Enum),

            // Ruby
            (Language::Ruby, "method") => Some(SymbolKind::Method),
            (Language::Ruby, "class") => Some(SymbolKind::Class),
            (Language::Ruby, "module") => Some(SymbolKind::Module),

            // PHP
            (Language::Php, "function_definition") => Some(SymbolKind::Function),
            (Language::Php, "method_declaration") => Some(SymbolKind::Method),
            (Language::Php, "class_declaration") => Some(SymbolKind::Class),
            (Language::Php, "interface_declaration") => Some(SymbolKind::Interface),
            (Language::Php, "trait_declaration") => Some(SymbolKind::Trait),

            _ => None,
        };

        if let Some(sk) = symbol_kind
            && let Some(name_node) = self.base.find_child_by_field(node, "name") {
                let name = self.base.node_text(name_node, source);
                let doc_comment = self.base.extract_doc_comment(node, source);

                let mut children = Vec::new();
                if matches!(
                    sk,
                    SymbolKind::Class | SymbolKind::Struct | SymbolKind::Interface
                )
                    && let Some(body) = self.base.find_child_by_field(node, "body") {
                        self.extract_generic_symbols(body, source, &mut children);
                    }

                symbols.push(ParsedSymbol {
                    name,
                    kind: sk,
                    location: self.base.node_location(node),
                    signature: None,
                    doc_comment,
                    visibility: Visibility::Unknown,
                    children,
                });
                return;
            }

        for i in 0..node.child_count() {
            if let Some(child) = node.child(i) {
                self.extract_generic_symbols(child, source, symbols);
            }
        }
    }

    fn collect_generic_references(
        &self,
        node: Node,
        source: &str,
        refs: &mut Vec<ParsedReference>,
    ) {
        let kind = node.kind();

        match self.language {
            Language::Python => self.collect_python_references(node, source, refs),
            Language::JavaScript | Language::TypeScript => {
                self.collect_js_references(node, source, refs);
            }
            Language::Go => self.collect_go_references(node, source, refs),
            Language::Java => self.collect_java_references(node, source, refs),
            Language::C | Language::Cpp => self.collect_c_references(node, source, refs),
            _ => {
                if (kind == "call_expression" || kind == "call")
                    && let Some(func) = node
                        .child_by_field_name("function")
                        .or_else(|| node.child_by_field_name("name"))
                        .or_else(|| node.child(0))
                    {
                        let name = self.base.node_text(func, source);
                        if !name.is_empty() {
                            refs.push(ParsedReference {
                                name,
                                kind: ReferenceKind::Call,
                                location: self.base.node_location(func),
                                containing_symbol_index: None,
                            });
                        }
                    }
            }
        }

        for i in 0..node.child_count() {
            if let Some(child) = node.child(i) {
                self.collect_generic_references(child, source, refs);
            }
        }
    }

    // Python-specific methods
    fn collect_python_references(&self, node: Node, source: &str, refs: &mut Vec<ParsedReference>) {
        match node.kind() {
            "call" => {
                if let Some(func) = node.child_by_field_name("function") {
                    let name = self.extract_python_call_name(func, source);
                    if !name.is_empty() && !self.is_python_builtin(&name) {
                        refs.push(ParsedReference {
                            name,
                            kind: ReferenceKind::Call,
                            location: self.base.node_location(func),
                            containing_symbol_index: None,
                        });
                    }
                }
            }
            "import_statement" | "import_from_statement" => {
                self.extract_python_imports(node, source, refs);
            }
            "attribute" => {
                if let Some(attr) = node.child_by_field_name("attribute") {
                    let name = self.base.node_text(attr, source);
                    refs.push(ParsedReference {
                        name,
                        kind: ReferenceKind::FieldAccess,
                        location: self.base.node_location(attr),
                        containing_symbol_index: None,
                    });
                }
            }
            "class_definition" => {
                if let Some(args) = node.child_by_field_name("superclasses") {
                    for i in 0..args.child_count() {
                        if let Some(arg) = args.child(i)
                            && (arg.kind() == "identifier" || arg.kind() == "attribute") {
                                let name = self.base.node_text(arg, source);
                                refs.push(ParsedReference {
                                    name,
                                    kind: ReferenceKind::Inheritance,
                                    location: self.base.node_location(arg),
                                    containing_symbol_index: None,
                                });
                            }
                    }
                }
            }
            _ => {}
        }
    }

    fn extract_python_call_name(&self, node: Node, source: &str) -> String {
        match node.kind() {
            "identifier" => self.base.node_text(node, source),
            "attribute" => {
                if let Some(attr) = node.child_by_field_name("attribute") {
                    self.base.node_text(attr, source)
                } else {
                    self.base.node_text(node, source)
                }
            }
            _ => self.base.node_text(node, source),
        }
    }

    fn extract_python_imports(&self, node: Node, source: &str, refs: &mut Vec<ParsedReference>) {
        match node.kind() {
            "import_statement" => {
                for i in 0..node.child_count() {
                    if let Some(child) = node.child(i)
                        && (child.kind() == "dotted_name" || child.kind() == "aliased_import") {
                            let name = if child.kind() == "aliased_import" {
                                if let Some(name_node) = child.child_by_field_name("name") {
                                    self.base.node_text(name_node, source)
                                } else {
                                    continue;
                                }
                            } else {
                                self.base.node_text(child, source)
                            };
                            refs.push(ParsedReference {
                                name,
                                kind: ReferenceKind::Import,
                                location: self.base.node_location(child),
                                containing_symbol_index: None,
                            });
                        }
                }
            }
            "import_from_statement" => {
                let module = node
                    .child_by_field_name("module_name")
                    .map(|n| self.base.node_text(n, source))
                    .unwrap_or_default();

                for i in 0..node.child_count() {
                    if let Some(child) = node.child(i)
                        && (child.kind() == "dotted_name"
                            || child.kind() == "aliased_import"
                            || child.kind() == "identifier")
                        {
                            let import_name = if child.kind() == "aliased_import" {
                                if let Some(name_node) = child.child_by_field_name("name") {
                                    self.base.node_text(name_node, source)
                                } else {
                                    continue;
                                }
                            } else {
                                self.base.node_text(child, source)
                            };

                            let full_name = if module.is_empty() {
                                import_name
                            } else {
                                format!("{module}.{import_name}")
                            };

                            refs.push(ParsedReference {
                                name: full_name,
                                kind: ReferenceKind::Import,
                                location: self.base.node_location(child),
                                containing_symbol_index: None,
                            });
                        }
                }
            }
            _ => {}
        }
    }

    fn is_python_builtin(&self, name: &str) -> bool {
        matches!(
            name,
            "print"
                | "len"
                | "range"
                | "str"
                | "int"
                | "float"
                | "bool"
                | "list"
                | "dict"
                | "set"
                | "tuple"
                | "type"
                | "isinstance"
                | "issubclass"
                | "hasattr"
                | "getattr"
                | "setattr"
                | "delattr"
                | "id"
                | "hash"
                | "repr"
                | "abs"
                | "round"
                | "min"
                | "max"
                | "sum"
                | "sorted"
                | "reversed"
                | "enumerate"
                | "zip"
                | "map"
                | "filter"
                | "any"
                | "all"
                | "open"
                | "input"
                | "super"
                | "object"
                | "None"
                | "True"
                | "False"
        )
    }

    // JavaScript/TypeScript-specific methods
    fn collect_js_references(&self, node: Node, source: &str, refs: &mut Vec<ParsedReference>) {
        match node.kind() {
            "call_expression" => {
                if let Some(func) = node.child_by_field_name("function") {
                    let name = self.extract_js_call_name(func, source);
                    if !name.is_empty() && !self.is_js_builtin(&name) {
                        refs.push(ParsedReference {
                            name,
                            kind: ReferenceKind::Call,
                            location: self.base.node_location(func),
                            containing_symbol_index: None,
                        });
                    }
                }
            }
            "new_expression" => {
                if let Some(constructor) = node.child_by_field_name("constructor") {
                    let name = self.base.node_text(constructor, source);
                    refs.push(ParsedReference {
                        name,
                        kind: ReferenceKind::Call,
                        location: self.base.node_location(constructor),
                        containing_symbol_index: None,
                    });
                }
            }
            "import_statement" | "import_clause" => {
                self.extract_js_imports(node, source, refs);
            }
            "member_expression" => {
                if let Some(prop) = node.child_by_field_name("property") {
                    let name = self.base.node_text(prop, source);
                    refs.push(ParsedReference {
                        name,
                        kind: ReferenceKind::FieldAccess,
                        location: self.base.node_location(prop),
                        containing_symbol_index: None,
                    });
                }
            }
            "class_declaration" | "class" => {
                if let Some(heritage) = node.child_by_field_name("heritage") {
                    for i in 0..heritage.child_count() {
                        if let Some(clause) = heritage.child(i)
                            && clause.kind() == "extends_clause" {
                                for j in 0..clause.child_count() {
                                    if let Some(base) = clause.child(j)
                                        && (base.kind() == "identifier"
                                            || base.kind() == "member_expression")
                                        {
                                            let name = self.base.node_text(base, source);
                                            refs.push(ParsedReference {
                                                name,
                                                kind: ReferenceKind::Inheritance,
                                                location: self.base.node_location(base),
                                                containing_symbol_index: None,
                                            });
                                        }
                                }
                            }
                    }
                }
            }
            "type_identifier" => {
                let name = self.base.node_text(node, source);
                if !self.is_js_primitive_type(&name) {
                    refs.push(ParsedReference {
                        name,
                        kind: ReferenceKind::TypeReference,
                        location: self.base.node_location(node),
                        containing_symbol_index: None,
                    });
                }
            }
            _ => {}
        }
    }

    fn extract_js_call_name(&self, node: Node, source: &str) -> String {
        match node.kind() {
            "identifier" => self.base.node_text(node, source),
            "member_expression" => {
                if let Some(prop) = node.child_by_field_name("property") {
                    self.base.node_text(prop, source)
                } else {
                    self.base.node_text(node, source)
                }
            }
            _ => self.base.node_text(node, source),
        }
    }

    fn extract_js_imports(&self, node: Node, source: &str, refs: &mut Vec<ParsedReference>) {
        if let Some(source_node) = node.child_by_field_name("source") {
            let module_path = self
                .base
                .node_text(source_node, source)
                .trim_matches(|c| c == '"' || c == '\'')
                .to_string();

            for i in 0..node.child_count() {
                if let Some(child) = node.child(i) {
                    match child.kind() {
                        "identifier" => {
                            let name = self.base.node_text(child, source);
                            refs.push(ParsedReference {
                                name: format!("{module_path}:{name}"),
                                kind: ReferenceKind::Import,
                                location: self.base.node_location(child),
                                containing_symbol_index: None,
                            });
                        }
                        "import_specifier" => {
                            if let Some(name_node) = child.child_by_field_name("name") {
                                let name = self.base.node_text(name_node, source);
                                refs.push(ParsedReference {
                                    name: format!("{module_path}:{name}"),
                                    kind: ReferenceKind::Import,
                                    location: self.base.node_location(name_node),
                                    containing_symbol_index: None,
                                });
                            }
                        }
                        "named_imports" => {
                            for j in 0..child.child_count() {
                                if let Some(spec) = child.child(j)
                                    && spec.kind() == "import_specifier"
                                        && let Some(name_node) = spec.child_by_field_name("name") {
                                            let name = self.base.node_text(name_node, source);
                                            refs.push(ParsedReference {
                                                name: format!("{module_path}:{name}"),
                                                kind: ReferenceKind::Import,
                                                location: self.base.node_location(name_node),
                                                containing_symbol_index: None,
                                            });
                                        }
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
    }

    fn is_js_builtin(&self, name: &str) -> bool {
        matches!(
            name,
            "console"
                | "log"
                | "error"
                | "warn"
                | "info"
                | "debug"
                | "parseInt"
                | "parseFloat"
                | "isNaN"
                | "isFinite"
                | "encodeURI"
                | "decodeURI"
                | "encodeURIComponent"
                | "decodeURIComponent"
                | "eval"
                | "setTimeout"
                | "setInterval"
                | "clearTimeout"
                | "clearInterval"
                | "fetch"
                | "require"
                | "module"
                | "exports"
                | "process"
                | "JSON"
                | "Math"
                | "Date"
                | "RegExp"
                | "Error"
                | "Promise"
                | "Array"
                | "Object"
                | "String"
                | "Number"
                | "Boolean"
                | "Symbol"
                | "Map"
                | "Set"
                | "WeakMap"
                | "WeakSet"
                | "Proxy"
                | "Reflect"
        )
    }

    fn is_js_primitive_type(&self, name: &str) -> bool {
        matches!(
            name,
            "string"
                | "number"
                | "boolean"
                | "null"
                | "undefined"
                | "void"
                | "any"
                | "never"
                | "unknown"
                | "object"
                | "symbol"
                | "bigint"
        )
    }

    // Go-specific methods
    fn collect_go_references(&self, node: Node, source: &str, refs: &mut Vec<ParsedReference>) {
        match node.kind() {
            "call_expression" => {
                if let Some(func) = node.child_by_field_name("function") {
                    let name = self.extract_go_call_name(func, source);
                    if !name.is_empty() && !self.is_go_builtin(&name) {
                        refs.push(ParsedReference {
                            name,
                            kind: ReferenceKind::Call,
                            location: self.base.node_location(func),
                            containing_symbol_index: None,
                        });
                    }
                }
            }
            "import_declaration" => {
                self.extract_go_imports(node, source, refs);
            }
            "selector_expression" => {
                if let Some(field) = node.child_by_field_name("field") {
                    let name = self.base.node_text(field, source);
                    refs.push(ParsedReference {
                        name,
                        kind: ReferenceKind::FieldAccess,
                        location: self.base.node_location(field),
                        containing_symbol_index: None,
                    });
                }
            }
            "type_identifier" => {
                let name = self.base.node_text(node, source);
                if !self.is_go_primitive_type(&name) {
                    refs.push(ParsedReference {
                        name,
                        kind: ReferenceKind::TypeReference,
                        location: self.base.node_location(node),
                        containing_symbol_index: None,
                    });
                }
            }
            _ => {}
        }
    }

    fn extract_go_call_name(&self, node: Node, source: &str) -> String {
        match node.kind() {
            "identifier" => self.base.node_text(node, source),
            "selector_expression" => {
                if let Some(field) = node.child_by_field_name("field") {
                    self.base.node_text(field, source)
                } else {
                    self.base.node_text(node, source)
                }
            }
            _ => self.base.node_text(node, source),
        }
    }

    fn extract_go_imports(&self, node: Node, source: &str, refs: &mut Vec<ParsedReference>) {
        for i in 0..node.child_count() {
            if let Some(child) = node.child(i)
                && (child.kind() == "import_spec" || child.kind() == "import_spec_list") {
                    self.extract_go_import_spec(child, source, refs);
                }
        }
    }

    fn extract_go_import_spec(&self, node: Node, source: &str, refs: &mut Vec<ParsedReference>) {
        match node.kind() {
            "import_spec" => {
                if let Some(path) = node.child_by_field_name("path") {
                    let import_path = self
                        .base
                        .node_text(path, source)
                        .trim_matches('"')
                        .to_string();
                    refs.push(ParsedReference {
                        name: import_path,
                        kind: ReferenceKind::Import,
                        location: self.base.node_location(path),
                        containing_symbol_index: None,
                    });
                }
            }
            "import_spec_list" => {
                for i in 0..node.child_count() {
                    if let Some(child) = node.child(i) {
                        self.extract_go_import_spec(child, source, refs);
                    }
                }
            }
            _ => {}
        }
    }

    fn is_go_builtin(&self, name: &str) -> bool {
        matches!(
            name,
            "append"
                | "cap"
                | "close"
                | "complex"
                | "copy"
                | "delete"
                | "imag"
                | "len"
                | "make"
                | "new"
                | "panic"
                | "print"
                | "println"
                | "real"
                | "recover"
        )
    }

    fn is_go_primitive_type(&self, name: &str) -> bool {
        matches!(
            name,
            "bool"
                | "string"
                | "int"
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
                | "byte"
                | "rune"
                | "float32"
                | "float64"
                | "complex64"
                | "complex128"
                | "error"
                | "any"
        )
    }

    // Java-specific methods
    fn collect_java_references(&self, node: Node, source: &str, refs: &mut Vec<ParsedReference>) {
        match node.kind() {
            "method_invocation" => {
                if let Some(name_node) = node.child_by_field_name("name") {
                    let name = self.base.node_text(name_node, source);
                    refs.push(ParsedReference {
                        name,
                        kind: ReferenceKind::Call,
                        location: self.base.node_location(name_node),
                        containing_symbol_index: None,
                    });
                }
            }
            "object_creation_expression" => {
                if let Some(type_node) = node.child_by_field_name("type") {
                    let name = self.base.node_text(type_node, source);
                    refs.push(ParsedReference {
                        name,
                        kind: ReferenceKind::Call,
                        location: self.base.node_location(type_node),
                        containing_symbol_index: None,
                    });
                }
            }
            "import_declaration" => {
                for i in 0..node.child_count() {
                    if let Some(child) = node.child(i)
                        && child.kind() == "scoped_identifier" {
                            let name = self.base.node_text(child, source);
                            refs.push(ParsedReference {
                                name,
                                kind: ReferenceKind::Import,
                                location: self.base.node_location(child),
                                containing_symbol_index: None,
                            });
                        }
                }
            }
            "field_access" => {
                if let Some(field) = node.child_by_field_name("field") {
                    let name = self.base.node_text(field, source);
                    refs.push(ParsedReference {
                        name,
                        kind: ReferenceKind::FieldAccess,
                        location: self.base.node_location(field),
                        containing_symbol_index: None,
                    });
                }
            }
            "class_declaration" => {
                if let Some(superclass) = node.child_by_field_name("superclass") {
                    let name = self.base.node_text(superclass, source);
                    refs.push(ParsedReference {
                        name,
                        kind: ReferenceKind::Inheritance,
                        location: self.base.node_location(superclass),
                        containing_symbol_index: None,
                    });
                }
                if let Some(interfaces) = node.child_by_field_name("interfaces") {
                    for i in 0..interfaces.child_count() {
                        if let Some(iface) = interfaces.child(i)
                            && iface.kind() == "type_identifier" {
                                let name = self.base.node_text(iface, source);
                                refs.push(ParsedReference {
                                    name,
                                    kind: ReferenceKind::Inheritance,
                                    location: self.base.node_location(iface),
                                    containing_symbol_index: None,
                                });
                            }
                    }
                }
            }
            "type_identifier" => {
                let name = self.base.node_text(node, source);
                if !self.is_java_primitive_type(&name) {
                    refs.push(ParsedReference {
                        name,
                        kind: ReferenceKind::TypeReference,
                        location: self.base.node_location(node),
                        containing_symbol_index: None,
                    });
                }
            }
            _ => {}
        }
    }

    fn is_java_primitive_type(&self, name: &str) -> bool {
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

    // C/C++-specific methods
    fn collect_c_references(&self, node: Node, source: &str, refs: &mut Vec<ParsedReference>) {
        match node.kind() {
            "call_expression" => {
                if let Some(func) = node.child_by_field_name("function") {
                    let name = self.base.node_text(func, source);
                    if !name.is_empty() && !self.is_c_builtin(&name) {
                        refs.push(ParsedReference {
                            name,
                            kind: ReferenceKind::Call,
                            location: self.base.node_location(func),
                            containing_symbol_index: None,
                        });
                    }
                }
            }
            "preproc_include" => {
                if let Some(path) = node.child_by_field_name("path") {
                    let name = self
                        .base
                        .node_text(path, source)
                        .trim_matches(|c| c == '"' || c == '<' || c == '>')
                        .to_string();
                    refs.push(ParsedReference {
                        name,
                        kind: ReferenceKind::Import,
                        location: self.base.node_location(path),
                        containing_symbol_index: None,
                    });
                }
            }
            "field_expression" => {
                if let Some(field) = node.child_by_field_name("field") {
                    let name = self.base.node_text(field, source);
                    refs.push(ParsedReference {
                        name,
                        kind: ReferenceKind::FieldAccess,
                        location: self.base.node_location(field),
                        containing_symbol_index: None,
                    });
                }
            }
            "type_identifier" => {
                let name = self.base.node_text(node, source);
                if !self.is_c_primitive_type(&name) {
                    refs.push(ParsedReference {
                        name,
                        kind: ReferenceKind::TypeReference,
                        location: self.base.node_location(node),
                        containing_symbol_index: None,
                    });
                }
            }
            "base_class_clause" => {
                for i in 0..node.child_count() {
                    if let Some(child) = node.child(i)
                        && (child.kind() == "type_identifier"
                            || child.kind() == "qualified_identifier")
                        {
                            let name = self.base.node_text(child, source);
                            refs.push(ParsedReference {
                                name,
                                kind: ReferenceKind::Inheritance,
                                location: self.base.node_location(child),
                                containing_symbol_index: None,
                            });
                        }
                }
            }
            _ => {}
        }
    }

    fn is_c_builtin(&self, name: &str) -> bool {
        matches!(
            name,
            "printf"
                | "scanf"
                | "malloc"
                | "free"
                | "realloc"
                | "calloc"
                | "memcpy"
                | "memset"
                | "memmove"
                | "memcmp"
                | "strlen"
                | "strcpy"
                | "strcat"
                | "strcmp"
                | "strncpy"
                | "strncmp"
                | "fopen"
                | "fclose"
                | "fread"
                | "fwrite"
                | "fprintf"
                | "fscanf"
                | "exit"
                | "abort"
                | "assert"
                | "sizeof"
                | "alignof"
        )
    }

    fn is_c_primitive_type(&self, name: &str) -> bool {
        matches!(
            name,
            "int"
                | "long"
                | "short"
                | "char"
                | "float"
                | "double"
                | "void"
                | "unsigned"
                | "signed"
                | "size_t"
                | "ptrdiff_t"
                | "intptr_t"
                | "uintptr_t"
                | "int8_t"
                | "int16_t"
                | "int32_t"
                | "int64_t"
                | "uint8_t"
                | "uint16_t"
                | "uint32_t"
                | "uint64_t"
                | "bool"
                | "_Bool"
                | "auto"
        )
    }
}

impl LanguageAnalyzer for GenericAnalyzer {
    fn extract_symbols(&self, source: &str, tree: &Tree) -> Vec<ParsedSymbol> {
        let mut symbols = Vec::new();
        self.extract_generic_symbols(tree.root_node(), source, &mut symbols);
        symbols
    }

    fn extract_references(&self, source: &str, tree: &Tree) -> Vec<ParsedReference> {
        let mut references = Vec::new();
        self.collect_generic_references(tree.root_node(), source, &mut references);
        references
    }
}
