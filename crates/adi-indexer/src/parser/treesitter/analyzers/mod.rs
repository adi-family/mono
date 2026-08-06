// Copyright (c) 2024-2025 Ihor
// SPDX-License-Identifier: BUSL-1.1
// See LICENSE file for details

//! Language analyzers for extracting symbols and references from parse trees.
//!
//! Language-specific analyzers are now provided by plugins (adi-lang-*).
//! Only `GenericAnalyzer` remains as a fallback.

pub mod base;
pub mod generic;

use crate::types::{ParsedReference, ParsedSymbol};
use tree_sitter::Tree;

/// Trait for language-specific code analysis
pub trait LanguageAnalyzer: Send + Sync {
    /// Extract symbols (functions, classes, etc.) from the parse tree
    fn extract_symbols(&self, source: &str, tree: &Tree) -> Vec<ParsedSymbol>;

    /// Extract references (calls, imports, etc.) from the parse tree
    fn extract_references(&self, source: &str, tree: &Tree) -> Vec<ParsedReference>;
}
