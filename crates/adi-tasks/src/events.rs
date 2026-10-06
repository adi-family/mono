//! adi-tasks' slice of the platform event catalog: the `adi.tasks.*` events, each described by a
//! JSON Schema and a concrete example generated from the very type the store serializes at the emit
//! site — a transparent [`TaskView`] wrapper for every mutation but `deleted`, [`TaskDeleted`]
//! for that one. The schemas reflect the emitted types, so the documented payloads stay in sync.
//! The full platform catalog (task events + agent events) is assembled a layer up, in
//! `adi_agents::event_catalog`.

use adi_events::{Event, EventType};
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::Value;

use crate::task::{TaskDeleted, TaskView};

/// `adi.tasks.created` — a task was created, carrying its resulting view.
#[derive(Debug, Clone, Copy, Serialize, JsonSchema)]
#[serde(transparent)]
pub struct TaskCreated<'a>(pub &'a TaskView);

impl<'a> TaskCreated<'a> {
    /// Wrap the resulting task view without copying it.
    #[must_use]
    pub fn new(view: &'a TaskView) -> Self {
        Self(view)
    }
}

impl Event for TaskCreated<'_> {
    const NAME: &'static str = "adi.tasks.created";
}

/// `adi.tasks.updated` — a task's fields were edited, carrying its resulting view.
#[derive(Debug, Clone, Copy, Serialize, JsonSchema)]
#[serde(transparent)]
pub struct TaskUpdated<'a>(pub &'a TaskView);

impl<'a> TaskUpdated<'a> {
    /// Wrap the resulting task view without copying it.
    #[must_use]
    pub fn new(view: &'a TaskView) -> Self {
        Self(view)
    }
}

impl Event for TaskUpdated<'_> {
    const NAME: &'static str = "adi.tasks.updated";
}

/// `adi.tasks.completed` — a task was marked done, carrying its resulting view.
#[derive(Debug, Clone, Copy, Serialize, JsonSchema)]
#[serde(transparent)]
pub struct TaskCompleted<'a>(pub &'a TaskView);

impl<'a> TaskCompleted<'a> {
    /// Wrap the resulting task view without copying it.
    #[must_use]
    pub fn new(view: &'a TaskView) -> Self {
        Self(view)
    }
}

impl Event for TaskCompleted<'_> {
    const NAME: &'static str = "adi.tasks.completed";
}

/// `adi.tasks.archived` — a task was archived, carrying its resulting view.
#[derive(Debug, Clone, Copy, Serialize, JsonSchema)]
#[serde(transparent)]
pub struct TaskArchived<'a>(pub &'a TaskView);

impl<'a> TaskArchived<'a> {
    /// Wrap the resulting task view without copying it.
    #[must_use]
    pub fn new(view: &'a TaskView) -> Self {
        Self(view)
    }
}

impl Event for TaskArchived<'_> {
    const NAME: &'static str = "adi.tasks.archived";
}

/// `adi.tasks.reopened` — a task was reopened, carrying its resulting view.
#[derive(Debug, Clone, Copy, Serialize, JsonSchema)]
#[serde(transparent)]
pub struct TaskReopened<'a>(pub &'a TaskView);

impl<'a> TaskReopened<'a> {
    /// Wrap the resulting task view without copying it.
    #[must_use]
    pub fn new(view: &'a TaskView) -> Self {
        Self(view)
    }
}

impl Event for TaskReopened<'_> {
    const NAME: &'static str = "adi.tasks.reopened";
}

impl Event for TaskDeleted {
    const NAME: &'static str = "adi.tasks.deleted";
}

/// The `adi.tasks.*` catalog entries, in reading order. Every mutation but `deleted` carries the
/// resulting [`TaskView`]; `deleted` carries only the id via [`TaskDeleted`].
#[must_use]
pub fn event_types() -> Vec<EventType> {
    // The transparent wrappers preserve the existing view payload and its published schema.
    let view_schema = serde_json::to_value(schemars::schema_for!(TaskView)).unwrap_or(Value::Null);
    let view = TaskView::example();
    vec![
        EventType::of_event(
            "A task was created.",
            view_schema.clone(),
            &TaskCreated::new(&view),
        ),
        EventType::of_event(
            "A task's fields (title, details, tag, assignee, parent) were edited.",
            view_schema.clone(),
            &TaskUpdated::new(&view),
        ),
        EventType::of_event(
            "A task was marked done.",
            view_schema.clone(),
            &TaskCompleted::new(&view),
        ),
        EventType::of_event(
            "A task was archived.",
            view_schema.clone(),
            &TaskArchived::new(&view),
        ),
        EventType::of_event(
            "A done or archived task was reopened.",
            view_schema,
            &TaskReopened::new(&view),
        ),
        EventType::of_event(
            "A task was permanently deleted (only its id survives).",
            serde_json::to_value(schemars::schema_for!(TaskDeleted)).unwrap_or(Value::Null),
            &TaskDeleted::new("t1"),
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn task_view_event_catalog_preserves_flat_payloads_and_schemas() {
        let schema = serde_json::to_value(schemars::schema_for!(TaskView)).expect("schema");
        let example = serde_json::to_value(TaskView::example()).expect("example");
        for event in event_types().into_iter().take(5) {
            assert_eq!(event.schema, schema, "{} schema changed", event.name);
            assert_eq!(event.example, example, "{} payload changed", event.name);
        }
    }
}
