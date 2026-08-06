// Copyright (c) 2024-2025 Ihor
// SPDX-License-Identifier: BUSL-1.1
// See LICENSE file for details

use thiserror::Error;

#[derive(Error, Debug)]
pub enum EmbedError {
    #[error("Embedding error: {0}")]
    Embedding(String),

    #[error("Config error: {0}")]
    Config(String),

    /// No embedder is compiled into this build (see the `candle` feature).
    #[error("{0}")]
    Unavailable(String),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("TOML parse error: {0}")]
    TomlParse(#[from] toml::de::Error),

    #[error("TOML serialize error: {0}")]
    TomlSerialize(#[from] toml::ser::Error),
}

pub type Result<T> = std::result::Result<T, EmbedError>;
