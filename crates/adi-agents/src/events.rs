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

/// `adi.agents.run.finished` — a run ended on its own, and here is what became of it.
///
/// The counterpart `stopped` never was: that one says *somebody stopped it*, so a run that failed
/// at four in the morning published nothing at all and the only way to learn of it was to go and
/// look. Every watcher therefore polled, and polling a conversation costs a whole turn. This is the
/// event that makes not-looking possible — it carries the verdict, so a subscriber decides whether
/// to care without opening anything.
///
/// Published when the ending is **noticed**, which for a run nobody was watching is later than when
/// it happened; [`duration_ms`](Self::duration_ms) is the run's own, not the wait.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct AgentRunFinished {
    /// The agent's name.
    pub agent: String,
    /// The run that ended.
    pub run_id: String,
    /// The engine's own word for how it ended — `completed`, `api_error`, `aborted_tools`, and so
    /// on. Absent from an engine that reports no such thing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminal_reason: Option<String>,
    /// Whether the engine called it a failure. The one field a trigger can match on without
    /// knowing any engine's vocabulary.
    pub is_error: bool,
    /// How long the run took, in milliseconds, as the engine reported it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
    /// What it cost, in micro-dollars (1e-6 USD).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_micro_usd: Option<u64>,
    /// The opening of what it answered — enough to tell work from a refusal in a notification.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub result_head: String,
}

/// `adi.agents.run.deleted` — one run of an agent was deleted outright: it was stopped if still
/// live, and its log, metadata and any transcript are gone. Distinct from `stopped`, which ends a
/// run but leaves it in the history to read.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct AgentRunDeleted {
    /// The agent's name.
    pub agent: String,
    /// The deleted run's id.
    pub run_id: String,
}

/// `adi.agents.question.asked` — a run stopped to ask a person something and ended its turn.
///
/// Named by a constant rather than spelled at each site because, unlike every other event here,
/// this one is emitted from a *child process* — the harness turn, or the MCP server serving a
/// Claude engine — and matched by whatever forwards it to a human. Three spellings that have to
/// agree is two too many.
pub const QUESTION_ASKED: &str = "adi.agents.question.asked";

/// `adi.agents.question.answered` — the question was settled and the conversation is moving again.
pub const QUESTION_ANSWERED: &str = "adi.agents.question.answered";

/// The payload of [`QUESTION_ASKED`]. Carries the headline rather than the whole ask: a subscriber
/// is deciding whether to interrupt somebody, and reads the rest in the app if it does.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct AgentQuestionAsked {
    /// The agent that asked.
    pub agent: String,
    /// The conversation blocked on the answer — what an answer must be delivered into.
    pub conv: String,
    /// The ask's id.
    pub ask: String,
    /// The first question, plus how many came with it — one line, fit to be a notification.
    pub question: String,
}

/// The payload of [`QUESTION_ANSWERED`].
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct AgentQuestionAnswered {
    pub agent: String,
    pub conv: String,
    pub ask: String,
    /// `human` when somebody answered, `default` when the deadline passed and the run's own
    /// assumption was taken. Worth publishing: a fleet where most asks time out is a fleet asking
    /// the wrong questions, or asking nobody.
    pub by: String,
}

/// `adi.agents.goal.set` — a goal was written onto a conversation, by a person or by the run
/// itself.
///
/// Named by constants for the same reason the question events are: a run sets and closes its own
/// goals from a *child process* — the CLI it runs from inside a turn — so the spelling has to agree
/// across two binaries.
pub const GOAL_SET: &str = "adi.agents.goal.set";

/// `adi.agents.goal.nudged` — a conversation fell quiet with a goal still open, and was asked about
/// it.
pub const GOAL_NUDGED: &str = "adi.agents.goal.nudged";

/// `adi.agents.goal.met` — somebody judged the goal done.
pub const GOAL_MET: &str = "adi.agents.goal.met";

/// `adi.agents.goal.given_up` — somebody judged it not going to be done, and said why.
pub const GOAL_GIVEN_UP: &str = "adi.agents.goal.given_up";

/// The payload of [`GOAL_SET`].
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct AgentGoalSet {
    pub agent: String,
    /// The conversation the goal is nudged into.
    pub conv: String,
    /// The goal's id — what closes it.
    pub goal: String,
    /// What done means, in the words it was set in.
    pub text: String,
    /// `human` or `agent`. A run that set its own goal has decided what it is doing, which reads
    /// differently from one that was told — and a fleet full of the latter is worth noticing.
    pub set_by: String,
}

