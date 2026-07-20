//! The catalog of platform events the ADI stack itself publishes — the answer to "what events
//! exist and how do I catch them?".
//!
//! Every entry is a concrete event [`name`](EventType::name) (a dotted topic), one line on when it
//! fires, and a compact example of the JSON [`payload`](EventType::payload). A subscriber names the
//! event in an event trigger's patterns — verbatim, or with a `*`/`**` wildcard (see
//! [`matches`](crate::matches)) — and reads the payload from `ADI_PAYLOAD` in its code block.
//!
//! This is documentation, kept deliberately next to nothing that produces events: the producers
//! (the task store, the agent registry, the CLI) each `emit` independently, so this list is a hand-
//! maintained index of the taxonomy rather than something derived. Keep it in sync when a producer
//! gains or renames an event.

/// One kind of event the platform publishes, for display in the editor, the CLI, and the agent's
/// system prompt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EventType {
    /// The exact event name published, e.g. `adi.tasks.created`. Also what a subscriber types
    /// into an event trigger's patterns — verbatim, or with a `*` (one segment) / `**` (the tail)
    /// wildcard.
    pub name: &'static str,
    /// One line on when it fires.
    pub summary: &'static str,
    /// A compact example of the JSON body delivered to a matching trigger as `ADI_PAYLOAD`.
    pub payload: &'static str,
}

/// How every event reaches a subscribed trigger's code block, in one sentence — reused by the UI
/// and the agent prompt so the "format" is described in exactly one place.
pub const ENVELOPE: &str = "Each event reaches a matching event trigger with its name in \
    $ADI_EVENT and its JSON body in $ADI_PAYLOAD (also written to $ADI_PAYLOAD_FILE).";

/// The task view (`adi.tasks.*`, except `deleted`) carries the whole task, flattened, plus its
/// computed status — the same shape the `/api/tasks` list returns.
const TASK_VIEW: &str = "{\"id\":\"t1\",\"title\":\"ship it\",\"status\":\"open\",\
\"project\":null,\"parent\":null,\"tag\":null,\"assignee\":null,\
\"effective\":\"ready\",\"children_total\":0,\"children_open\":0}";

/// Every event the platform publishes, grouped by producer, in a sensible reading order.
static CATALOG: &[EventType] = &[
    // Tasks — the task tree. Every mutation but `deleted` carries the resulting task view.
    EventType {
        name: "adi.tasks.created",
        summary: "A task was created.",
        payload: TASK_VIEW,
    },
    EventType {
        name: "adi.tasks.updated",
        summary: "A task's fields (title, details, tag, assignee, parent) were edited.",
        payload: TASK_VIEW,
    },
    EventType {
        name: "adi.tasks.completed",
        summary: "A task was marked done.",
        payload: TASK_VIEW,
    },
    EventType {
        name: "adi.tasks.archived",
        summary: "A task was archived.",
        payload: TASK_VIEW,
    },
    EventType {
        name: "adi.tasks.reopened",
        summary: "A done or archived task was reopened.",
        payload: TASK_VIEW,
    },
    EventType {
        name: "adi.tasks.deleted",
        summary: "A task was permanently deleted (only its id survives).",
        payload: "{\"id\":\"t1\"}",
    },
    // Agents — agent definitions and their runs.
    EventType {
        name: "adi.agents.saved",
        summary: "An agent definition was created or updated.",
        payload: "{\"agent\":\"my-agent\"}",
    },
    EventType {
        name: "adi.agents.deleted",
        summary: "An agent definition was deleted.",
        payload: "{\"agent\":\"my-agent\"}",
    },
    EventType {
        name: "adi.agents.run.started",
        summary: "An agent run was launched.",
        payload: "{\"agent\":\"my-agent\",\"message\":\"run\",\"backend\":\"process\",\
\"pid\":1234,\"run_id\":\"r-…\"}",
    },
    EventType {
        name: "adi.agents.run.stopped",
        summary: "A running agent (or one of its runs) was stopped.",
        payload: "{\"agent\":\"my-agent\",\"run_id\":\"r-…\"}",
    },
];

/// The full event catalog, in reading order.
#[must_use]
pub fn catalog() -> &'static [EventType] {
    CATALOG
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_is_coherent() {
        assert!(!catalog().is_empty());
        let mut names: Vec<&str> = catalog().iter().map(|e| e.name).collect();
        let count = names.len();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), count, "event names must be unique");
        for e in catalog() {
            assert!(!e.summary.is_empty(), "{} needs a summary", e.name);
            assert!(
                crate::validate_name(e.name).is_ok(),
                "{} is not a valid event name",
                e.name
            );
            // Every payload example must be valid JSON — it is shown as "this is what you'll parse".
            serde_json::from_str::<serde_json::Value>(e.payload)
                .unwrap_or_else(|e2| panic!("{} has a non-JSON payload example: {e2}", e.name));
        }
    }
}
