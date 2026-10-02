// Copyright (c) 2024-2025 Ihor
// SPDX-License-Identifier: BUSL-1.1
// See LICENSE file for details

pub mod treesitter;

#[cfg(test)]
mod tests;

#[cfg(all(test, feature = "lang-typescript"))]
mod typescript_tests;

use crate::error::Result;
use crate::types::{Language, ParsedFile};
use std::path::Path;

pub trait Parser: std::fmt::Debug + Send + Sync {
    fn parse(&self, source: &str, language: Language) -> Result<ParsedFile>;

    /// Parse with the file's syntax variant, when the language has more than one grammar.
    /// Custom parsers can keep implementing only `parse`.
    fn parse_for_path(&self, source: &str, language: Language, _path: &Path) -> Result<ParsedFile> {
        self.parse(source, language)
    }

    fn supports(&self, language: Language) -> bool;
}

pub use treesitter::TreeSitterParser;
