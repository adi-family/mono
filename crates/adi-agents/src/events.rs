//! The `adi.agents.*` event payloads as real types, plus the assembly of the whole platform event
//! catalog.
//!
//! Typing the payloads — instead of building an ad-hoc `serde_json::json!` at each emit site — is
//! what lets [`event_types`] publish a JSON Schema guaranteed to match what is emitted: the same
//! struct is both serialized onto the bus and reflected into the schema. Each event also implements
//! [`Event`], so its type owns the topic and builds the complete record without exposing JSON at
//! the publishing site.
//!
//! [`event_catalog`] is the task+agent catalog — the task events (from `adi-tasks`) followed by the
//! agent events defined here. It is assembled in this crate because this is the lowest one that can
//! see both producers' payload types: `adi-agents` depends on both `adi-events` and `adi-tasks`,
//! while `adi-events` (the bus) sits below both and cannot.
//!
//! It is no longer the *whole* platform catalog: `adi-channels` depends on this crate (to dispatch
//! a channel message to an agent), so it sits one layer further up and is where
//! `adi_channels::event_catalog` extends this one with `adi.channels.message`. `adi-core` and the
//! webapp API read that extended catalog; this function stays exactly what it always was.

use adi_events::{Event, EventType};
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

impl Event for AgentSaved {
    const NAME: &'static str = "adi.agents.saved";
}

/// `adi.agents.deleted` — an agent definition was deleted.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct AgentDeleted {
    /// The agent's name.
    pub agent: String,
}

impl Event for AgentDeleted {
    const NAME: &'static str = "adi.agents.deleted";
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

impl Event for AgentRunStarted {
    const NAME: &'static str = "adi.agents.run.started";
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

impl Event for AgentRunStopped {
    const NAME: &'static str = "adi.agents.run.stopped";
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
/// it happened — [`duration_ms`](Self::duration_ms) counts from when the run actually started, not
/// from when anybody looked.
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
    /// How long the run actually took, wall clock, in milliseconds (ADI-MONO-131). A run that spent
    /// most of its life waiting between turns — a pending await, most often — has this far larger
    /// than [`active_ms`](Self::active_ms), and that gap is not a bug: this is "how long it took",
    /// `active_ms` is "how long a model was actually thinking".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
    /// What the engine itself reported for its own active time — `duration_ms`'s entire meaning
    /// before ADI-MONO-131. Absent from an engine that reports no such thing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_ms: Option<u64>,
    /// What it cost, in micro-dollars (1e-6 USD).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_micro_usd: Option<u64>,
    /// The opening of what it answered — enough to tell work from a refusal in a notification.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub result_head: String,
}

impl AgentRunFinished {
    /// A run's verdict, with no optional outcome details. Set the public metadata fields when
    /// available, then use [`Event::to_record`] or [`adi_events::Events::emit_event`].
    #[must_use]
    pub fn new(run_id: impl Into<String>, agent: impl Into<String>, is_error: bool) -> Self {
        Self {
            agent: agent.into(),
            run_id: run_id.into(),
            terminal_reason: None,
            is_error,
            duration_ms: None,
            active_ms: None,
            cost_micro_usd: None,
            result_head: String::new(),
        }
    }
}

impl Event for AgentRunFinished {
    const NAME: &'static str = "adi.agents.run.finished";
}

/// `adi.agents.run.idle` — a turn ended and left the run waiting rather than finished (ADI-MONO-129):
/// a pending await, a queued message still behind it, or an unanswered question are all going to
/// move this conversation again on their own, and this says so on the bus the moment a listing
/// notices it, the same way [`AgentRunFinished`] says the opposite.
///
/// `run.finished` only ever means *really* finished — ADI-MONO-101's distinction stays exactly what
/// it was. This exists because that distinction had a blind spot: a launcher's auto-registered wake
/// matched `run.finished` alone, so a run that ended its turn still waiting on something that was
/// never coming (a dead await, most often) left nothing to tell its launcher so. Matching this event
/// too is what closes that — see `adi-cli`'s `agents run`.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct AgentRunIdle {
    /// The agent's name.
    pub agent: String,
    /// The run that is waiting.
    pub run_id: String,
    /// Always `"waiting"` — spelled out rather than implied, so a subscriber matching on this field
    /// does not have to know this event is only ever published for that one state.
    pub state: String,
    /// One line per pending await, in [`Await::describe`](crate::awaits::Await::describe)'s own
    /// words — what a launcher reads to tell a run waiting on its own background job from one parked
    /// on something that will never come.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pending_awaits: Vec<String>,
    /// The opening of what the turn said before it left the run waiting — same shape as
    /// [`AgentRunFinished::result_head`].
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub result_head: String,
}

impl Event for AgentRunIdle {
    const NAME: &'static str = "adi.agents.run.idle";
}

/// `adi.agents.spawn.refused` — an `agent:<caller>` launch named a target outside the caller's own
/// `can_spawn` (ADI-MONO-113). Published either way, `enforce` or `observe`: the field says which
/// happened, so a subscriber (or an operator watching before flipping the switch) can tell a
/// launch that was actually stopped from one that only would have been.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct AgentSpawnRefused {
    /// The agent that tried to launch another.
    pub caller: String,
    /// The agent it tried to launch.
    pub target: String,
    /// The caller's own run this launch was attempted from, when the launch got far enough to
    /// have one — empty for a refusal under `spawn_policy = "enforce"`, which stops the launch
    /// before a run is created.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub run_id: String,
    /// Whether this refusal actually stopped the launch (`spawn_policy = "enforce"`) or only
    /// noted it (`"observe"`, the default while this rolls out).
    pub enforced: bool,
}

