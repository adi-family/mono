// Copyright (c) 2024-2025 Ihor
// SPDX-License-Identifier: BUSL-1.1
// See LICENSE file for details

use crate::error::Result;
use crate::storage::Storage;
use crate::types::{Symbol, SymbolId};
use std::collections::{HashSet, VecDeque};
use std::sync::Arc;

#[derive(Debug)]
pub struct ReachabilityAnalyzer {
    storage: Arc<dyn Storage>,
}

impl ReachabilityAnalyzer {
    pub fn new(storage: Arc<dyn Storage>) -> Self {
        Self { storage }
    }

    pub fn compute_reachability(&self, entry_points: &[Symbol]) -> Result<HashSet<SymbolId>> {
        let mut reachable = HashSet::new();
        let mut queue = VecDeque::new();

        // Add all entry points to the queue and mark as reachable
        for symbol in entry_points {
            reachable.insert(symbol.id);
            queue.push_back(symbol.id);
        }

        // BFS traversal from entry points
        while let Some(symbol_id) = queue.pop_front() {
            // Get all symbols that this symbol calls/references
            if let Ok(callees) = self.storage.get_callees(symbol_id) {
                for callee in callees {
                    if !reachable.contains(&callee.id) {
                        reachable.insert(callee.id);
                        queue.push_back(callee.id);
                    }
                }
            }
        }

        Ok(reachable)
    }

    pub fn is_reachable(&self, symbol_id: SymbolId, entry_points: &[Symbol]) -> Result<bool> {
        let reachable = self.compute_reachability(entry_points)?;
        Ok(reachable.contains(&symbol_id))
    }
}
