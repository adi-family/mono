//! The public description of a platform event — one [`EventType`] per event the ADI stack
//! publishes — plus the one-sentence [`ENVELOPE`] that says how every event reaches a subscriber.
//!
//! This low-level bus crate defines the *type* but cannot enumerate the catalog itself: that needs
//! each producer's payload types (the task view lives in `adi-tasks`, the agent payloads in
//! `adi-agents`), which sit *above* this crate. The catalog is therefore assembled where every
//! producer is visible — see [`adi_agents::event_catalog`] — and each entry carries a JSON
//! [`schema`](EventType::schema) and a concrete [`example`](EventType::example) *derived from the
//! exact Rust type serialized at the emit site*, so the documented payload can never drift from
//! what is actually published.

use serde::Serialize;
use serde_json::Value;

/// One kind of event the platform publishes, for display in the editor, the CLI, and the agent's
/// system prompt. A producer builds it from its payload's own Rust type via [`EventType::new`], so
/// the schema and example are generated, never hand-maintained.
#[derive(Debug, Clone, PartialEq)]
pub struct EventType {
    /// The exact event name published, e.g. `adi.tasks.created`. Also what a subscriber types into
    /// an event trigger's patterns — verbatim, or with a `*` (one segment) / `**` (the tail)
    /// wildcard (see [`matches`](crate::matches)).
    pub name: &'static str,
    /// One line on when it fires.
    pub summary: &'static str,
    /// The JSON Schema of the JSON body delivered to a matching trigger as `ADI_PAYLOAD`, generated
    /// from the Rust type the producer serializes. The authoritative description of the payload's
    /// structure — this is what a user or agent reads to learn "what shape will I parse?".
    pub schema: Value,
    /// A concrete example body: a real instance of that same type, serialized. Never written by
    /// hand, so it stays in lockstep with [`schema`](Self::schema) and with what is emitted.
    pub example: Value,
}

impl EventType {
    /// Describe an event from its payload type. Pass `schema` as
    /// `serde_json::to_value(schemars::schema_for!(T))` and `example` as the serialized form of a
    /// real `T` value — both taken from the producer's own type so the two never diverge.
    #[must_use]
    pub fn new(name: &'static str, summary: &'static str, schema: Value, example: Value) -> Self {
        Self {
            name,
            summary,
            schema,
            example,
        }
    }

    /// Build from a live sample `value` of the payload type: derives the [`example`](Self::example)
    /// from it while taking the [`schema`](Self::schema) (which needs the concrete type by name, so
    /// the caller produces it with `schemars::schema_for!`). The most drift-proof entry point — one
    /// value drives the example.
    #[must_use]
    pub fn of<T: Serialize>(
        name: &'static str,
        summary: &'static str,
        schema: Value,
        value: &T,
    ) -> Self {
        Self::new(
            name,
            summary,
            schema,
            serde_json::to_value(value).unwrap_or(Value::Null),
        )
    }
}

/// How every event reaches a subscribed trigger's code block, in one sentence — reused by the UI
/// and the agent prompt so the "format" is described in exactly one place.
pub const ENVELOPE: &str = "Each event reaches a matching event trigger with its name in \
    $ADI_EVENT and its JSON body in $ADI_PAYLOAD (also written to $ADI_PAYLOAD_FILE).";
