// Copyright (c) 2024-2025 Ihor
// SPDX-License-Identifier: BUSL-1.1
// See LICENSE file for details

//! How a supervised adi service logs.

/// Send logs to stdout/stderr for the supervisor to capture, at `info` unless `RUST_LOG` says
/// otherwise.
///
/// Shared rather than per-binary because the level a service defaults to, and the `RUST_LOG` name
/// used to override it, are what someone debugging one reaches for on all of them. A service that
/// quietly defaulted to `warn` would look healthy by saying nothing.
///
/// # Panics
/// If a global subscriber is already installed — this is a `main` calling it once, at startup.
pub fn init_tracing() {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .init();
}
