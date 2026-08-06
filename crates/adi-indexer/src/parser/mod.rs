// Copyright (c) 2024-2025 Ihor
// SPDX-License-Identifier: BUSL-1.1
// See LICENSE file for details

pub mod treesitter;

#[cfg(test)]
mod tests;

use crate::error::Result;
use crate::types::{Language, ParsedFile};

pub trait Parser: std::fmt::Debug + Send + Sync {
    fn parse(&self, source: &str, language: Language) -> Result<ParsedFile>;
    fn supports(&self, language: Language) -> bool;
}

pub use treesitter::TreeSitterParser;