/// The payload of [`GOAL_NUDGED`].
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct AgentGoalNudged {
    pub agent: String,
    pub conv: String,
    /// The ids put to the conversation — all of its open goals travel in one message.
    pub goals: Vec<String>,
    /// How many times this conversation's oldest open goal has now been put. Published because a
    /// number that keeps climbing is the signal that a run is circling rather than converging, and
    /// nothing in the platform will stop it: only `met` and `knowingly-give-up` close a goal.
    pub nudges: u64,
}

/// The payload of [`GOAL_MET`] and [`GOAL_GIVEN_UP`] alike — the same facts either way, and the
/// event name carries which happened.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct AgentGoalClosed {
    pub agent: String,
    pub conv: String,
    pub goal: String,
    pub text: String,
    /// The evidence a `met` carried, or the reason a give-up did.
    pub note: String,
    /// How many times it had been put to the conversation before it closed.
    pub nudges: u64,
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
        EventType::of(
            "adi.agents.run.finished",
            "An agent run ended on its own, with the engine's verdict on how it went.",
            schema::<AgentRunFinished>(),
            &AgentRunFinished {
                agent: "my-agent".into(),
                run_id: "r-1a2b3c".into(),
                terminal_reason: Some("completed".into()),
                is_error: false,
                duration_ms: Some(23_169),
                cost_micro_usd: Some(2_137_364),
                result_head: "Filed three tasks and left a note on the scope.".into(),
            },
        ),
        EventType::of(
            "adi.agents.run.deleted",
            "One run of an agent was deleted, along with everything it kept.",
            schema::<AgentRunDeleted>(),
            &AgentRunDeleted {
                agent: "my-agent".into(),
                run_id: "r-1a2b3c".into(),
            },
        ),
        EventType::of(
            QUESTION_ASKED,
            "A run stopped to ask a person a question, and is waiting on the answer.",
            schema::<AgentQuestionAsked>(),
            &AgentQuestionAsked {
                agent: "my-agent".into(),
                conv: "1750000000000-0001".into(),
                ask: "q-1750000000000-0001".into(),
                question: "Auth method: session cookies or bearer tokens?".into(),
            },
        ),
        EventType::of(
            QUESTION_ANSWERED,
            "A run's question was answered and the conversation is moving again.",
            schema::<AgentQuestionAnswered>(),
            &AgentQuestionAnswered {
                agent: "my-agent".into(),
                conv: "1750000000000-0001".into(),
                ask: "q-1750000000000-0001".into(),
                by: "human".into(),
            },
        ),
        EventType::of(
            GOAL_SET,
            "A goal was written onto a conversation — what would make it done.",
            schema::<AgentGoalSet>(),
            &AgentGoalSet {
                agent: "my-agent".into(),
                conv: "1750000000000-0001".into(),
                goal: "g-1750000000000-0001".into(),
                text: "every flaky test in the suite is either fixed or quarantined".into(),
                set_by: "human".into(),
            },
        ),
        EventType::of(
            GOAL_NUDGED,
            "A conversation fell quiet with a goal still open, and was asked whether it is met.",
            schema::<AgentGoalNudged>(),
            &AgentGoalNudged {
                agent: "my-agent".into(),
                conv: "1750000000000-0001".into(),
                goals: vec!["g-1750000000000-0001".into()],
                nudges: 3,
            },
        ),
        EventType::of(
            GOAL_MET,
            "A goal was judged done, with whatever evidence was offered for it.",
            schema::<AgentGoalClosed>(),
            &AgentGoalClosed {
                agent: "my-agent".into(),
                conv: "1750000000000-0001".into(),
                goal: "g-1750000000000-0001".into(),
                text: "every flaky test in the suite is either fixed or quarantined".into(),
                note: "12 fixed, 2 quarantined; three green runs in a row".into(),
                nudges: 4,
            },
        ),
        EventType::of(
            GOAL_GIVEN_UP,
            "A goal was given up on, with the reason it could not be met.",
            schema::<AgentGoalClosed>(),
            &AgentGoalClosed {
                agent: "my-agent".into(),
                conv: "1750000000000-0001".into(),
                goal: "g-1750000000000-0001".into(),
                text: "every flaky test in the suite is either fixed or quarantined".into(),
                note: "two of them need the staging database, which I cannot reach from here"
                    .into(),
                nudges: 9,
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
            assert!(e.schema.is_object(), "{} has a non-object schema", e.name);
            assert!(e.example.is_object(), "{} has a non-object example", e.name);
        }
    }

    #[test]
    fn run_started_matches_launch_variants() {
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
