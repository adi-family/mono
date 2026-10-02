use crate::parser::{Parser, TreeSitterParser};
use crate::types::{Language, ReferenceKind, SymbolKind, Visibility};

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

#[test]
fn explicit_exports_mark_declarations_public_without_exporting_nested_helpers() {
    let parsed = TreeSitterParser::new().parse(
        "export function api() { function helper() {} }\nexport const arrow = () => { function inside() {} };\nexport class Api {}\nexport interface Contract {}\nexport type Value = string;\nexport enum Choice { One }\nfunction local() {}",
        Language::TypeScript,
    ).unwrap();
    for name in ["api", "arrow", "Api", "Contract", "Value", "Choice"] {
        let symbol = parsed
            .symbols
            .iter()
            .find(|symbol| symbol.name == name)
            .unwrap();
        assert_eq!(symbol.visibility, Visibility::Public, "{name}");
        assert!(
            symbol
                .children
                .iter()
                .all(|child| child.visibility != Visibility::Public)
        );
    }
    assert_eq!(
        parsed
            .symbols
            .iter()
            .find(|symbol| symbol.name == "local")
            .unwrap()
            .visibility,
        Visibility::Unknown
    );
}

#[test]
fn local_export_aliases_and_default_identifiers_mark_their_local_declaration() {
    for language in [Language::TypeScript, Language::JavaScript] {
        let parsed = TreeSitterParser::new().parse(
            "export { later as published }; function later() {} function chosen() {} export default chosen; function remote() {} export { remote } from './other';",
            language,
        ).unwrap();
        for name in ["later", "chosen"] {
            assert_eq!(
                parsed
                    .symbols
                    .iter()
                    .find(|symbol| symbol.name == name)
                    .unwrap()
                    .visibility,
                Visibility::Public
            );
        }
        assert_ne!(
            parsed
                .symbols
                .iter()
                .find(|symbol| symbol.name == "remote")
                .unwrap()
                .visibility,
            Visibility::Public
        );
        assert!(
            !parsed
                .symbols
                .iter()
                .any(|symbol| matches!(symbol.name.as_str(), "default" | "published"))
        );
    }
}

#[test]
fn type_wrapped_callables_keep_the_binding_name_and_body_children() {
    let parsed = TreeSitterParser::new().parse(
        "const asserted = (() => { function inner() {} return 1; }) as () => number;\nconst checked = ((function implementation() { return 2; }) satisfies () => number);\nconst chained = ((() => 3) as (() => number)) satisfies (() => number);\nconst nonNull = (() => 4)!;\nconst angle = <(() => number)>(() => 5);\nconst ordinary = 42 as number;",
        Language::TypeScript,
    ).unwrap();
    for name in ["asserted", "checked", "chained", "nonNull", "angle"] {
        let symbol = parsed
            .symbols
            .iter()
            .find(|symbol| symbol.name == name)
            .expect(name);
        assert_eq!(symbol.kind, SymbolKind::Function);
        assert_eq!(
            symbol.signature.as_deref(),
            Some(format!("{name}()").as_str())
        );
    }
    assert_eq!(parsed.symbols[0].children[0].name, "inner");
    assert!(
        !parsed
            .symbols
            .iter()
            .any(|symbol| symbol.name == "implementation"
                || (symbol.name == "ordinary" && symbol.kind == SymbolKind::Function))
    );
}

