//! Which runner runs a given backend — the one lookup that replaces fourteen `match` arms.
//!
//! This is the *only* place a [`Backend`] is turned into behaviour. Everything above holds a
//! `dyn Runner` and never learns which engine it got, which is what stops "add a backend" from
//! meaning "find every match in the crate and add an arm to it".
//!
//! A backend with no runner is not an error here: an unknown or empty backend has nothing to run at
//! all. It answers `None`, and the caller turns that into
//! [`Error::NotRunnable`](crate::Error::NotRunnable) at the point where it actually matters.

use crate::backend::Backend;

use super::{Runner, detached::DetachedRunner, pty::PtyRunner};

/// The runner for this backend, or `None` when nothing here runs it.
#[must_use]
pub fn runner_for(backend: &Backend) -> Option<Box<dyn Runner>> {
    match backend {
        Backend::PtyClaude | Backend::PtyCodex => {
            Some(Box::new(PtyRunner::new(backend.clone())))
        }
        Backend::ProcessClaude
        | Backend::ProcessCodex
        | Backend::HarnessClaudeSdk
        | Backend::HarnessAdi => Some(Box::new(DetachedRunner::new(backend.clone()))),
        // A plugin backend, or the empty default: kept verbatim through the store, run by nobody.
        Backend::Other(_) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every backend this crate claims to run must resolve to a runner — the honest version of the
    /// old `is_runnable` match, now with nowhere for a backend to be quietly forgotten.
    #[test]
    fn every_runnable_backend_has_a_runner() {
        for wire in [
            "pty:claude",
            "pty:codex",
            "process:claude",
            "process:codex",
            "harness:claude-sdk",
            "harness:adi",
        ] {
            assert!(
                runner_for(&Backend::from(wire)).is_some(),
                "{wire} must resolve to a runner"
            );
        }
    }

    /// A terminal answers the extension; a headless run does not. This is the question call sites ask
    /// instead of matching on the kind, so it is the one worth pinning.
    #[test]
    fn only_terminal_backends_answer_the_terminal_extension() {
        let pty = runner_for(&Backend::PtyClaude).expect("pty runner");
        assert!(pty.as_terminal().is_some());
        assert_eq!(pty.kind().as_str(), "pty");

        let detached = runner_for(&Backend::ProcessClaude).expect("detached runner");
        assert!(detached.as_terminal().is_none());
        assert_eq!(detached.kind().as_str(), "detached");
    }

    #[test]
    fn backends_with_nothing_to_manage_have_no_runner() {
        for wire in ["wasm:loop-script", "cloud:worker", "harness:unknown", ""] {
            assert!(
                runner_for(&Backend::from(wire)).is_none(),
                "{wire} must have no runner"
            );
        }
    }
}
