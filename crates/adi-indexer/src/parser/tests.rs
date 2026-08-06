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
    use crate::parser::treesitter::TreeSitterParser;
    use crate::parser::Parser;
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
            let parsed = parse("struct P;\nimpl P { fn origin() -> Self { P } }", Language::Rust);

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
        fn a_module_is_a_symbol_but_its_body_is_not_descended_into() {
            // Upstream behaviour, carried over as-is: `mod_item` yields the module and stops,
            // so items declared inside an inline module are not indexed. Files are what this
            // indexer walks; `mod foo;` bodies live in their own file and are reached that way.
            let parsed = parse("mod geometry { fn origin() {} }", Language::Rust);

            assert_eq!(
                find(&parsed.symbols, "geometry").map(|s| s.kind),
                Some(SymbolKind::Module)
            );
            assert!(find(&parsed.symbols, "origin").is_none());
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
        fn visibility_is_not_read_off_rust_declarations() {
            // The Rust analyzer stamps every symbol `Unknown` — it never looks at `pub`. That
            // is how it arrived, and it is why the dead-code analysis's public-symbol filter
            // finds nothing to keep in a Rust tree. Recorded here so a fix is a test change,
            // not a surprise.
            let parsed = parse("pub fn open() {}\nfn shut() {}", Language::Rust);

            use crate::types::Visibility;
            assert_eq!(find(&parsed.symbols, "open").unwrap().visibility, Visibility::Unknown);
            assert_eq!(find(&parsed.symbols, "shut").unwrap().visibility, Visibility::Unknown);
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
            assert_eq!(find(&parsed.symbols, "third_line").unwrap().location.start_line, 2);
        }

        #[test]
        fn empty_and_comment_only_sources_are_not_errors() {
            assert!(parse("", Language::Rust).symbols.is_empty());
            assert!(parse("// nothing here\n", Language::Rust).symbols.is_empty());
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
            let parsed = parse(
                "public class App { public void run() {} }",
                Language::Java,
            );
            assert_eq!(
                find(&parsed.symbols, "App").map(|s| s.kind),
                Some(SymbolKind::Class)
            );
            assert!(find(&parsed.symbols, "run").is_some());
        }
    }
}
