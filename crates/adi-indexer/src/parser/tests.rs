// Copyright (c) 2024-2025 Ihor
// SPDX-License-Identifier: BUSL-1.1
// See LICENSE file for details

//! Parser tests.
//!
//! Upstream every one of these was `#[ignore]`d and `unimplemented!()` — parsing needed an
//! `adi-lang-*` dylib installed on the machine, which a test run had no way to guarantee.
//! The grammars are linked into this crate now, so each test gated on its language feature
//! runs for real.

#[cfg(test)]
#[allow(clippy::module_inception)]
mod tests {
    use crate::parser::Parser;
    use crate::parser::treesitter::TreeSitterParser;
    use crate::types::{Language, ParsedFile, ParsedSymbol, SymbolKind};

    fn parse(source: &str, language: Language) -> ParsedFile {
        TreeSitterParser::new()
            .parse(source, language)
            .expect("source parses")
    }

    /// Symbols nest (methods inside a class), so a name search has to walk the tree.
    fn find<'a>(symbols: &'a [ParsedSymbol], name: &str) -> Option<&'a ParsedSymbol> {
        for symbol in symbols {
            if symbol.name == name {
                return Some(symbol);
            }
            if let Some(found) = find(&symbol.children, name) {
                return Some(found);
            }
        }
        None
    }

    fn references_to(parsed: &ParsedFile, name: &str) -> usize {
        parsed.references.iter().filter(|r| r.name == name).count()
    }

    #[test]
    fn a_language_with_no_grammar_is_unsupported() {
        let parser = TreeSitterParser::new();
        assert!(!parser.supports(Language::Unknown));
        assert!(parser.parse("fn main() {}", Language::Unknown).is_err());
    }

    // --- Rust ---

    #[cfg(feature = "lang-rust")]
    mod rust {
        use super::*;

        #[test]
        fn function() {
            let parsed = parse("fn add(a: i32, b: i32) -> i32 { a + b }", Language::Rust);
            let f = find(&parsed.symbols, "add").expect("add");
            assert_eq!(f.kind, SymbolKind::Function);
            assert!(f.signature.as_deref().unwrap_or("").contains("i32"));
        }

        #[test]
        fn struct_and_enum_and_trait() {
            let parsed = parse(
                "pub struct Point { x: f64 }\nenum Shape { Dot }\ntrait Draw { fn draw(&self); }",
                Language::Rust,
            );
            assert_eq!(
                find(&parsed.symbols, "Point").map(|s| s.kind),
                Some(SymbolKind::Struct)
            );
            assert_eq!(
                find(&parsed.symbols, "Shape").map(|s| s.kind),
                Some(SymbolKind::Enum)
            );
            assert_eq!(
                find(&parsed.symbols, "Draw").map(|s| s.kind),
                Some(SymbolKind::Trait)
            );
        }

        #[test]
        fn impl_methods_are_qualified_by_their_type() {
            let parsed = parse(
                "struct P;\nimpl P { fn origin() -> Self { P } }",
                Language::Rust,
            );

            let method = find(&parsed.symbols, "P::origin").expect("P::origin");
            assert_eq!(method.kind, SymbolKind::Method);
        }

        #[test]
        fn a_trait_impl_names_both_sides() {
            let parsed = parse(
                "struct P;\ntrait Draw { fn draw(&self); }\nimpl Draw for P { fn draw(&self) {} }",
                Language::Rust,
            );

            assert!(find(&parsed.symbols, "Draw for P::draw").is_some());
        }

        #[test]
        fn inline_modules_preserve_nested_items_and_external_modules_stay_empty() {
            let parsed = parse(
                "mod geometry { fn origin() {} mod nested { struct Point; impl Point { fn new() {} } } } mod external;",
                Language::Rust,
            );

            assert_eq!(parsed.symbols.len(), 2);
            let geometry = find(&parsed.symbols, "geometry").unwrap();
            assert_eq!(geometry.kind, SymbolKind::Module);
            assert_eq!(geometry.children.len(), 2);
            assert!(find(&geometry.children, "origin").is_some());
            let nested = find(&geometry.children, "nested").unwrap();
            assert_eq!(nested.children.len(), 2);
            assert!(find(&nested.children, "Point::new").is_some());
            assert!(
                find(&parsed.symbols, "external")
                    .unwrap()
                    .children
                    .is_empty()
            );
        }

        #[test]
        fn constant() {
            let parsed = parse("const MAX: usize = 10;", Language::Rust);
            assert_eq!(
                find(&parsed.symbols, "MAX").map(|s| s.kind),
                Some(SymbolKind::Constant)
            );
        }

        #[test]
        fn declared_visibility_distinguishes_public_and_restricted_access() {
            use crate::types::Visibility;

            let parsed = parse(
                "pub fn open() {} fn shut() {} pub(crate) fn crate_only() {} pub(super) fn parent_only() {} pub(self) fn local_only() {} pub(in crate::outer) fn scoped() {} pub(in crate) fn in_crate() {} pub(in super) fn in_parent() {} pub struct Point { pub x: u8, y: u8 } impl Point { pub fn new() {} fn hidden() {} }",
                Language::Rust,
            );
            for (name, visibility) in [
                ("open", Visibility::Public),
                ("shut", Visibility::Private),
                ("crate_only", Visibility::PublicCrate),
                ("parent_only", Visibility::PublicSuper),
                ("local_only", Visibility::Private),
                ("scoped", Visibility::Internal),
                ("in_crate", Visibility::PublicCrate),
                ("in_parent", Visibility::PublicSuper),
                ("Point", Visibility::Public),
                ("x", Visibility::Public),
                ("y", Visibility::Private),
                ("Point::new", Visibility::Public),
                ("Point::hidden", Visibility::Private),
            ] {
                assert_eq!(
                    find(&parsed.symbols, name).unwrap().visibility,
                    visibility,
                    "{name}"
                );
            }
        }

        #[test]
        fn trait_and_impl_associated_items_are_indexed() {
            use crate::types::Visibility;

            let parsed = parse(
                "pub trait Size { type Output; const LEN: usize; fn size(&self); fn default_size(&self) {} } struct P; impl Size for P { type Output = u8; const LEN: usize = 4; fn size(&self) {} } impl P { pub const CAP: usize = 8; }",
                Language::Rust,
            );
            let trait_symbol = find(&parsed.symbols, "Size").unwrap();
            assert_eq!(trait_symbol.children.len(), 4);
            for (name, kind) in [
                ("Output", SymbolKind::Type),
                ("LEN", SymbolKind::Constant),
                ("size", SymbolKind::Method),
                ("default_size", SymbolKind::Method),
            ] {
                let symbol = find(&trait_symbol.children, name).unwrap();
                assert_eq!(symbol.kind, kind);
                assert_eq!(symbol.visibility, Visibility::Public);
            }
            for (name, kind) in [
                ("Size for P::Output", SymbolKind::Type),
                ("Size for P::LEN", SymbolKind::Constant),
                ("Size for P::size", SymbolKind::Method),
                ("P::CAP", SymbolKind::Constant),
            ] {
                let symbol = find(&parsed.symbols, name).unwrap();
                assert_eq!(symbol.kind, kind);
                assert_eq!(symbol.visibility, Visibility::Public);
            }
        }

        #[test]
        fn signatures_keep_array_lengths_and_const_generic_expressions() {
            let parsed = parse(
                "trait Read { fn read(\n &self, bytes: [u8; 4]\n) -> [u8; 4]; } fn sized() -> Buffer<{ 1 + 2 }> { loop {} }",
                Language::Rust,
            );
            assert_eq!(
                find(&parsed.symbols, "read").unwrap().signature.as_deref(),
                Some("fn read(\n &self, bytes: [u8; 4]\n) -> [u8; 4]")
            );
            assert_eq!(
                find(&parsed.symbols, "sized").unwrap().signature.as_deref(),
                Some("fn sized() -> Buffer<{ 1 + 2 }>")
            );
        }

        #[test]
        fn generic_calls_reference_the_callee_without_type_arguments() {
            use crate::types::ReferenceKind;

            let parsed = parse(
                "fn main() { helper::<u8>(); obj.render::<u8>(); Thing::build::<u8>(); }",
                Language::Rust,
            );
            let calls: Vec<_> = parsed
                .references
                .iter()
                .filter(|r| r.kind == ReferenceKind::Call)
                .map(|r| r.name.as_str())
                .collect();
            assert_eq!(calls, ["helper", "obj.render", "Thing::build"]);
        }

        #[test]
        fn method_calls_keep_receivers_separate_from_free_functions() {
            use crate::types::ReferenceKind;

            let parsed = parse(
                "struct Thing; impl Thing { fn run(&self) { object.helper(); self.helper(); helper(); self.generic::<u8>(); } }",
                Language::Rust,
            );
            let calls: Vec<_> = parsed
                .references
                .iter()
                .filter(|reference| reference.kind == ReferenceKind::Call)
                .map(|reference| reference.name.as_str())
                .collect();
            assert_eq!(
                calls,
                ["object.helper", "self.helper", "helper", "self.generic"]
            );
        }

        #[test]
        fn doc_comment_rides_along() {
            let parsed = parse("/// Adds two numbers.\nfn add() {}", Language::Rust);
            let doc = find(&parsed.symbols, "add")
                .and_then(|s| s.doc_comment.clone())
                .unwrap_or_default();
            assert!(doc.contains("Adds two numbers"), "got {doc:?}");
        }

        #[test]
        fn a_call_is_a_reference_but_a_definition_is_not() {
            let parsed = parse("fn helper() {}\nfn main() { helper(); }", Language::Rust);
            assert_eq!(references_to(&parsed, "helper"), 1);
            assert_eq!(references_to(&parsed, "main"), 0);
        }

        #[test]
        fn location_points_at_the_declaration() {
            let parsed = parse("\n\nfn third_line() {}", Language::Rust);
            assert_eq!(
                find(&parsed.symbols, "third_line")
                    .unwrap()
                    .location
                    .start_line,
                2
            );
        }

        #[test]
        fn empty_and_comment_only_sources_are_not_errors() {
            assert!(parse("", Language::Rust).symbols.is_empty());
            assert!(
                parse("// nothing here\n", Language::Rust)
                    .symbols
                    .is_empty()
            );
        }
    }

    // --- Python ---

    #[cfg(feature = "lang-python")]
    mod python {
        use super::*;

        #[test]
        fn function_and_class() {
            let parsed = parse(
                "def greet(name):\n    return name\n\nclass Greeter:\n    def hello(self):\n        greet('x')\n",
                Language::Python,
            );
            assert_eq!(
                find(&parsed.symbols, "greet").map(|s| s.kind),
                Some(SymbolKind::Function)
            );
            assert_eq!(
                find(&parsed.symbols, "Greeter").map(|s| s.kind),
                Some(SymbolKind::Class)
            );
            assert!(find(&parsed.symbols, "hello").is_some());
            assert_eq!(references_to(&parsed, "greet"), 1);
        }
    }

    // --- TypeScript / JavaScript ---

    #[cfg(feature = "lang-typescript")]
    mod typescript {
        use super::*;

        #[test]
        fn interface_and_class() {
            let parsed = parse(
                "interface Shape { area(): number }\nclass Circle implements Shape { area() { return 1 } }",
                Language::TypeScript,
            );
            assert_eq!(
                find(&parsed.symbols, "Shape").map(|s| s.kind),
                Some(SymbolKind::Interface)
            );
            assert_eq!(
                find(&parsed.symbols, "Circle").map(|s| s.kind),
                Some(SymbolKind::Class)
            );
        }

        #[test]
        fn javascript_uses_its_own_grammar_with_the_same_walker() {
            let parser = TreeSitterParser::new();
            assert!(parser.supports(Language::JavaScript));

            let parsed = parse(
                "function helper() {}\nclass Thing {}\nhelper();",
                Language::JavaScript,
            );
            assert!(find(&parsed.symbols, "helper").is_some());
            assert!(find(&parsed.symbols, "Thing").is_some());
            assert_eq!(references_to(&parsed, "helper"), 1);
        }
    }

    // --- Go ---

    #[cfg(feature = "lang-go")]
    mod go {
        use super::*;

        #[test]
        fn function() {
            let parsed = parse(
                "package main\n\nfunc Helper() {}\n\nfunc main() { Helper() }\n",
                Language::Go,
            );
            assert_eq!(
                find(&parsed.symbols, "Helper").map(|s| s.kind),
                Some(SymbolKind::Function)
            );
            assert_eq!(references_to(&parsed, "Helper"), 1);
        }
    }

    // --- Java ---

    #[cfg(feature = "lang-java")]
    mod java {
        use super::*;

        #[test]
        fn class_and_method() {
            let parsed = parse("public class App { public void run() {} }", Language::Java);
            assert_eq!(
                find(&parsed.symbols, "App").map(|s| s.kind),
                Some(SymbolKind::Class)
            );
            assert!(find(&parsed.symbols, "run").is_some());
        }
    }
}
