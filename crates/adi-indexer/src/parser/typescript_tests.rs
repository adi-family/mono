use crate::parser::{Parser, TreeSitterParser};
use crate::types::{Language, ReferenceKind, SymbolKind};

#[test]
fn function_bindings_are_indexed_by_their_callable_name() {
    for language in [Language::TypeScript, Language::JavaScript] {
        let source = r"
export const increment = x => x + 1;
const double = function (x) { return x * 2; };
const triple = function implementation(x) { return x * 3; };
const sequence = function* () { yield 1; };
const wrapped = (() => 1);
";
        let parsed = TreeSitterParser::new().parse(source, language).unwrap();
        for name in ["increment", "double", "triple", "sequence", "wrapped"] {
            let symbol = parsed.symbols.iter().find(|s| s.name == name);
            assert!(
                symbol.is_some(),
                "missing {name} in {language:?}: {:?}",
                parsed.symbols
            );
            let symbol = symbol.unwrap();
            assert_eq!(symbol.kind, SymbolKind::Function);
            assert!(
                symbol
                    .signature
                    .as_deref()
                    .is_some_and(|s| s.starts_with(name))
            );
            assert!(symbol.structure.is_some());
        }
        assert_eq!(
            parsed.symbols.len(),
            5,
            "a named function expression must not emit a second top-level symbol"
        );
    }
}

#[test]
fn bound_callable_bodies_keep_nested_declarations_as_children() {
    for language in [Language::TypeScript, Language::JavaScript] {
        for callable in [
            "() =>",
            "function ()",
            "function implementation()",
            "function* ()",
        ] {
            let source = format!(
                "const outer = {callable} {{ function inner() {{}} class Inner {{ method() {{}} }} const local = () => 1; return inner(); }};"
            );
            let parsed = TreeSitterParser::new().parse(&source, language).unwrap();
            assert_eq!(parsed.symbols.len(), 1, "{language:?}: {source}");
            let outer = &parsed.symbols[0];
            assert_eq!(outer.name, "outer");
            let children: Vec<_> = outer
                .children
                .iter()
                .map(|child| child.name.as_str())
                .collect();
            assert_eq!(
                children,
                ["inner", "Inner", "local"],
                "{language:?}: {source}"
            );
            assert!(outer.children.iter().all(|child| child.structure.is_some()));
            assert_eq!(outer.children[1].children[0].name, "method");
            assert!(parsed.references.iter().any(
                |reference| reference.name == "inner" && reference.kind == ReferenceKind::Call
            ));
        }
    }
}

#[test]
fn generic_functions_keep_valid_return_type_signatures() {
    let parsed = TreeSitterParser::new()
        .parse(
            "function identity<T>(value: T): T { return value; }",
            Language::TypeScript,
        )
        .unwrap();
    assert_eq!(
        parsed.symbols[0].signature.as_deref(),
        Some("identity<T>(value: T): T")
    );
}

#[test]
fn typed_arrow_bindings_keep_generic_parameters_and_return_types() {
    let parsed = TreeSitterParser::new()
        .parse(
            "export const identity = <T>(value: T): T => value;",
            Language::TypeScript,
        )
        .unwrap();
    assert_eq!(
        parsed.symbols[0].signature.as_deref(),
        Some("identity<T>(value: T): T")
    );
}

#[test]
fn destructured_and_non_callable_bindings_do_not_become_functions() {
    let parsed = TreeSitterParser::new()
        .parse(
            "const [first, second] = [() => 1, () => 2]; const answer = 42; const object = { run: () => 1 };",
            Language::JavaScript,
        )
        .unwrap();
    assert!(!parsed.symbols.iter().any(|symbol| {
        symbol.kind == SymbolKind::Function
            && ["first", "second", "answer", "object"].contains(&symbol.name.as_str())
    }));
}

#[test]
fn generator_declarations_are_callable_symbols() {
    for language in [Language::TypeScript, Language::JavaScript] {
        let parsed = TreeSitterParser::new()
            .parse("export function* items() { yield 1; }", language)
            .unwrap();
        assert!(
            parsed
                .symbols
                .iter()
                .any(|s| s.name == "items" && s.kind == SymbolKind::Function)
        );
    }
}

#[test]
fn reexports_have_module_dependency_references() {
    let parsed = TreeSitterParser::new()
        .parse(
            "export { helper } from './helpers';\nexport * from './other';\nexport const local = 1;",
            Language::TypeScript,
        )
        .unwrap();
    let imports: Vec<_> = parsed
        .references
        .iter()
        .filter(|reference| reference.kind == ReferenceKind::Import)
        .map(|reference| reference.name.as_str())
        .collect();
    assert_eq!(imports, ["./helpers", "./other"]);
}
