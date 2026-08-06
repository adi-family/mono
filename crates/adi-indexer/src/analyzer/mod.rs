// Copyright (c) 2024-2025 Ihor
// SPDX-License-Identifier: BUSL-1.1
// See LICENSE file for details

pub mod entry_points;
pub mod filters;
pub mod reachability;
pub mod report;

use crate::error::Result;
use crate::storage::Storage;
use std::sync::Arc;

pub use entry_points::EntryPointDetector;
pub use filters::{AnalysisConfig, AnalysisMode, DeadCodeFilter};
pub use reachability::ReachabilityAnalyzer;
pub use report::{DeadCodeReport, ReportFormat};

#[derive(Debug)]
pub struct DeadCodeAnalyzer {
    storage: Arc<dyn Storage>,
    config: AnalysisConfig,
}

impl DeadCodeAnalyzer {
    pub fn new(storage: Arc<dyn Storage>, config: AnalysisConfig) -> Self {
        Self { storage, config }
    }

    pub fn analyze(&self) -> Result<DeadCodeReport> {
        let entry_detector = EntryPointDetector::new(self.storage.clone());
        let entry_points = entry_detector.detect_entry_points()?;

        let reachability_analyzer = ReachabilityAnalyzer::new(self.storage.clone());
        let reachable = reachability_analyzer.compute_reachability(&entry_points)?;

        let filter = DeadCodeFilter::new(&self.config, self.storage.clone());
        let dead_code = filter.filter_unreachable(&reachable)?;

        // Get all symbols count for the report
        let all_symbols = self.storage.get_all_symbols()?;
        let total_count = all_symbols.len();

        Ok(DeadCodeReport::new(
            dead_code,
            entry_points,
            total_count,
            reachable.len(),
        ))
    }
}
