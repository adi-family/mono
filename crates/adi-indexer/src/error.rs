// Copyright (c) 2024-2025 Ihor
// SPDX-License-Identifier: BUSL-1.1
// See LICENSE file for details

use thiserror::Error;

#[derive(Error, Debug)]
pub enum Error {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("SQLite error: {0}")]
    Sqlite(#[from] rusqlite::Error),

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("TOML parse error: {0}")]
    TomlParse(#[from] toml::de::Error),

    #[error("Configuration error: {0}")]
    Config(String),

    #[error("Parser error: {0}")]
    Parser(String),

    #[error("Embedding error: {0}")]
    Embedding(String),

    #[error("Index error: {0}")]
    Index(String),

    #[error("Storage error: {0}")]
    Storage(String),

    #[error("Not found: {0}")]
    NotFound(String),

    #[error("Invalid path: {0}")]
    InvalidPath(String),

    #[error("Unsupported language: {0}")]
    UnsupportedLanguage(String),

    #[error("File watcher error: {0}")]
    Watcher(#[from] notify::Error),

    #[error("Embed error: {0}")]
    Embed(#[from] crate::embed::EmbedError),
}

pub type Result<T> = std::result::Result<T, Error>;
