//! Name resolution from the information the index actually retains.
//!
//! Prefer the nearest lexical scope, then a unique project-level declaration. Qualified names
//! must match their qualification; dropping a receiver or module prefix fabricates edges.
//! This is deliberately not a compiler: imports/aliases, receiver types, overload selection,
//! conditional compilation, and package manifests are not present in this symbol table. When
//! more than one declaration fits, leave the reference unresolved instead of choosing a row or
//! connecting it to every candidate.

use std::collections::{HashMap, HashSet};

use crate::storage::PendingRef;
use crate::types::{FileId, Language, Reference, ReferenceKind, Symbol, SymbolId, SymbolKind};

pub(super) fn resolve_references(references: &[PendingRef], symbols: &[Symbol]) -> Vec<Reference> {
    let resolver = Resolver::new(symbols);
    references
        .iter()
        .filter_map(|pending| {
            let source = *resolver.by_id.get(&pending.from_symbol_id)?;
            let target = resolver.resolve(source, &pending.target_name, pending.kind)?;
            Some(Reference {
                from_symbol_id: pending.from_symbol_id,
                to_symbol_id: symbols[target].id,
                kind: pending.kind,
                location: pending.location.clone(),
            })
        })
        .collect()
}

type ScopeKey = (FileId, Option<SymbolId>, String);

enum Lookup {
    Absent,
    Unresolved,
    Resolved(usize),
}

impl Lookup {
    fn target(self) -> Option<usize> {
        match self {
            Self::Resolved(target) => Some(target),
            Self::Absent | Self::Unresolved => None,
        }
    }
}

struct Resolver<'a> {
    symbols: &'a [Symbol],
    by_id: HashMap<SymbolId, usize>,
    by_file: HashMap<FileId, usize>,
    scopes: HashMap<ScopeKey, Vec<usize>>,
    global: HashMap<String, Vec<usize>>,
    qualified: HashMap<Vec<String>, Vec<usize>>,
    declared: HashMap<Vec<String>, Vec<usize>>,
    paths: Vec<Vec<String>>,
}

impl<'a> Resolver<'a> {
    fn new(symbols: &'a [Symbol]) -> Self {
        let mut resolver = Self {
            symbols,
            by_id: symbols.iter().enumerate().map(|(i, s)| (s.id, i)).collect(),
            by_file: symbols
                .iter()
                .enumerate()
                .map(|(i, s)| (s.file_id, i))
                .collect(),
            scopes: HashMap::new(),
            global: HashMap::new(),
            qualified: HashMap::new(),
            declared: HashMap::new(),
            paths: Vec::with_capacity(symbols.len()),
        };
        for (index, symbol) in symbols.iter().enumerate() {
            resolver
                .scopes
                .entry((symbol.file_id, symbol.parent_id, symbol.name.clone()))
                .or_default()
                .push(index);
            if symbol.parent_id.is_none() {
                resolver
                    .global
                    .entry(symbol.name.clone())
                    .or_default()
                    .push(index);
            }
            let mut declared = Vec::new();
            for ancestor in resolver.ancestors(index).into_iter().rev() {
                declared.extend(name_parts(&symbols[ancestor].name));
            }
            let mut qualified = file_module(symbol);
            qualified.extend(declared.iter().cloned());
            resolver
                .qualified
                .entry(qualified.clone())
                .or_default()
                .push(index);
            // Rust impl methods carry `Type::method` directly in their declaration name.
            // Keep that existing identity, but do not remove a containing file module from
            // arbitrary nested declarations (a.rs's `mod b` is `a::b`, not root `b`).
            if symbol.parent_id.is_none() && name_parts(&symbol.name).len() > 1 {
                resolver.declared.entry(declared).or_default().push(index);
            }
            resolver.paths.push(qualified);
        }
        resolver
    }

    /// Innermost symbol first. A damaged parent chain must not hang a search.
    fn ancestors(&self, source: usize) -> Vec<usize> {
        let mut ancestors = Vec::new();
        let mut seen = HashSet::new();
        let mut next = Some(source);
        while let Some(index) = next {
            let symbol = &self.symbols[index];
            if symbol.file_id != self.symbols[source].file_id || !seen.insert(symbol.id) {
                break;
            }
            ancestors.push(index);
            next = symbol
                .parent_id
                .and_then(|parent| self.by_id.get(&parent).copied());
        }
        ancestors
    }

