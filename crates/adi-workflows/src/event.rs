//! The one event this crate emits so far: `adi.workflows.run.started`, spec section E's "Emit
//! `adi.workflows.run.started`/`completed`/`failed`." Only `started` exists yet — `completed` and
//! `failed` belong to whatever drives a run's root task to a terminal state, which is the
//! execution engine this crate doesn't build yet (see the crate docs).
//!
//! Not yet registered in `adi_agents::event_catalog`'s published catalog — a small follow-up left
//! for whoever wires the execution engine's own events in alongside it, rather than publishing
//! half the family now.

use adi_events::Event;
use serde::Serialize;

/// `adi.workflows.run.started` — a workflow run was poured: its row and root task both exist.
#[derive(Debug, Clone, Serialize)]
pub struct WorkflowRunStarted {
    /// `${workflow}@${slot}` (schedule), `${workflow}@evt:<seq>` (event), or `${workflow}@manual:
    /// <requestId>` (manual — not yet a real starter, see the crate docs).
    pub run_key: String,
    /// The workflow id — see [`crate::discover::WorkflowFile::id`].
    pub workflow: String,
    /// The run's root task — every other task of the run (once anything creates one) is its
    /// descendant.
    pub root_task: String,
    /// Why this run started — the same JSON [`crate::store::RunRow::cause`] is stored under.
    pub cause: serde_json::Value,
}

impl Event for WorkflowRunStarted {
    const NAME: &'static str = "adi.workflows.run.started";
}
