// Copyright (c) 2024-2025 Ihor
// SPDX-License-Identifier: BUSL-1.1
// See LICENSE file for details

use crate::error::Result;
use crate::storage::Storage;
use crate::types::{Symbol, SymbolKind, Visibility};
use std::sync::Arc;

#[derive(Debug)]
pub struct EntryPointDetector {
    storage: Arc<dyn Storage>,
}

impl EntryPointDetector {
    pub fn new(storage: Arc<dyn Storage>) -> Self {
        Self { storage }
    }

    pub fn detect_entry_points(&self) -> Result<Vec<Symbol>> {
        let all_symbols = self.storage.get_all_symbols()?;
        let mut entry_points = Vec::new();

        for symbol in all_symbols {
            if self.is_entry_point(&symbol) {
                entry_points.push(symbol);
            }
        }

        Ok(entry_points)
    }

    fn is_entry_point(&self, symbol: &Symbol) -> bool {
        if symbol.is_entry_point {
            return true;
        }

        // Language-specific entry point detection
        if self.is_main_function(symbol) {
            return true;
        }

        if self.is_test_function(symbol) {
            return true;
        }

        if self.is_public_export(symbol) {
            return true;
        }

        false
    }

    fn is_main_function(&self, symbol: &Symbol) -> bool {
        if symbol.name == "main" && matches!(symbol.kind, SymbolKind::Function) {
            return true;
        }
        false
    }

    fn is_test_function(&self, symbol: &Symbol) -> bool {
        if let Some(doc) = &symbol.doc_comment
            && (doc.contains("#[test]") || doc.contains("@Test")) {
                return true;
            }

        if symbol.name.starts_with("test_") || symbol.name.starts_with("Test") {
            return true;
        }

        false
    }

    fn is_public_export(&self, symbol: &Symbol) -> bool {
        // Check if the symbol is a public export
        match symbol.visibility {
            Visibility::Public => {
                // For library mode, all public symbols are entry points
                matches!(
                    symbol.kind,
                    SymbolKind::Function
                        | SymbolKind::Struct
                        | SymbolKind::Class
                        | SymbolKind::Trait
                        | SymbolKind::Interface
                        | SymbolKind::Enum
                        | SymbolKind::Constant
                        | SymbolKind::Type
                )
            }
            _ => false,
        }
    }
}
