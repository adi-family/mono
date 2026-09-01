// Copyright (c) 2024-2025 Ihor
// SPDX-License-Identifier: BUSL-1.1
// See LICENSE file for details

use crate::error::Result;
use crate::storage::Storage;
use crate::types::{Symbol, SymbolId};
use std::collections::{HashSet, VecDeque};
use std::sync::Arc;

/// Graph traversal utilities for symbol references
/// Breadth-first walk of the call graph from `id`, tagging every symbol reached with the depth
/// it was first seen at.
///
/// `step` is what makes it a caller or a callee walk — the only thing that differs between the
/// two directions.
fn transitive(
    id: SymbolId,
    storage: &Arc<dyn Storage>,
    max_depth: Option<usize>,
    step: fn(&Arc<dyn Storage>, SymbolId) -> Result<Vec<Symbol>>,
) -> Result<Vec<(Symbol, usize)>> {
    let mut visited: HashSet<i64> = HashSet::new();
    let mut result: Vec<(Symbol, usize)> = Vec::new();
    let mut queue: VecDeque<(SymbolId, usize)> = VecDeque::new();

    queue.push_back((id, 0));
    visited.insert(id.0);

    while let Some((current_id, depth)) = queue.pop_front() {
        if let Some(max) = max_depth
            && depth >= max
        {
            continue;
        }

        for next in step(storage, current_id)? {
            if !visited.contains(&next.id.0) {
                visited.insert(next.id.0);
                result.push((next.clone(), depth + 1));
                queue.push_back((next.id, depth + 1));
            }
        }
    }

    Ok(result)
}

/// Get all transitive callers (symbols that directly or indirectly call the target)
pub fn get_transitive_callers(
    id: SymbolId,
    storage: &Arc<dyn Storage>,
    max_depth: Option<usize>,
) -> Result<Vec<(Symbol, usize)>> {
    transitive(id, storage, max_depth, |s, current| s.get_callers(current))
}

/// Get all transitive callees (symbols that are directly or indirectly called by the target)
pub fn get_transitive_callees(
    id: SymbolId,
    storage: &Arc<dyn Storage>,
    max_depth: Option<usize>,
) -> Result<Vec<(Symbol, usize)>> {
    transitive(id, storage, max_depth, |s, current| s.get_callees(current))
}

/// Detect cycles in the call graph starting from a symbol
pub fn detect_cycles(id: SymbolId, storage: &Arc<dyn Storage>) -> Result<Vec<Vec<Symbol>>> {
    let mut cycles: Vec<Vec<Symbol>> = Vec::new();
    let mut visited: HashSet<i64> = HashSet::new();
    let mut path: Vec<Symbol> = Vec::new();

    fn dfs(
        current_id: SymbolId,
        storage: &Arc<dyn Storage>,
        visited: &mut HashSet<i64>,
        path: &mut Vec<Symbol>,
        cycles: &mut Vec<Vec<Symbol>>,
    ) -> Result<()> {
        let symbol = storage.get_symbol(current_id)?;
        path.push(symbol);

        let callees = storage.get_callees(current_id)?;

        for callee in callees {
            if path.iter().any(|s| s.id == callee.id) {
                // Found a cycle
                let cycle_start_idx = path.iter().position(|s| s.id == callee.id).unwrap();
                let mut cycle: Vec<Symbol> = path[cycle_start_idx..].to_vec();
                cycle.push(callee);
                cycles.push(cycle);
            } else if !visited.contains(&callee.id.0) {
                visited.insert(callee.id.0);
                dfs(callee.id, storage, visited, path, cycles)?;
            }
        }

        path.pop();
        Ok(())
    }

    visited.insert(id.0);
    dfs(id, storage, &mut visited, &mut path, &mut cycles)?;

    Ok(cycles)
}

/// Get usage statistics: how many times each symbol is referenced
pub fn get_usage_stats(storage: &Arc<dyn Storage>) -> Result<Vec<(Symbol, u64)>> {
    let tree = storage.get_tree()?;
    let mut stats: Vec<(Symbol, u64)> = Vec::new();

    for file in &tree.files {
        for symbol_node in &file.symbols {
            let symbol = storage.get_symbol(symbol_node.id)?;
            let count = storage.get_reference_count(symbol_node.id)?;
            if count > 0 {
                stats.push((symbol, count));
            }
        }
    }

    // Sort by reference count descending
    stats.sort_by(|a, b| b.1.cmp(&a.1));

    Ok(stats)
}