    fn resolve(&self, source: usize, name: &str, kind: ReferenceKind) -> Option<usize> {
        let symbol = &self.symbols[source];
        let parts = name_parts(name);
        if parts.is_empty() {
            return None;
        }
        let ancestors = self.ancestors(source);
        let source_language = language(symbol);
        if parts.len() == 1 && !name.starts_with("::") {
            // Most analyzers retain only the property token for a field access. A property
            // called `run` does not establish that its receiver is the surrounding class.
            if kind == ReferenceKind::FieldAccess {
                return None;
            }
            for scope in ancestors
                .iter()
                .filter(|&&index| {
                    let scope = self.symbols[index].kind;
                    // These languages require this/self for class members. Python class
                    // bodies themselves have a local namespace, but method bodies do not.
                    !((matches!(source_language, Language::JavaScript | Language::TypeScript)
                        && scope == SymbolKind::Class)
                        || (source_language == Language::Python
                            && scope == SymbolKind::Class
                            && index != source)
                        || (source_language == Language::Rust && scope == SymbolKind::Trait))
                })
                .map(|&i| Some(self.symbols[i].id))
                .chain([None])
            {
                if let Some(candidates) =
                    self.scopes.get(&(symbol.file_id, scope, name.to_string()))
                {
                    return self.unique(candidates, symbol.file_id, kind, None);
                }
            }
            // Preserve a unique project-level match when no lexical declaration is known.
            // Nested symbols in unrelated modules/classes are never candidates here.
            return self
                .global
                .get(name)
                .and_then(|candidates| self.unique(candidates, symbol.file_id, kind, None));
        }

        let mut absolute = name.starts_with("::");
        let mut anchored = None;
        let mut explicit_parts = parts.len();
        let rust_path =
            source_language == Language::Rust && name.contains("::") && !name.contains('.');
        if rust_path && parts[0] == "crate" {
            absolute = true;
            anchored = Some(parts[1..].to_vec());
            explicit_parts -= 1;
        } else if (rust_path && parts[0] == "Self")
            || (parts[0] == "this"
                && name.contains('.')
                && matches!(
                    source_language,
                    Language::JavaScript | Language::TypeScript | Language::Java | Language::CSharp
                ))
            || (parts[0] == "self"
                && name.contains('.')
                && matches!(
                    source_language,
                    Language::Rust | Language::Python | Language::Ruby | Language::Swift
                ))
        {
            let owner = ancestors.iter().find_map(|&index| {
                let candidate = &self.symbols[index];
                if matches!(
                    candidate.kind,
                    SymbolKind::Class | SymbolKind::Struct | SymbolKind::Trait | SymbolKind::Enum
                ) {
                    Some(self.paths[index].clone())
                } else if candidate.kind == SymbolKind::Method
                    && name_parts(&candidate.name).len() > 1
                {
                    let mut owner = self.paths[index].clone();
                    owner.pop();
                    Some(owner)
                } else {
                    None
                }
            })?;
            anchored = Some(
                owner
                    .into_iter()
                    .chain(parts[1..].iter().cloned())
                    .collect(),
            );
            explicit_parts -= 1;
        } else if rust_path && (parts[0] == "self" || parts[0] == "super") {
            let mut module = ancestors
                .iter()
                .find(|&&i| {
                    matches!(
                        self.symbols[i].kind,
                        SymbolKind::Module | SymbolKind::Namespace
                    )
                })
                .map_or_else(|| file_module(symbol), |&i| self.paths[i].clone());
            let mut offset = usize::from(parts[0] == "self");
            while parts.get(offset).is_some_and(|part| part == "super") {
                module.pop()?;
                offset += 1;
            }
            module.extend(parts[offset..].iter().cloned());
            anchored = Some(module);
            explicit_parts -= offset;
        }
        if let Some(path) = anchored {
            return self
                .lookup(&self.qualified, &path, symbol.file_id, kind, explicit_parts)
                .target();
        }

        // Rust dots denote value receivers, not module/type qualification. Their spelling
        // alone cannot establish a type, even if an unrelated file has the same stem.
        if language(symbol) == Language::Rust && name.contains('.') {
            return None;
        }

        if !absolute {
            for prefix in ancestors
                .iter()
                .map(|&i| self.paths[i].clone())
                .chain([file_module(symbol)])
            {
                let path: Vec<_> = prefix.into_iter().chain(parts.iter().cloned()).collect();
                match self.lookup(&self.qualified, &path, symbol.file_id, kind, explicit_parts) {
                    Lookup::Absent => {}
                    found => return found.target(),
                }
            }
        }
        match self.lookup(
            &self.qualified,
            &parts,
            symbol.file_id,
            kind,
            explicit_parts,
        ) {
            Lookup::Absent => {}
            found => return found.target(),
        }
        if absolute {
            return None;
        }
        // An explicit declaration such as `Widget::run` can be indexed in another Rust file.
        // Keep the full declared qualification here; never fall back to just `run`.
        self.lookup(&self.declared, &parts, symbol.file_id, kind, explicit_parts)
            .target()
    }

