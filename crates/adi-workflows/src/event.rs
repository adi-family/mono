//! The `adi.workflows.run.*` events — spec section E: "Emit `adi.workflows.run.started/
//! completed/failed`." [`event_types`] is this crate's slice of the whole-platform catalog —
//! see [`event_catalog`]'s own doc for where it's assembled into that whole.
//!
//! Every one of these carries [`caused_by_run`](WorkflowRunStarted::caused_by_run) — Igor's fix
//! for the self-retrigger hazard found while building WF-5's run-starting half: see
//! [`crate::causation`] for what reads it back and why.

use adi_events::{Event, EventType};
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::Value;

/// `adi.workflows.run.started` — a workflow run was poured: its row and root task both exist.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct WorkflowRunStarted {
    /// `${workflow}@${slot}` (schedule), `${workflow}@evt:<seq>` (event),
    /// `${workflow}@manual:<requestId>` (manual), or `${child}@${parentRun}/${key}` (a `ctx.run`
    /// child).
    pub run_key: String,
    /// The workflow id — see [`crate::discover::WorkflowFile::id`].
    pub workflow: String,
    /// The run's root task — every other task of the run (once anything creates one) is its
    /// descendant.
    pub root_task: String,
    /// Why this run started — the same JSON [`crate::store::RunRow::cause`] is stored under.
    pub cause: Value,
    /// The run whose own execution caused this one to start: the parent, for a `ctx.run` child;
    /// propagated from the triggering event's own causer, for an event-started run that was
    /// itself an echo of some run's activity; `None` for a schedule, a manual start, or an event
    /// with no causer of its own.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub caused_by_run: Option<String>,
}

impl Event for WorkflowRunStarted {
    const NAME: &'static str = "adi.workflows.run.started";
}

/// `adi.workflows.run.completed` — a run's workflow body returned, its output validated against
/// its own `output` zod shape, and its root task completed with that value as the result.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct WorkflowRunCompleted {
    pub run_key: String,
    pub workflow: String,
    pub root_task: String,
    /// What the workflow returned.
    pub result: Value,
    /// A run produces this fact about itself — always `Some(run_key)`, kept as its own field
    /// (rather than letting a reader assume `run_key` doubles for it) so [`crate::causation`]'s
    /// "read the causer off any event's payload" stays one uniform lookup across the whole
    /// `adi.workflows.*` family, not a special case for these two.
    pub caused_by_run: String,
}

impl Event for WorkflowRunCompleted {
    const NAME: &'static str = "adi.workflows.run.completed";
}

/// `adi.workflows.run.failed` — a run ended in an error the workflow body never caught: a thrown
/// exception, a reused step key, an `input`/`output` zod mismatch, or an uncaught `TaskGaveUp`.
/// Its root task is given up on (not completed — there is no result that would satisfy its own
/// `output_schema`), with this same message as the reason.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct WorkflowRunFailed {
    pub run_key: String,
    pub workflow: String,
    pub root_task: String,
    /// What went wrong, in one line.
    pub reason: String,
    pub caused_by_run: String,
}

impl Event for WorkflowRunFailed {
    const NAME: &'static str = "adi.workflows.run.failed";
}

fn schema<T: JsonSchema>() -> Value {
    serde_json::to_value(schemars::schema_for!(T)).unwrap_or(Value::Null)
}

/// This crate's slice of the whole-platform catalog — the three `adi.workflows.run.*` events.
/// Composed in with the task/agent/channel events one layer up (see `adi-core`'s own
/// `event_catalog`), the same way `adi_channels::event_types` is: `adi-workflows` doesn't depend
/// on `adi-channels` (nor the reverse), so neither can see the other's producer types, and the
/// first crate that can see every one of task + agent + channel + workflow is the application
/// facade itself.
#[must_use]
pub fn event_types() -> Vec<EventType> {
    vec![
        EventType::of_event(
            "A workflow run started: its row and root task both exist.",
            schema::<WorkflowRunStarted>(),
            &WorkflowRunStarted {
                run_key: "weekly-report@2026-01-05T05:00:00Z".into(),
                workflow: "weekly-report".into(),
                root_task: "t42".into(),
                cause: serde_json::json!({"kind": "schedule", "slot": "2026-01-05T05:00:00Z"}),
                caused_by_run: None,
            },
        ),
        EventType::of_event(
            "A workflow run's body returned, and its root task completed with the result.",
            schema::<WorkflowRunCompleted>(),
            &WorkflowRunCompleted {
                run_key: "weekly-report@2026-01-05T05:00:00Z".into(),
                workflow: "weekly-report".into(),
                root_task: "t42".into(),
                result: serde_json::json!({"posted": true}),
                caused_by_run: "weekly-report@2026-01-05T05:00:00Z".into(),
            },
        ),
        EventType::of_event(
            "A workflow run ended in an error its own body never caught; its root task was given \
             up on with the same reason.",
            schema::<WorkflowRunFailed>(),
            &WorkflowRunFailed {
                run_key: "weekly-report@2026-01-05T05:00:00Z".into(),
                workflow: "weekly-report".into(),
                root_task: "t42".into(),
                reason: "output did not match the workflow's declared shape: /posted: expected boolean"
                    .into(),
                caused_by_run: "weekly-report@2026-01-05T05:00:00Z".into(),
            },
        ),
    ]
}
