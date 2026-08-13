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
use crate::store::SessionRecord;

use super::{Runner, RunnerKind, detached::DetachedRunner, human::HumanRunner, pty::PtyRunner};

/// The runner that started **this session**, which is not always the one its agent's backend
/// implies.
///
/// Two things make the record the authority rather than the manifest. A simulated run is the same
/// agent, in the same environment, run by a person instead of an engine — same backend, different
/// runner — so the backend cannot answer for it. And an agent whose backend is changed afterwards
/// must not lose the runs it already has: they are still whatever ran them, and asking the new
/// backend about them gets a runner that knows nothing about the session it is being asked to read.
///
/// A record with no runner recorded is every session written before this column existed, and reads
/// as "whatever runs its backend" — which is what those sessions were.
#[must_use]
pub fn runner_of(record: &SessionRecord) -> Option<Box<dyn Runner>> {
    match record.runner.as_ref().map(RunnerKind::as_str) {
        Some(super::human::KIND) => Some(Box::new(HumanRunner::new())),
        _ => runner_for(&record.backend),
    }
}

/// The runner for this backend, or `None` when nothing here runs it.
///
/// The right question when there is no session yet — a launch about to open one, or the Run button
/// asking whether it would work. Once a session exists, ask [`runner_of`] instead.
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

    /// The record wins over the backend. This is the whole of B4: a simulated session carries the
    /// same backend as a real one, and resolving it by backend would hand it the engine's runner —
    /// which would then look for a child that never existed and report the run dead on its first
    /// poll.
    #[test]
    fn a_session_is_resolved_by_the_runner_that_started_it() {
        let mut record = SessionRecord {
            backend: Backend::HarnessAdi,
            runner: Some(RunnerKind::new("human")),
            ..SessionRecord::default()
        };
        assert_eq!(
            runner_of(&record).expect("a runner").kind().as_str(),
            "human",
        );

        // No runner recorded is every session written before the column existed: they read as
        // whatever runs their backend, which is what they were.
        record.runner = None;
        assert_eq!(
            runner_of(&record).expect("a runner").kind().as_str(),
            "detached",
        );

        // And a backend nobody runs still has no runner, however it is asked for.
        record.backend = Backend::from("cloud:worker");
        assert!(runner_of(&record).is_none());
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