/// Find the shortest path between two symbols in the call graph
pub fn find_call_path(
    from_id: SymbolId,
    to_id: SymbolId,
    storage: &Arc<dyn Storage>,
    max_depth: Option<usize>,
) -> Result<Option<Vec<Symbol>>> {
    let mut visited: HashSet<i64> = HashSet::new();
    let mut queue: VecDeque<(SymbolId, Vec<Symbol>)> = VecDeque::new();

    let start_symbol = storage.get_symbol(from_id)?;
    queue.push_back((from_id, vec![start_symbol]));
    visited.insert(from_id.0);

    while let Some((current_id, path)) = queue.pop_front() {
        if let Some(max) = max_depth
            && path.len() > max
        {
            continue;
        }

        if current_id == to_id {
            return Ok(Some(path));
        }

        let callees = storage.get_callees(current_id)?;

        for callee in callees {
            if !visited.contains(&callee.id.0) {
                visited.insert(callee.id.0);
                let mut new_path = path.clone();
                new_path.push(callee.clone());
                queue.push_back((callee.id, new_path));
            }
        }
    }

    Ok(None)
}

/// Every symbol the whole tree over that `keep` accepts, given whether anything calls it and
/// whether it calls anything. The two ends of the call graph are the same sweep with opposite
/// tests, and the sweep is a query per symbol — worth writing once.
fn ends_of_graph(
    storage: &Arc<dyn Storage>,
    keep: impl Fn(bool, bool) -> bool,
) -> Result<Vec<Symbol>> {
    let tree = storage.get_tree()?;
    let mut found: Vec<Symbol> = Vec::new();

    for file in &tree.files {
        for symbol_node in &file.symbols {
            let is_called = !storage.get_callers(symbol_node.id)?.is_empty();
            let calls = !storage.get_callees(symbol_node.id)?.is_empty();

            if keep(is_called, calls) {
                found.push(storage.get_symbol(symbol_node.id)?);
            }
        }
    }

    Ok(found)
}

/// Get symbols that are entry points (no callers but have callees)
pub fn get_entry_points(storage: &Arc<dyn Storage>) -> Result<Vec<Symbol>> {
    ends_of_graph(storage, |is_called, calls| !is_called && calls)
}

/// Get symbols that are leaf nodes (have callers but no callees)
pub fn get_leaf_nodes(storage: &Arc<dyn Storage>) -> Result<Vec<Symbol>> {
    ends_of_graph(storage, |is_called, calls| is_called && !calls)
}

/// Calculate metrics for a symbol in the call graph
#[derive(Debug, Clone)]
pub struct SymbolMetrics {
    pub symbol: Symbol,
    /// Number of direct callers
    pub direct_callers: usize,
    /// Number of direct callees
    pub direct_callees: usize,
    /// Total transitive callers (fan-in)
    pub fan_in: usize,
    /// Total transitive callees (fan-out)
    pub fan_out: usize,
    /// Whether this is an entry point
    pub is_entry_point: bool,
    /// Whether this is a leaf node
    pub is_leaf: bool,
}

pub fn calculate_metrics(
    id: SymbolId,
    storage: &Arc<dyn Storage>,
    max_depth: Option<usize>,
) -> Result<SymbolMetrics> {
    let symbol = storage.get_symbol(id)?;
    let direct_callers = storage.get_callers(id)?;
    let direct_callees = storage.get_callees(id)?;
    let transitive_callers = get_transitive_callers(id, storage, max_depth)?;
    let transitive_callees = get_transitive_callees(id, storage, max_depth)?;

    Ok(SymbolMetrics {
        symbol,
        direct_callers: direct_callers.len(),
        direct_callees: direct_callees.len(),
        fan_in: transitive_callers.len(),
        fan_out: transitive_callees.len(),
        is_entry_point: direct_callers.is_empty() && !direct_callees.is_empty(),
        is_leaf: !direct_callers.is_empty() && direct_callees.is_empty(),
    })
}