    /// An absent name permits another scope lookup; an unresolved match stops the search.
    fn lookup(
        &self,
        names: &HashMap<Vec<String>, Vec<usize>>,
        path: &[String],
        file: FileId,
        kind: ReferenceKind,
        explicit_parts: usize,
    ) -> Lookup {
        let Some(candidates) = names.get(path) else {
            return Lookup::Absent;
        };
        match self.unique(candidates, file, kind, Some(explicit_parts)) {
            Some(target) => Lookup::Resolved(target),
            None => Lookup::Unresolved,
        }
    }

    fn unique(
        &self,
        candidates: &[usize],
        file: FileId,
        kind: ReferenceKind,
        explicit_parts: Option<usize>,
    ) -> Option<usize> {
        let local = candidates.iter().any(|&i| self.symbols[i].file_id == file);
        let source_language = language(&self.symbols[self.by_file[&file]]);
        let mut eligible = candidates.iter().copied().filter(|&i| {
            (!local || self.symbols[i].file_id == file)
                && same_language_family(source_language, language(&self.symbols[i]))
                && compatible(kind, self.symbols[i].kind)
                && explicit_parts.is_none_or(|parts| {
                    let explicit_start = self.paths[i].len().saturating_sub(parts);
                    // A function contains lexical declarations, not namespace members. A
                    // scope implicitly prepended above may contain a function, but a written
                    // `outer::inner` / `outer.inner` cannot qualify through that function.
                    self.ancestors(i).into_iter().skip(1).all(|ancestor| {
                        !matches!(
                            self.symbols[ancestor].kind,
                            SymbolKind::Function
                                | SymbolKind::Method
                                | SymbolKind::Constructor
                                | SymbolKind::Destructor
                                | SymbolKind::Operator
                        ) || self.paths[ancestor].len() <= explicit_start
                    })
                })
        });
        let first = eligible.next()?;
        eligible.next().is_none().then_some(first)
    }
}

fn name_parts(name: &str) -> Vec<String> {
    name.split("::")
        .flat_map(|part| part.split('.'))
        .filter(|part| !part.is_empty())
        .map(str::to_string)
        .collect()
}

fn language(symbol: &Symbol) -> Language {
    symbol
        .file_path
        .extension()
        .and_then(|ext| ext.to_str())
        .map_or(Language::Unknown, Language::from_extension)
}

fn same_language_family(a: Language, b: Language) -> bool {
    a == b
        || matches!(
            (a, b),
            (Language::TypeScript, Language::JavaScript)
                | (Language::JavaScript, Language::TypeScript)
                | (Language::C, Language::Cpp)
                | (Language::Cpp, Language::C)
        )
}

/// Conventional Rust module paths are recoverable without parsing a manifest. Other languages'
/// file-to-module mappings depend on imports/package configuration, which the index does not retain.
fn file_module(symbol: &Symbol) -> Vec<String> {
    if !symbol
        .file_path
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("rs"))
    {
        return Vec::new();
    }
    let mut parts: Vec<_> = symbol
        .file_path
        .parent()
        .into_iter()
        .flat_map(|path| path.iter())
        .filter_map(|part| part.to_str())
        .map(str::to_string)
        .collect();
    if let Some(src) = parts.iter().rposition(|part| part == "src") {
        parts.drain(..=src);
    }
    if let Some(stem) = symbol.file_path.file_stem().and_then(|stem| stem.to_str())
        && !matches!(stem, "lib" | "main" | "mod")
    {
        parts.push(stem.to_string());
    }
    parts
}