#[test]
fn class_callable_fields_have_member_visibility_and_nested_symbols() {
    let parsed = TreeSitterParser::new().parse(
        "export class Api { public run = () => { function helper() {} }; private hidden = function () {}; protected guarded = (() => 1) satisfies () => number; #secret = () => 2; normal() { class Local {} } private ordinary() {} }",
        Language::TypeScript,
    ).unwrap();
    let class = &parsed.symbols[0];
    for (name, visibility) in [
        ("run", Visibility::Public),
        ("hidden", Visibility::Private),
        ("guarded", Visibility::Protected),
        ("#secret", Visibility::Private),
        ("normal", Visibility::Public),
        ("ordinary", Visibility::Private),
    ] {
        let member = class
            .children
            .iter()
            .find(|member| member.name == name)
            .expect(name);
        assert_eq!(member.kind, SymbolKind::Method);
        assert_eq!(member.visibility, visibility);
        assert!(member.structure.is_some());
    }
    assert_eq!(
        class
            .children
            .iter()
            .find(|member| member.name == "run")
            .unwrap()
            .children[0]
            .name,
        "helper"
    );
    assert_eq!(
        class
            .children
            .iter()
            .find(|member| member.name == "normal")
            .unwrap()
            .children[0]
            .name,
        "Local"
    );

    let parsed = TreeSitterParser::new()
        .parse(
            "class Api { run = () => 1; #hidden = function () {}; }",
            Language::JavaScript,
        )
        .unwrap();
    assert_eq!(parsed.symbols[0].children[0].name, "run");
    assert_eq!(parsed.symbols[0].children[0].visibility, Visibility::Public);
    assert_eq!(parsed.symbols[0].children[1].name, "#hidden");
    assert_eq!(
        parsed.symbols[0].children[1].visibility,
        Visibility::Private
    );
}

#[test]
fn default_exports_keep_named_declarations_and_name_anonymous_values() {
    for language in [Language::TypeScript, Language::JavaScript] {
        for (source, name, kind) in [
            (
                "export default function named() { function inner() {} }",
                "named",
                SymbolKind::Function,
            ),
            (
                "export default function () { function inner() {} }",
                "default",
                SymbolKind::Function,
            ),
            (
                "export default () => { function inner() {} };",
                "default",
                SymbolKind::Function,
            ),
            (
                "export default class Named { run() {} }",
                "Named",
                SymbolKind::Class,
            ),
            (
                "export default class { run = () => 1; }",
                "default",
                SymbolKind::Class,
            ),
            ("export default 42;", "default", SymbolKind::Constant),
        ] {
            let parsed = TreeSitterParser::new().parse(source, language).unwrap();
            assert_eq!(parsed.symbols.len(), 1, "{language:?}: {source}");
            let symbol = &parsed.symbols[0];
            assert_eq!(symbol.name, name);
            assert_eq!(symbol.kind, kind);
            assert_eq!(symbol.visibility, Visibility::Public);
            if kind == SymbolKind::Function {
                assert_eq!(symbol.children[0].name, "inner");
            }
            if kind == SymbolKind::Class {
                assert_eq!(symbol.children[0].name, "run");
            }
        }
    }
}

#[test]
fn function_declarations_preserve_nested_declarations_at_every_level() {
    for language in [Language::TypeScript, Language::JavaScript] {
        let parsed = TreeSitterParser::new().parse(
            "function outer() { function middle() { const inner = () => { class Local {} }; } }",
            language,
        ).unwrap();
        let outer = &parsed.symbols[0];
        assert_eq!(outer.children[0].name, "middle");
        assert_eq!(outer.children[0].children[0].name, "inner");
        assert_eq!(outer.children[0].children[0].children[0].name, "Local");
    }
}

#[test]
fn exported_value_and_class_bindings_keep_their_declared_names() {
    for language in [Language::TypeScript, Language::JavaScript] {
        let parsed = TreeSitterParser::new().parse(
            "export const answer = 42; export let mutable = 1; const selected = 2; export default selected; export const Factory = class Implementation { run = () => 1; }; const ordinary = 7;",
            language,
        ).unwrap();
        for (name, kind) in [
            ("answer", SymbolKind::Constant),
            ("mutable", SymbolKind::Variable),
            ("selected", SymbolKind::Constant),
            ("Factory", SymbolKind::Class),
        ] {
            let symbol = parsed
                .symbols
                .iter()
                .find(|symbol| symbol.name == name)
                .expect(name);
            assert_eq!(symbol.kind, kind);
            assert_eq!(symbol.visibility, Visibility::Public);
        }
        let class = parsed
            .symbols
            .iter()
            .find(|symbol| symbol.name == "Factory")
            .unwrap();
        assert_eq!(class.children[0].name, "run");
        assert!(
            !parsed
                .symbols
                .iter()
                .any(|symbol| symbol.name == "Implementation")
        );
        assert_eq!(
            parsed
                .symbols
                .iter()
                .find(|symbol| symbol.name == "ordinary")
                .unwrap()
                .visibility,
            Visibility::Unknown
        );
    }
}