impl Event for AgentSpawnRefused {
    const NAME: &'static str = "adi.agents.spawn.refused";
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

impl Event for AgentRunDeleted {
    const NAME: &'static str = "adi.agents.run.deleted";
}

/// `adi.agents.run.reported` — a run handed over an interim report *on purpose*, via the `Report`
/// tool, without waiting for the run to actually end.
///
/// Named by a constant for the same reason [`QUESTION_ASKED`] is: it is published from inside a
/// turn — the harness loop, or the MCP server serving a Claude/codex engine — and the launcher's
/// own wake (`adi-cli`'s `agents run`) subscribes to the exact spelling.
pub const RUN_REPORTED: &str = AgentRunReported::NAME;

/// The payload of [`RUN_REPORTED`].
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct AgentRunReported {
    /// The agent's name.
    pub agent: String,
    /// The run that reported in.
    pub run_id: String,
    /// The report itself, verbatim.
    pub report: String,
}

impl Event for AgentRunReported {
    const NAME: &'static str = "adi.agents.run.reported";
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
    /// The same conversation, spelled the way [`AgentRunFinished`] and [`AgentRunReported`] spell
    /// it — so a launcher's wake can filter on one field name across all three endings a run may
    /// report back through, rather than knowing that this one event calls it `conv`. Equal to
    /// `conv`, always; `#[serde(default)]` only so a payload from before this field existed still
    /// deserializes.
    #[serde(default)]
    pub run_id: String,
    /// The ask's id.
    pub ask: String,
    /// The first question, plus how many came with it — one line, fit to be a notification.
    pub question: String,
}

impl Event for AgentQuestionAsked {
    const NAME: &'static str = QUESTION_ASKED;
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

impl Event for AgentQuestionAnswered {
    const NAME: &'static str = QUESTION_ANSWERED;
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

impl Event for AgentGoalSet {
    const NAME: &'static str = GOAL_SET;
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

impl Event for AgentGoalNudged {
    const NAME: &'static str = GOAL_NUDGED;
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

/// A goal was judged done. The wrapper owns the topic; its payload stays the shared closure facts.
#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(transparent)]
pub struct AgentGoalMet(pub AgentGoalClosed);

impl Event for AgentGoalMet {
    const NAME: &'static str = GOAL_MET;
}

/// A goal was given up on, with the reason it could not be met.
#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(transparent)]
pub struct AgentGoalGivenUp(pub AgentGoalClosed);

impl Event for AgentGoalGivenUp {
    const NAME: &'static str = GOAL_GIVEN_UP;
}

fn schema<T: JsonSchema>() -> Value {
    serde_json::to_value(schemars::schema_for!(T)).unwrap_or(Value::Null)
}

/// This crate's slice of the catalog: the `adi.agents.*` events, each with a schema and example
/// generated from the very type serialized at its emit site.
#[must_use]
pub fn event_types() -> Vec<EventType> {
    vec![
        EventType::of_event(
            "An agent definition was created or updated.",
            schema::<AgentSaved>(),
            &AgentSaved {
                agent: "my-agent".into(),
            },
        ),
        EventType::of_event(
            "An agent definition was deleted.",
            schema::<AgentDeleted>(),
            &AgentDeleted {
                agent: "my-agent".into(),
            },
        ),
        EventType::of_event(
            "An agent run was launched.",
            schema::<AgentRunStarted>(),
            &AgentRunStarted::Process {
                agent: "my-agent".into(),
                message: "run".into(),
                pid: 1234,
                run_id: "r-1a2b3c".into(),
            },
        ),
        EventType::of_event(
            "A running agent (or one of its runs) was stopped.",
            schema::<AgentRunStopped>(),
            &AgentRunStopped {
                agent: "my-agent".into(),
                run_id: Some("r-1a2b3c".into()),
            },
        ),
        EventType::of_event(
            "An agent run ended on its own, with the engine's verdict on how it went.",
            schema::<AgentRunFinished>(),
            &AgentRunFinished {
                agent: "my-agent".into(),
                run_id: "r-1a2b3c".into(),
                terminal_reason: Some("completed".into()),
                is_error: false,
                duration_ms: Some(189_423),
                active_ms: Some(23_169),
                cost_micro_usd: Some(2_137_364),
                result_head: "Filed three tasks and left a note on the scope.".into(),
            },
        ),
        EventType::of_event(
            "A turn ended and left the run waiting — on a pending await, a queued message, or an \
             unanswered question — rather than really finished.",
            schema::<AgentRunIdle>(),
            &AgentRunIdle {
                agent: "my-agent".into(),
                run_id: "r-1a2b3c".into(),
                state: "waiting".into(),
                pending_awaits: vec!["in 600s, then every 600s, if the check passes".into()],
                result_head: "Started the migration in the background.".into(),
            },
        ),
        EventType::of_event(
            "An agent-to-agent launch named a target outside the caller's own can_spawn.",
            schema::<AgentSpawnRefused>(),
            &AgentSpawnRefused {
                caller: "research-worker".into(),
                target: "billing-admin".into(),
                run_id: "r-1a2b3c".into(),
                enforced: false,
            },
        ),
        EventType::of_event(
            "One run of an agent was deleted, along with everything it kept.",
            schema::<AgentRunDeleted>(),
            &AgentRunDeleted {
                agent: "my-agent".into(),
                run_id: "r-1a2b3c".into(),
            },
        ),
        EventType::of_event(
            "A run handed over an interim report on purpose, via the Report tool, without waiting \
             for the run to end.",
            schema::<AgentRunReported>(),
            &AgentRunReported {
                agent: "my-agent".into(),
                run_id: "r-1a2b3c".into(),
                report: "Phase 1 done: the migration ran clean on staging. Kicking off phase 2 \
                         now, in the background."
                    .into(),
            },
        ),
        EventType::of_event(
            "A run stopped to ask a person a question, and is waiting on the answer.",
            schema::<AgentQuestionAsked>(),
            &AgentQuestionAsked {
                agent: "my-agent".into(),
                conv: "1750000000000-0001".into(),
                run_id: "1750000000000-0001".into(),
                ask: "q-1750000000000-0001".into(),
                question: "Auth method: session cookies or bearer tokens?".into(),
            },
        ),
        EventType::of_event(
            "A run's question was answered and the conversation is moving again.",
            schema::<AgentQuestionAnswered>(),
            &AgentQuestionAnswered {
                agent: "my-agent".into(),
                conv: "1750000000000-0001".into(),
                ask: "q-1750000000000-0001".into(),
                by: "human".into(),
            },
        ),
        EventType::of_event(
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
        EventType::of_event(
            "A conversation fell quiet with a goal still open, and was asked whether it is met.",
            schema::<AgentGoalNudged>(),
            &AgentGoalNudged {
                agent: "my-agent".into(),
                conv: "1750000000000-0001".into(),
                goals: vec!["g-1750000000000-0001".into()],
                nudges: 3,
            },
        ),
        EventType::of_event(
            "A goal was judged done, with whatever evidence was offered for it.",
            schema::<AgentGoalClosed>(),
            &AgentGoalMet(AgentGoalClosed {
                agent: "my-agent".into(),
                conv: "1750000000000-0001".into(),
                goal: "g-1750000000000-0001".into(),
                text: "every flaky test in the suite is either fixed or quarantined".into(),
                note: "12 fixed, 2 quarantined; three green runs in a row".into(),
                nudges: 4,
            }),
        ),
        EventType::of_event(
            "A goal was given up on, with the reason it could not be met.",
            schema::<AgentGoalClosed>(),
            &AgentGoalGivenUp(AgentGoalClosed {
                agent: "my-agent".into(),
                conv: "1750000000000-0001".into(),
                goal: "g-1750000000000-0001".into(),
                text: "every flaky test in the suite is either fixed or quarantined".into(),
                note: "two of them need the staging database, which I cannot reach from here"
                    .into(),
                nudges: 9,
            }),
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
    fn run_finished_builds_a_complete_record() {
        let run_id = "run-\"quoted\"\\path\nnext";
        let agent = "worker-\"quoted\"\\path\nnext";
        for is_error in [false, true] {
            let before = adi_config::now_unix();
            let record = AgentRunFinished::new(run_id, agent, is_error)
                .to_record()
                .expect("run-finished record");
            assert_eq!(record.name, "adi.agents.run.finished");
            assert!((before..=adi_config::now_unix()).contains(&record.emitted_at));
            assert_eq!(
                serde_json::from_str::<Value>(&record.payload).expect("JSON payload"),
                serde_json::json!({
                    "agent": agent,
                    "run_id": run_id,
                    "is_error": is_error,
                })
            );
        }
    }

    #[test]
    fn goal_outcomes_preserve_the_payload_and_select_distinct_topics() {
        let payload = AgentGoalClosed {
            agent: "worker".into(),
            conv: "r-42".into(),
            goal: "g-1".into(),
            text: "ship the fix".into(),
            note: "quoted \"evidence\"\nnext line".into(),
            nudges: 3,
        };
        let expected = serde_json::to_value(&payload).expect("original closure payload");
        let met = AgentGoalMet(payload.clone())
            .to_record()
            .expect("met record");
        let given_up = AgentGoalGivenUp(payload)
            .to_record()
            .expect("give-up record");
        assert_eq!(met.name, "adi.agents.goal.met");
        assert_eq!(given_up.name, "adi.agents.goal.given_up");
        for record in [met, given_up] {
            assert_eq!(
                serde_json::from_str::<Value>(&record.payload).expect("JSON payload"),
                expected,
                "the topic wrapper must not add a field or nesting to the payload"
            );
        }
    }

    #[test]
    fn typed_catalog_preserves_the_published_topics() {
        let catalog = event_types();
        assert_eq!(
            catalog.iter().map(|event| event.name).collect::<Vec<_>>(),
            vec![
                "adi.agents.saved",
                "adi.agents.deleted",
                "adi.agents.run.started",
                "adi.agents.run.stopped",
                "adi.agents.run.finished",
                "adi.agents.run.idle",
                "adi.agents.spawn.refused",
                "adi.agents.run.deleted",
                "adi.agents.run.reported",
                "adi.agents.question.asked",
                "adi.agents.question.answered",
                "adi.agents.goal.set",
                "adi.agents.goal.nudged",
                "adi.agents.goal.met",
                "adi.agents.goal.given_up",
            ]
        );
    }

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
