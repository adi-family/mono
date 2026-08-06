// Copyright (c) 2024-2025 Ihor
// SPDX-License-Identifier: BUSL-1.1
// See LICENSE file for details

use crate::error::Result;
use crate::storage::Storage;
use crate::types::{SymbolId, Symbol, SymbolKind};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::sync::Arc;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AnalysisMode {
    Strict,
    Library,
    Application,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnalysisConfig {
    pub mode: AnalysisMode,
    pub exclude_tests: bool,
    pub exclude_traits: bool,
    pub exclude_ffi: bool,
    pub exclude_patterns: Vec<String>,
}

impl Default for AnalysisConfig {
    fn default() -> Self {
        Self {
            mode: AnalysisMode::Strict,
            exclude_tests: true,
            exclude_traits: true,
            exclude_ffi: true,
            exclude_patterns: vec![],
        }
    }
}

#[derive(Debug)]
pub struct DeadCodeFilter<'a> {
    config: &'a AnalysisConfig,
    storage: Arc<dyn Storage>,
}

impl<'a> DeadCodeFilter<'a> {
    pub fn new(config: &'a AnalysisConfig, storage: Arc<dyn Storage>) -> Self {
        Self { config, storage }
    }

    pub fn filter_unreachable(&self, reachable: &HashSet<SymbolId>) -> Result<Vec<SymbolId>> {
        // Get all symbols from storage
        let all_symbols = self.storage.get_all_symbols()?;

        // Filter to find unreachable symbols
        let mut unreachable = Vec::new();

        for symbol in all_symbols {
            // Skip if reachable
            if reachable.contains(&symbol.id) {
                continue;
            }

            // Skip if excluded by config
            if self.should_exclude(&symbol) {
                continue;
            }

            unreachable.push(symbol.id);
        }

        Ok(unreachable)
    }

    #[must_use]
    pub fn should_exclude(&self, symbol: &Symbol) -> bool {
        // Exclude test functions if configured
        if self.config.exclude_tests && self.is_test_symbol(symbol) {
            return true;
        }

        // Exclude trait implementations if configured
        if self.config.exclude_traits && self.is_trait_impl(symbol) {
            return true;
        }

        // Exclude FFI symbols if configured
        if self.config.exclude_ffi && self.is_ffi_symbol(symbol) {
            return true;
        }

        // Exclude symbols matching configured patterns
        for pattern in &self.config.exclude_patterns {
            if symbol.name.contains(pattern) {
                return true;
            }
        }

        false
    }

    fn is_test_symbol(&self, symbol: &Symbol) -> bool {
        if let Some(doc) = &symbol.doc_comment
            && (doc.contains("#[test]") || doc.contains("@Test")) {
                return true;
            }

        symbol.name.starts_with("test_") || symbol.name.starts_with("Test")
    }

    fn is_trait_impl(&self, symbol: &Symbol) -> bool {
        matches!(symbol.kind, SymbolKind::Trait)
    }

    fn is_ffi_symbol(&self, symbol: &Symbol) -> bool {
        if let Some(signature) = &symbol.signature {
            signature.contains("extern") || signature.contains("ffi")
        } else {
            false
        }
    }
}
