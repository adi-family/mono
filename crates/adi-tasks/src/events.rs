//! adi-tasks' slice of the platform event catalog: the `adi.tasks.*` events, each described by a
//! JSON Schema and a concrete example generated from the very type the store serializes at the emit
//! site — [`TaskView`] for every mutation but `deleted`, [`TaskDeleted`] for that one. Because the
//! schema is reflected from the emitted type, the documented payload can never drift from the real
//! one. The full platform catalog (task events + agent events) is assembled a layer up, in
//! `adi_agents::event_catalog`.

use adi_events::EventType;
use serde_json::Value;

use crate::task::{TaskDeleted, TaskView};

/// The `adi.tasks.*` catalog entries, in reading order. Every mutation but `deleted` carries the
/// resulting [`TaskView`]; `deleted` carries only the id via [`TaskDeleted`].
#[must_use]
pub fn event_types() -> Vec<EventType> {
    // One schema shared by every task-view event; `to_value` of the reflected schema keeps the
    // catalog free of any hand-written JSON.
    let view_schema = serde_json::to_value(schemars::schema_for!(TaskView)).unwrap_or(Value::Null);
    let view = |name, summary| {
        EventType::of(name, summary, view_schema.clone(), &TaskView::example())
    };
    vec![
        view("adi.tasks.created", "A task was created."),
        view(
            "adi.tasks.updated",
            "A task's fields (title, details, tag, assignee, parent) were edited.",
        ),
        view("adi.tasks.completed", "A task was marked done."),
        view("adi.tasks.archived", "A task was archived."),
        view("adi.tasks.reopened", "A done or archived task was reopened."),
        EventType::of(
            "adi.tasks.deleted",
            "A task was permanently deleted (only its id survives).",
            serde_json::to_value(schemars::schema_for!(TaskDeleted)).unwrap_or(Value::Null),
            &TaskDeleted { id: "t1".into() },
        ),
    ]
}