fn compatible(reference: ReferenceKind, symbol: SymbolKind) -> bool {
    use ReferenceKind as R;
    use SymbolKind as S;
    match reference {
        R::Call => matches!(
            symbol,
            S::Function
                | S::Method
                | S::Constructor
                | S::Destructor
                | S::Operator
                | S::Class
                | S::Struct
                | S::Enum
                | S::Type
        ),
        R::TypeReference | R::Inheritance => matches!(
            symbol,
            S::Class | S::Struct | S::Enum | S::Interface | S::Trait | S::Type
        ),
        R::FieldAccess => matches!(symbol, S::Field | S::Property | S::Method | S::Function),
        R::MacroInvocation => symbol == S::Macro,
        R::VariableReference => {
            matches!(symbol, S::Variable | S::Constant | S::Field | S::Property)
        }
        R::Import => symbol != S::Import,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{Location, Visibility};

    fn symbol(
        id: i64,
        file: i64,
        path: &str,
        parent: Option<i64>,
        name: &str,
        kind: SymbolKind,
    ) -> Symbol {
        Symbol {
            id: SymbolId(id),
            file_id: FileId(file),
            file_path: path.into(),
            parent_id: parent.map(SymbolId),
            name: name.into(),
            kind,
            location: Location::new(1, 0, 2, 0, id as u32 * 10, id as u32 * 10 + 9),
            signature: None,
            description: None,
            doc_comment: None,
            visibility: Visibility::Unknown,
            is_entry_point: false,
            structure: None,
        }
    }

    fn local(id: i64, parent: Option<i64>, name: &str, kind: SymbolKind) -> Symbol {
        symbol(id, 1, "src/lib.rs", parent, name, kind)
    }

    fn reference(from: i64, name: &str) -> PendingRef {
        PendingRef {
            from_symbol_id: SymbolId(from),
            target_name: name.into(),
            kind: ReferenceKind::Call,
            location: Location::new(10, 3, 10, 9, 100, 106),
        }
    }

    fn targets(symbols: &[Symbol], from: i64, name: &str) -> Vec<i64> {
        resolve_references(&[reference(from, name)], symbols)
            .iter()
            .map(|edge| edge.to_symbol_id.0)
            .collect()
    }

    #[test]
    fn nearest_lexical_scope_wins_over_sibling_modules_and_files() {
        let symbols = vec![
            local(1, None, "left", SymbolKind::Module),
            local(2, None, "right", SymbolKind::Module),
            local(3, Some(1), "caller", SymbolKind::Function),
            local(4, Some(1), "helper", SymbolKind::Function),
            local(5, Some(2), "helper", SymbolKind::Function),
            local(6, None, "helper", SymbolKind::Function),
            symbol(7, 2, "src/remote.rs", None, "helper", SymbolKind::Function),
        ];
        assert_eq!(targets(&symbols, 3, "helper"), vec![4]);
        assert_eq!(targets(&symbols, 3, "right::helper"), vec![5]);
    }

    #[test]
    fn nested_declarations_shadow_outer_scope_and_recursive_calls_survive() {
        let symbols = vec![
            local(1, None, "outer", SymbolKind::Function),
            local(2, Some(1), "helper", SymbolKind::Function),
            local(3, None, "helper", SymbolKind::Function),
        ];
        assert_eq!(targets(&symbols, 1, "helper"), vec![2]);
        assert_eq!(targets(&symbols, 2, "helper"), vec![2]);
        assert_eq!(targets(&symbols, 3, "helper"), vec![3]);
        let edges = resolve_references(&[reference(2, "helper")], &symbols);
        assert_eq!(edges[0].location.start_byte, 100);
    }

    #[test]
    fn same_file_declaration_wins_and_sibling_scopes_do_not_leak() {
        let symbols = vec![
            local(1, None, "caller", SymbolKind::Function),
            local(2, None, "helper", SymbolKind::Function),
            symbol(3, 2, "src/remote.rs", None, "helper", SymbolKind::Function),
            local(4, None, "other", SymbolKind::Module),
            local(5, Some(4), "hidden", SymbolKind::Function),
        ];
        assert_eq!(targets(&symbols, 1, "helper"), vec![2]);
        assert!(targets(&symbols, 1, "hidden").is_empty());
    }

    #[test]
    fn duplicate_declarations_and_cross_file_ambiguity_do_not_fan_out() {
        let mut symbols = vec![
            local(1, None, "caller", SymbolKind::Function),
            local(2, None, "overloaded", SymbolKind::Function),
            local(3, None, "overloaded", SymbolKind::Function),
            symbol(4, 2, "src/left.rs", None, "remote", SymbolKind::Function),
            symbol(5, 3, "src/right.rs", None, "remote", SymbolKind::Function),
        ];
        for _ in 0..2 {
            assert!(targets(&symbols, 1, "overloaded").is_empty());
            assert!(targets(&symbols, 1, "remote").is_empty());
            symbols.reverse();
        }
    }

    #[test]
    fn a_unique_project_level_function_remains_resolvable() {
        let symbols = vec![
            local(1, None, "caller", SymbolKind::Function),
            symbol(2, 2, "src/remote.rs", None, "helper", SymbolKind::Function),
        ];
        assert_eq!(targets(&symbols, 1, "helper"), vec![2]);
    }

    #[test]
    fn qualified_names_match_rust_file_modules_without_discarding_the_prefix() {
        let symbols = vec![
            local(1, None, "caller", SymbolKind::Function),
            symbol(2, 2, "src/left.rs", None, "helper", SymbolKind::Function),
            symbol(
                3,
                3,
                "src/right/mod.rs",
                None,
                "helper",
                SymbolKind::Function,
            ),
        ];
        assert_eq!(targets(&symbols, 1, "left::helper"), vec![2]);
        assert_eq!(targets(&symbols, 1, "crate::right::helper"), vec![3]);
        assert!(targets(&symbols, 1, "missing::helper").is_empty());
        assert!(targets(&symbols, 1, "object.helper").is_empty());
    }

    #[test]
    fn qualified_names_do_not_drop_the_containing_file_module() {
        let symbols = vec![
            local(1, None, "caller", SymbolKind::Function),
            symbol(2, 2, "src/outer.rs", None, "inner", SymbolKind::Module),
            symbol(
                3,
                2,
                "src/outer.rs",
                Some(2),
                "helper",
                SymbolKind::Function,
            ),
        ];
        assert!(targets(&symbols, 1, "inner::helper").is_empty());
        assert_eq!(targets(&symbols, 1, "outer::inner::helper"), vec![3]);
    }

    #[test]
    fn explicit_impl_names_can_match_across_files_only_when_unique() {
        let mut symbols = vec![
            local(1, None, "caller", SymbolKind::Function),
            symbol(
                2,
                2,
                "src/worker.rs",
                None,
                "Worker::run",
                SymbolKind::Method,
            ),
        ];
        assert_eq!(targets(&symbols, 1, "Worker::run"), vec![2]);
        symbols.push(symbol(
            3,
            3,
            "src/other.rs",
            None,
            "Worker::run",
            SymbolKind::Method,
        ));
        assert!(targets(&symbols, 1, "Worker::run").is_empty());
    }

    #[test]
    fn rust_self_super_and_crate_paths_are_anchored_to_the_module() {
        let symbols = vec![
            local(1, None, "outer", SymbolKind::Module),
            local(2, Some(1), "inner", SymbolKind::Module),
            local(3, None, "helper", SymbolKind::Function),
            local(4, Some(1), "helper", SymbolKind::Function),
            local(5, Some(2), "helper", SymbolKind::Function),
            local(6, Some(2), "caller", SymbolKind::Function),
        ];
        assert_eq!(targets(&symbols, 6, "self::helper"), vec![5]);
        assert_eq!(targets(&symbols, 6, "super::helper"), vec![4]);
        assert_eq!(targets(&symbols, 6, "super::super::helper"), vec![3]);
        assert_eq!(targets(&symbols, 6, "crate::helper"), vec![3]);
        assert!(targets(&symbols, 6, "super::super::super::helper").is_empty());
    }

    #[test]
    fn type_qualified_and_self_calls_preserve_the_owner() {
        let symbols = vec![
            local(1, None, "Widget::run", SymbolKind::Method),
            local(2, None, "Widget::helper", SymbolKind::Method),
            local(3, None, "Other::helper", SymbolKind::Method),
            local(4, None, "Class", SymbolKind::Class),
            local(5, Some(4), "helper", SymbolKind::Method),
        ];
        assert_eq!(targets(&symbols, 1, "Self::helper"), vec![2]);
        assert_eq!(targets(&symbols, 1, "Other::helper"), vec![3]);
        assert_eq!(targets(&symbols, 1, "Class::helper"), vec![5]);
        assert_eq!(targets(&symbols, 5, "self.helper"), vec![5]);
        assert!(targets(&symbols, 1, "unknown.helper").is_empty());
    }

    #[test]
    fn source_ids_must_exist_and_reference_kinds_separate_namespaces() {
        let symbols = vec![
            local(1, None, "caller", SymbolKind::Function),
            local(2, None, "Shared", SymbolKind::Function),
            local(3, None, "Shared", SymbolKind::Type),
        ];
        assert!(targets(&symbols, 99, "Shared").is_empty());
        let mut pending = reference(1, "Shared");
        pending.kind = ReferenceKind::TypeReference;
        assert_eq!(
            resolve_references(&[pending], &symbols)[0].to_symbol_id,
            SymbolId(3)
        );
    }

    #[test]
    fn unrelated_languages_do_not_create_or_confuse_edges() {
        let mut symbols = vec![
            local(1, None, "caller", SymbolKind::Function),
            symbol(2, 2, "helpers.py", None, "helper", SymbolKind::Function),
        ];
        assert!(targets(&symbols, 1, "helper").is_empty());
        symbols.push(symbol(
            3,
            3,
            "src/helper.rs",
            None,
            "helper",
            SymbolKind::Function,
        ));
        assert_eq!(targets(&symbols, 1, "helper"), vec![3]);
        assert!(
            targets(&symbols, 1, "helper.helper").is_empty(),
            "a Rust value receiver is not a file module"
        );
    }

    #[test]
    fn javascript_class_qualification_keeps_the_class_identity() {
        let symbols = vec![
            symbol(1, 1, "code.ts", None, "caller", SymbolKind::Function),
            symbol(2, 1, "code.ts", None, "Worker", SymbolKind::Class),
            symbol(3, 1, "code.ts", Some(2), "run", SymbolKind::Method),
            symbol(4, 2, "other.js", None, "other", SymbolKind::Function),
        ];
        assert_eq!(targets(&symbols, 1, "Worker.run"), vec![3]);
        assert_eq!(targets(&symbols, 3, "this.run"), vec![3]);
        assert_eq!(targets(&symbols, 1, "other"), vec![4]);
    }

    #[test]
    fn bare_field_tokens_cannot_establish_the_receiver_type() {
        let symbols = vec![
            local(1, None, "Worker", SymbolKind::Class),
            local(2, Some(1), "caller", SymbolKind::Method),
            local(3, Some(1), "helper", SymbolKind::Method),
        ];
        let mut pending = reference(2, "helper");
        pending.kind = ReferenceKind::FieldAccess;
        assert!(resolve_references(&[pending], &symbols).is_empty());
    }

    #[test]
    fn class_members_require_explicit_receivers_in_javascript_and_python() {
        for path in ["code.js", "code.ts", "code.py"] {
            let mut symbols = vec![
                symbol(1, 1, path, None, "Worker", SymbolKind::Class),
                symbol(2, 1, path, Some(1), "caller", SymbolKind::Method),
                symbol(3, 1, path, Some(1), "helper", SymbolKind::Method),
            ];
            assert!(targets(&symbols, 2, "helper").is_empty(), "{path}");
            symbols.push(symbol(4, 1, path, None, "helper", SymbolKind::Function));
            assert_eq!(targets(&symbols, 2, "helper"), vec![4], "{path}");
            let receiver = if path.ends_with("py") {
                "self.helper"
            } else {
                "this.helper"
            };
            assert_eq!(targets(&symbols, 2, receiver), vec![3], "{path}");
        }
    }

    #[test]
    fn rust_trait_associated_functions_require_self_qualification() {
        let symbols = vec![
            local(1, None, "Worker", SymbolKind::Trait),
            local(2, Some(1), "caller", SymbolKind::Method),
            local(3, Some(1), "helper", SymbolKind::Method),
        ];
        assert!(targets(&symbols, 2, "helper").is_empty());
        assert_eq!(targets(&symbols, 2, "Self::helper"), vec![3]);
    }

    #[test]
    fn nested_functions_are_lexical_bindings_not_namespace_members() {
        for path in ["src/lib.rs", "code.ts", "code.py"] {
            let symbols = vec![
                symbol(1, 1, path, None, "caller", SymbolKind::Function),
                symbol(2, 1, path, None, "outer", SymbolKind::Function),
                symbol(3, 1, path, Some(2), "inner", SymbolKind::Function),
            ];
            assert!(targets(&symbols, 1, "outer::inner").is_empty(), "{path}");
            assert!(targets(&symbols, 1, "outer.inner").is_empty(), "{path}");
            assert_eq!(targets(&symbols, 2, "inner"), vec![3], "{path}");
        }
    }

    #[test]
    fn rust_module_keywords_are_not_special_receivers_in_other_languages() {
        for path in ["code.ts", "code.js", "code.py"] {
            let symbols = vec![
                symbol(1, 1, path, None, "caller", SymbolKind::Function),
                symbol(2, 1, path, None, "helper", SymbolKind::Function),
            ];
            for name in [
                "crate.helper",
                "super.helper",
                "crate::helper",
                "self::helper",
            ] {
                assert!(targets(&symbols, 1, name).is_empty(), "{path}: {name}");
            }
        }
    }

    #[cfg(feature = "lang-rust")]
    #[test]
    fn parsed_rust_method_calls_do_not_link_untyped_receivers_to_free_functions() {
        use crate::parser::{Parser, TreeSitterParser};
        use crate::types::ParsedSymbol;

        fn insert(parsed: &[ParsedSymbol], parent: Option<SymbolId>, symbols: &mut Vec<Symbol>) {
            for parsed in parsed {
                let id = SymbolId(symbols.len() as i64 + 1);
                let mut stored = local(id.0, parent.map(|id| id.0), &parsed.name, parsed.kind);
                stored.location = parsed.location.clone();
                symbols.push(stored);
                insert(&parsed.children, Some(id), symbols);
            }
        }

        let parsed = TreeSitterParser::new().parse(
            "fn helper() {} struct Worker; impl Worker { fn helper(&self) {} fn run(&self) { self.helper(); other.helper(); helper(); Self::helper(self); } }",
            Language::Rust,
        ).unwrap();
        let mut symbols = Vec::new();
        insert(&parsed.symbols, None, &mut symbols);
        let pending: Vec<_> = parsed
            .references
            .iter()
            .filter_map(|reference| {
                let source = symbols
                    .iter()
                    .filter(|symbol| {
                        reference.location.start_byte >= symbol.location.start_byte
                            && reference.location.start_byte < symbol.location.end_byte
                    })
                    .min_by_key(|symbol| symbol.location.end_byte - symbol.location.start_byte)?;
                Some(PendingRef {
                    from_symbol_id: source.id,
                    target_name: reference.name.clone(),
                    kind: reference.kind,
                    location: reference.location.clone(),
                })
            })
            .collect();
        let edges = resolve_references(&pending, &symbols);
        let caller = symbols
            .iter()
            .find(|symbol| symbol.name == "Worker::run")
            .unwrap();
        let called: Vec<_> = edges
            .iter()
            .filter(|edge| edge.from_symbol_id == caller.id)
            .map(|edge| {
                symbols
                    .iter()
                    .find(|symbol| symbol.id == edge.to_symbol_id)
                    .unwrap()
                    .name
                    .as_str()
            })
            .collect();
        assert_eq!(called, ["Worker::helper", "helper", "Worker::helper"]);
    }
}
