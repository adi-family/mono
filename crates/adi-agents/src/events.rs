//! The `adi.agents.*` event payloads as real types, plus the assembly of the whole platform event
//! catalog.
//!
//! Typing the payloads — instead of building an ad-hoc `serde_json::json!` at each emit site — is
//! what lets [`event_types`] publish a JSON Schema guaranteed to match what is emitted: the same
//! struct is both serialized onto the bus and reflected into the schema.
//!
//! [`event_catalog`] is the *entire* catalog — the task events (from `adi-tasks`) followed by the
//! agent events defined here. It is assembled in this crate because this is the lowest one that can
//! see every producer's payload type: `adi-agents` depends on both `adi-events` and `adi-tasks`,
//! while `adi-events` (the bus) sits below both and cannot.

use adi_events::EventType;
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::Value;

use crate::run::Launch;

/// `adi.agents.saved` — an agent definition was created or updated.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct AgentSaved {
    /// The agent's name.
    pub agent: String,
}

/// `adi.agents.deleted` — an agent definition was deleted.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct AgentDeleted {
    /// The agent's name.
    pub agent: String,
}

/// `adi.agents.run.started` — a run was launched, identified by its backend-specific handle (a pty
/// session, or a detached run's pid + run id) so a subscriber can follow the run it just heard
/// about. Tagged by `backend`: `{"backend":"pty",…}` or `{"backend":"process",…}`.
// Internally tagged so the serialized shape matches what the emit site builds from `Launch`.
#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(tag = "backend", rename_all = "snake_case")]
pub enum AgentRunStarted {
    /// An interactive pty-backed run, reachable by its pty session name.
    Pty {
        /// The agent's name.
        agent: String,
        /// The task the run was launched with.
        message: String,
        /// The pty session hosting the run.
        session: String,
    },
    /// A detached headless run, reachable by pid and its own run id.
    Process {
        /// The agent's name.
        agent: String,
        /// The task the run was launched with.
        message: String,
        /// The detached process id.
        pid: u32,
        /// This run's id — its own log/PID slot, independent of the agent's other runs.
        run_id: String,
    },
}

impl AgentRunStarted {
    /// Build the payload for a launched run from its backend handle.
    pub(crate) fn of(name: &str, message: &str, launch: &Launch) -> Self {
        match launch {
            Launch::Pty { session, .. } => Self::Pty {
                agent: name.to_string(),
                message: message.to_string(),
                session: session.clone(),
            },
            Launch::Process { pid, run_id, .. } => Self::Process {
                agent: name.to_string(),
                message: message.to_string(),
                pid: *pid,
                run_id: run_id.clone(),
            },
        }
    }
}

/// `adi.agents.run.stopped` — a running agent, or one specific run of it, was stopped. `run_id` is
/// present only when a single run was targeted; a whole-agent stop omits it.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct AgentRunStopped {
    /// The agent's name.
    pub agent: String,
    /// The stopped run's id, when a specific run (not the whole agent) was targeted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
}

/// The JSON Schema of `T` as a plain `serde_json::Value` for a catalog entry — `to_value` of the
/// reflected schema, so nothing in the catalog is hand-written.
fn schema<T: JsonSchema>() -> Value {
    serde_json::to_value(schemars::schema_for!(T)).unwrap_or(Value::Null)
}

/// This crate's slice of the catalog: the `adi.agents.*` events, each with a schema and example
/// generated from the very type serialized at its emit site.
#[must_use]
pub fn event_types() -> Vec<EventType> {
    vec![
        EventType::of(
            "adi.agents.saved",
            "An agent definition was created or updated.",
            schema::<AgentSaved>(),
            &AgentSaved {
                agent: "my-agent".into(),
            },
        ),
        EventType::of(
            "adi.agents.deleted",
            "An agent definition was deleted.",
            schema::<AgentDeleted>(),
            &AgentDeleted {
                agent: "my-agent".into(),
            },
        ),
        EventType::of(
            "adi.agents.run.started",
            "An agent run was launched.",
            schema::<AgentRunStarted>(),
            &AgentRunStarted::Process {
                agent: "my-agent".into(),
                message: "run".into(),
                pid: 1234,
                run_id: "r-1a2b3c".into(),
            },
        ),
        EventType::of(
            "adi.agents.run.stopped",
            "A running agent (or one of its runs) was stopped.",
            schema::<AgentRunStopped>(),
            &AgentRunStopped {
                agent: "my-agent".into(),
                run_id: Some("r-1a2b3c".into()),
            },
        ),
    ]
}

/// The whole platform event catalog, in reading order: the task events, then the agent events.
/// Assembled here because this is the lowest crate that can see every producer's payload type — the
/// single source of truth behind `adi events types`, `GET /api/triggers` → `event_types`, and the
/// default agent's system prompt.
#[must_use]
pub fn event_catalog() -> Vec<EventType> {
    let mut all = adi_tasks::event_types();
    all.extend(event_types());
    all
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_is_coherent() {
        let catalog = event_catalog();
        assert!(!catalog.is_empty());

        let mut names: Vec<&str> = catalog.iter().map(|e| e.name).collect();
        let count = names.len();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), count, "event names must be unique");

        for e in &catalog {
            assert!(!e.summary.is_empty(), "{} needs a summary", e.name);
            assert!(
                adi_events::validate_name(e.name).is_ok(),
                "{} is not a valid event name",
                e.name
            );
            // The schema must be a real JSON Schema object, and the example a real (non-null) body
            // — both reflected/serialized from the emitted type, so this also proves the type wired
            // up. `deleted`/`stopped` bodies are objects too, never bare scalars.
            assert!(e.schema.is_object(), "{} has a non-object schema", e.name);
            assert!(e.example.is_object(), "{} has a non-object example", e.name);
        }
    }

    #[test]
    fn run_started_matches_launch_variants() {
        // The typed payload must serialize to exactly the expected wire shapes.
        let pty = serde_json::to_value(AgentRunStarted::Pty {
            agent: "a".into(),
            message: "run".into(),
            session: "adi-agent-a".into(),
        })
        .unwrap();
        assert_eq!(pty["backend"], "pty");
        assert_eq!(pty["session"], "adi-agent-a");
        assert!(pty.get("pid").is_none());

        let process = serde_json::to_value(AgentRunStarted::Process {
            agent: "a".into(),
            message: "run".into(),
            pid: 7,
            run_id: "r-1".into(),
        })
        .unwrap();
        assert_eq!(process["backend"], "process");
        assert_eq!(process["pid"], 7);
        assert_eq!(process["run_id"], "r-1");
    }

    #[test]
    fn run_stopped_omits_absent_run_id() {
        let whole = serde_json::to_value(AgentRunStopped {
            agent: "a".into(),
            run_id: None,
        })
        .unwrap();
        assert_eq!(whole, serde_json::json!({ "agent": "a" }));
    }
}
