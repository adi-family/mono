//! The languages this build can parse.
//!
//! Upstream these were `adi.lang.*` cdylib plugins, discovered under the plugins directory at
//! run time and dlopen'd for their grammar. Here each is a module compiled in behind its
//! `lang-*` feature, and this file is the whole registry: [`grammar`] hands the parser a
//! tree-sitter language, [`analyzer`] hands it the walker that turns that tree into symbols.
//!
//! A language with a grammar but no analyzer still indexes — the parser falls back to
//! [`GenericAnalyzer`](crate::parser::treesitter::analyzers::generic::GenericAnalyzer), which
//! reads node kinds it recognizes across languages.

mod common;

#[cfg(feature = "lang-cpp")]
pub mod cpp;
#[cfg(feature = "lang-csharp")]
pub mod csharp;
#[cfg(feature = "lang-go")]
pub mod go;
#[cfg(feature = "lang-java")]
pub mod java;
#[cfg(feature = "lang-lua")]
pub mod lua;
#[cfg(feature = "lang-php")]
pub mod php;
#[cfg(feature = "lang-python")]
pub mod python;
#[cfg(feature = "lang-ruby")]
pub mod ruby;
#[cfg(feature = "lang-rust")]
pub mod rust;
#[cfg(feature = "lang-swift")]
pub mod swift;
#[cfg(feature = "lang-typescript")]
pub mod typescript;

use crate::parser::treesitter::analyzers::LanguageAnalyzer;
use crate::types::Language;

/// The tree-sitter grammar for a language, if this build carries one.
#[allow(unused_variables, clippy::match_single_binding)]
#[must_use]
pub fn grammar(language: Language) -> Option<tree_sitter::Language> {
    match language {
        #[cfg(feature = "lang-rust")]
        Language::Rust => Some(rust::language()),
        #[cfg(feature = "lang-typescript")]
        Language::TypeScript => Some(typescript::language()),
        // The TypeScript analyzer reads plain JavaScript trees just as well — the node kinds it
        // matches on (`function_declaration`, `class_declaration`, `call_expression`, …) are
        // shared — so JS gets the JS grammar and the same walker rather than nothing.
        #[cfg(feature = "lang-typescript")]
        Language::JavaScript => Some(tree_sitter_javascript::LANGUAGE.into()),
        #[cfg(feature = "lang-python")]
        Language::Python => Some(python::language()),
        #[cfg(feature = "lang-go")]
        Language::Go => Some(go::language()),
        #[cfg(feature = "lang-java")]
        Language::Java => Some(java::language()),
        #[cfg(feature = "lang-cpp")]
        Language::Cpp => Some(cpp::cpp_language()),
        #[cfg(feature = "lang-cpp")]
        Language::C => Some(cpp::c_language()),
        #[cfg(feature = "lang-csharp")]
        Language::CSharp => Some(csharp::language()),
        #[cfg(feature = "lang-php")]
        Language::Php => Some(php::language()),
        #[cfg(feature = "lang-ruby")]
        Language::Ruby => Some(ruby::language()),
        #[cfg(feature = "lang-lua")]
        Language::Lua => Some(lua::language()),
        #[cfg(feature = "lang-swift")]
        Language::Swift => Some(swift::language()),
        _ => None,
    }
}

/// The dedicated analyzer for a language, if this build carries one. `None` means the caller
/// should fall back to the generic analyzer.
#[allow(unused_variables, clippy::match_single_binding)]
#[must_use]
pub fn analyzer(language: Language) -> Option<Box<dyn LanguageAnalyzer>> {
    match language {
        #[cfg(feature = "lang-rust")]
        Language::Rust => Some(Box::new(rust::RustAnalyzer)),
        #[cfg(feature = "lang-typescript")]
        Language::TypeScript | Language::JavaScript => {
            Some(Box::new(typescript::TypeScriptAnalyzer))
        }
        #[cfg(feature = "lang-python")]
        Language::Python => Some(Box::new(python::PythonAnalyzer)),
        #[cfg(feature = "lang-go")]
        Language::Go => Some(Box::new(go::GoAnalyzer)),
        #[cfg(feature = "lang-java")]
        Language::Java => Some(Box::new(java::JavaAnalyzer)),
        #[cfg(feature = "lang-cpp")]
        Language::Cpp | Language::C => Some(Box::new(cpp::CppAnalyzer)),
        #[cfg(feature = "lang-csharp")]
        Language::CSharp => Some(Box::new(csharp::CSharpAnalyzer)),
        #[cfg(feature = "lang-php")]
        Language::Php => Some(Box::new(php::PhpAnalyzer)),
        #[cfg(feature = "lang-ruby")]
        Language::Ruby => Some(Box::new(ruby::RubyAnalyzer)),
        #[cfg(feature = "lang-lua")]
        Language::Lua => Some(Box::new(lua::LuaAnalyzer)),
        #[cfg(feature = "lang-swift")]
        Language::Swift => Some(Box::new(swift::SwiftAnalyzer)),
        _ => None,
    }
}

/// Every language this build can parse, in a stable order — what `indexer languages` prints.
#[must_use]
pub fn supported() -> Vec<Language> {
    [
        Language::Rust,
        Language::TypeScript,
        Language::JavaScript,
        Language::Python,
        Language::Go,
        Language::Java,
        Language::Cpp,
        Language::C,
        Language::CSharp,
        Language::Php,
        Language::Ruby,
        Language::Lua,
        Language::Swift,
    ]
    .into_iter()
    .filter(|l| grammar(*l).is_some())
    .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_supported_language_has_a_grammar_and_an_analyzer() {
        for language in supported() {
            assert!(grammar(language).is_some(), "{language:?} lost its grammar");
            assert!(
                analyzer(language).is_some(),
                "{language:?} lost its analyzer"
            );
        }
    }

    #[cfg(feature = "all-languages")]
    #[test]
    fn the_default_build_carries_every_language() {
        assert_eq!(supported().len(), 13);
    }

    #[test]
    fn a_language_with_no_grammar_is_not_claimed() {
        assert!(grammar(Language::Unknown).is_none());
        assert!(grammar(Language::Zig).is_none());
    }
}
