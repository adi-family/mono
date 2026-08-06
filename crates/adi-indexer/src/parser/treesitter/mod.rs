// Copyright (c) 2024-2025 Ihor
// SPDX-License-Identifier: BUSL-1.1
// See LICENSE file for details

pub mod analyzers;

use tree_sitter::Parser as TsParser;

use crate::error::{Error, Result};
use crate::lang;
use crate::parser::Parser;
use crate::structure;
use crate::types::{Language, ParsedFile, ParsedSymbol};
use analyzers::{generic::GenericAnalyzer, LanguageAnalyzer};

/// Tree-sitter parser over the grammars this build links in (see [`crate::lang`]).
///
/// A file is parsed once, and the resulting tree is handed to the language's own analyzer —
/// or to [`GenericAnalyzer`] when the language has a grammar but no dedicated walker.
#[derive(Debug)]
pub struct TreeSitterParser;

impl TreeSitterParser {
    #[must_use]
    pub fn new() -> Self {
        Self
    }

    /// The analyzer for a language: its own if this build has one, the generic walker otherwise.
    fn analyzer_for(language: Language) -> Box<dyn LanguageAnalyzer> {
        lang::analyzer(language).unwrap_or_else(|| Box::new(GenericAnalyzer::new(language)))
    }
}

impl Default for TreeSitterParser {
    fn default() -> Self {
        Self::new()
    }
}

impl Parser for TreeSitterParser {
    fn parse(&self, source: &str, language: Language) -> Result<ParsedFile> {
        let ts_lang = lang::grammar(language).ok_or_else(|| {
            Error::UnsupportedLanguage(format!(
                "{}: this build has no grammar for it (enable the lang-{} feature)",
                language.as_str(),
                language.as_str()
            ))
        })?;

        let mut parser = TsParser::new();
        parser
            .set_language(&ts_lang)
            .map_err(|e| Error::Parser(format!("Failed to set language: {e}")))?;

        let tree = parser
            .parse(source, None)
            .ok_or_else(|| Error::Parser("Failed to parse source".to_string()))?;

        let analyzer = Self::analyzer_for(language);
        let mut symbols = analyzer.extract_symbols(source, &tree);
        let references = analyzer.extract_references(source, &tree);

        // The analyzer says which spans are symbols; the fingerprint says what shape each one
        // has. Kept apart so every language gets structural search from the one walk here,
        // rather than thirteen analyzers each having to remember to do it.
        attach_structure(&mut symbols, tree.root_node());

        Ok(ParsedFile {
            language,
            symbols,
            references,
        })
    }

    fn supports(&self, language: Language) -> bool {
        lang::grammar(language).is_some()
    }
}

/// Fingerprint every symbol in the tree the analyzer just produced, children included.
fn attach_structure(symbols: &mut [ParsedSymbol], root: tree_sitter::Node) {
    for symbol in symbols {
        symbol.structure = structure::node_for_range(
            root,
            symbol.location.start_byte,
            symbol.location.end_byte,
        )
        .map(structure::fingerprint);

        attach_structure(&mut symbol.children, root);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_language_without_a_grammar_is_unsupported() {
        let parser = TreeSitterParser::new();
        assert!(!parser.supports(Language::Unknown));
        assert!(parser.parse("anything", Language::Unknown).is_err());
    }

    #[cfg(feature = "lang-rust")]
    #[test]
    fn rust_parses_to_its_symbols() {
        let parser = TreeSitterParser::new();
        assert!(parser.supports(Language::Rust));

        let parsed = parser
            .parse("pub fn main() { helper(); }\nfn helper() {}", Language::Rust)
            .expect("rust source parses");

        let names: Vec<_> = parsed.symbols.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"main"), "got {names:?}");
        assert!(names.contains(&"helper"), "got {names:?}");
        assert!(parsed.references.iter().any(|r| r.name == "helper"));
    }
}
