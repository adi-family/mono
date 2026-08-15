// Copyright (c) 2024-2025 Ihor
// SPDX-License-Identifier: BUSL-1.1
// See LICENSE file for details

//! The shutdown signal every adi service waits on.
//!
//! Behind the `signals` feature, because it is the one thing in this crate that needs an async
//! runtime — the rest is pure std, and a crate compiled into hooks and tools should not pull tokio
//! in to get a symlink.

/// Resolves when the process is asked to stop: `SIGTERM` from a supervisor (launchd, systemd,
/// `adi daemon`) or `ctrl-c` from a terminal.
///
/// On a platform without Unix signals only `ctrl-c` is available, which is the whole of the
/// non-Unix implementation.
///
/// **If the `SIGTERM` handler cannot be installed, this degrades to `ctrl-c` and logs it.** The
/// four services that each carried a copy of this function had each answered that question
/// differently — two `expect`ed and so panicked the service at startup over a condition it could
/// survive, one returned a future that never resolves, which is a service that ignores `SIGTERM`
/// for the rest of its life and cannot say why. Degrading is the only one of the three that keeps
/// the service running *and* keeps it stoppable.
pub async fn shutdown_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        match signal(SignalKind::terminate()) {
            Ok(mut term) => {
                tokio::select! {
                    _ = tokio::signal::ctrl_c() => {},
                    _ = term.recv() => {},
                }
            }
            Err(e) => {
                tracing::warn!(error = %e, "could not install SIGTERM handler; using ctrl-c only");
                let _ = tokio::signal::ctrl_c().await;
            }
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}
