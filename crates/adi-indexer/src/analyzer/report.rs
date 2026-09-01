// Copyright (c) 2024-2025 Ihor
// SPDX-License-Identifier: BUSL-1.1
// See LICENSE file for details

use crate::types::{Symbol, SymbolId};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ReportFormat {
    Text,
    Json,
    Markdown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeadCodeReport {
    pub dead_symbols: Vec<SymbolId>,
    pub entry_points: Vec<Symbol>,
    pub summary: ReportSummary,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReportSummary {
    pub total_symbols: usize,
    pub reachable: usize,
    pub unreachable: usize,
    pub by_kind: HashMap<String, usize>,
}

impl DeadCodeReport {
    #[must_use]
    pub fn new(
        dead_symbols: Vec<SymbolId>,
        entry_points: Vec<Symbol>,
        total_symbols: usize,
        reachable: usize,
    ) -> Self {
        let unreachable = dead_symbols.len();
        let summary = ReportSummary {
            total_symbols,
            reachable,
            unreachable,
            by_kind: HashMap::new(),
        };

        Self {
            dead_symbols,
            entry_points,
            summary,
        }
    }

    #[must_use]
    pub fn format(&self, format: ReportFormat) -> String {
        match format {
            ReportFormat::Text => self.format_text(),
            ReportFormat::Json => self.format_json(),
            ReportFormat::Markdown => self.format_markdown(),
        }
    }

    fn format_text(&self) -> String {
        let mut output = String::new();
        output.push_str("Dead Code Analysis Report\n");
        output.push_str("========================\n\n");
        output.push_str(&format!(
            "Found {} unused symbols\n\n",
            self.summary.unreachable
        ));
        output.push_str("Summary:\n");
        output.push_str(&format!(
            "  Total symbols: {}\n",
            self.summary.total_symbols
        ));
        output.push_str(&format!("  Reachable: {}\n", self.summary.reachable));
        output.push_str(&format!("  Unreachable: {}\n", self.summary.unreachable));

        output
    }

    fn format_json(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_else(|_| "{}".to_string())
    }

    fn format_markdown(&self) -> String {
        let mut output = String::new();
        output.push_str("# Dead Code Analysis Report\n\n");
        output.push_str(&format!(
            "Found **{}** unused symbols\n\n",
            self.summary.unreachable
        ));
        output.push_str("## Summary\n\n");
        output.push_str(&format!(
            "- Total symbols: {}\n",
            self.summary.total_symbols
        ));
        output.push_str(&format!("- Reachable: {}\n", self.summary.reachable));
        output.push_str(&format!("- Unreachable: {}\n", self.summary.unreachable));

        output
    }
}
