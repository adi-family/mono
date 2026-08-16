//! Goals — what a conversation is for, and the sweep that keeps asking about it.
//!
//! A turn ends when the model stops calling tools. That is a statement about the turn, not about
//! the work: the run may have finished, or merely paused at the end of a thought. A **goal** is a
//! sentence saying what done means, kept beside the conversation and re-put to it every time it
//! falls quiet — until somebody closes it.
//!
//! ```text
//!   create ──▶ OPEN ──idle──▶ nudge ──▶ turn ──▶ idle ──▶ nudge ──▶ …
//!                │
//!                ├── goals met <id>               ──▶ MET
//!                └── goals knowingly-give-up <id> ──▶ GIVEN_UP
//! ```
//!
//! # Nothing here decides a goal is over
//!
//! There is no attempt cap and no sweep that gives up on the run's behalf. The two verbs are the
//! only exits and both are somebody's judgement, written down — a run that cannot get there says so
//! with `knowingly-give-up` and a reason, which is a sentence worth reading afterward in a way that
//! "exceeded 10 attempts" never is. The consequence is deliberate and worth stating plainly: a run
//! that neither finishes nor gives up is asked again for as long as its conversation exists.
//!
//! The one thing that *is* enforced is [`NUDGE_FLOOR_MS`], and it is not a limit on the run — it is
//! a limit on this sweep, so an engine that dies on startup cannot be respawned as fast as `send`
//! returns.
//!
//! # A run can set its own goal
//!
//! `goals create` with no `--agent`/`--session` reads `ADI_AGENT` and `ADI_RUN_ID` out of the
//! environment every turn is given, so a run can write down what it is doing and then be held to
//! it. That closes a loop the platform cannot: an agent that sets itself a goal mid-turn is nudged
//! the moment that turn ends, and keeps itself going without anybody watching. It is also the one
//! way a run can put itself in a cycle that only its own give-up breaks, which is the price of the
//! rule above and not a bug in it.
//!
//! # Where the clock comes from
//!
//! Like [`awaits`](crate::awaits) and [`questions`](crate::questions), this crate holds the store
//! and the decision, and the app owns the tick — [`tick`] rides the same once-a-second worker
//! (`adi-app/src/awaits.rs`). Nothing here polls, listens, or spawns.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use crate::awaits::Awaits;
use crate::error::Result;
use crate::events::{
    AgentGoalClosed, AgentGoalNudged, AgentGoalSet, GOAL_GIVEN_UP, GOAL_MET, GOAL_NUDGED, GOAL_SET,
};
use crate::runner::runner_of;
use crate::store::{Goal, GoalClosed, GoalState, SetBy, now_ms};
use crate::{Agents, answerable};

/// The shortest gap between two nudges into the same conversation.
///
/// Not a leash on the run — see the module note. A nudge ends up as a turn, and a turn that fails
/// to start (a misconfigured engine, a binary that is not there) leaves the conversation idle again
/// within milliseconds; without a floor the sweep would re-ask at its own tick rate for as long as
/// that lasted. Thirty seconds is short enough to be invisible behind real work and long enough
/// that a crash loop is a trickle rather than a flood.
pub const NUDGE_FLOOR_MS: u64 = 30_000;

/// One conversation that was asked about its goals.
#[derive(Debug, Clone)]
pub struct Nudged {
    pub agent: String,
    pub conv: String,
    /// The goal ids put to it — every open goal of the conversation travels in one message.
    pub goals: Vec<String>,
    /// Set when the nudge was stamped but could not be delivered. The stamp stands either way, so
    /// a conversation whose engine is broken is re-asked on the floor rather than every tick.
    pub error: Option<String>,
}

/// Ask every conversation that has fallen quiet with a goal still open whether it is met.
///
/// Called by the app on its own clock. A pass over a store with nothing open is one indexed query.
#[must_use]
pub fn tick(agents: &Agents) -> Vec<Nudged> {
    let store = agents.sessions();
    let awaits = Awaits::with_config(agents.config().clone());
    let now = now_ms();

    // Grouped before anything is asked: all of a conversation's open goals go in one message, so
    // the idle check — which costs a liveness probe and three queries — is paid once per
    // conversation rather than once per goal.
    let mut by_conversation: BTreeMap<(String, String), Vec<Goal>> = BTreeMap::new();
    for goal in store.all_open_goals() {
        by_conversation
            .entry((goal.agent.clone(), goal.conv.clone()))
            .or_default()
            .push(goal);
    }

    let mut nudged = Vec::new();
    for ((agent, conv), goals) in by_conversation {
        if !quiet(agents, &awaits, &agent, &conv, &goals, now) {
            continue;
        }
        // Stamped before the message goes out, for the reason the `error` field exists: a delivery
        // that fails must still push the next attempt out by the floor, or a conversation nothing
        // can be delivered into is retried at the tick rate for ever.
        if store.mark_goals_nudged(&agent, &conv, now).is_err() {
            continue;
        }
        let error = agents
            .deliver(&agent, &conv, &nudge_message(&goals))
            .err()
            .map(|e| e.to_string());
        agents.emit(
            GOAL_NUDGED,
            &AgentGoalNudged {
                agent: agent.clone(),
                conv: conv.clone(),
                goals: goals.iter().map(|g| g.id.clone()).collect(),
                // +1 for the stamp just written: these copies were read before it.
                nudges: goals.first().map_or(0, |g| g.nudges + 1),
            },
        );
        nudged.push(Nudged {
            agent,
            conv,
            goals: goals.into_iter().map(|g| g.id).collect(),
            error,
        });
    }
    nudged
}

/// Whether this conversation has actually fallen quiet — the whole of the condition for a nudge.
///
/// Every clause is a different way of *not* being finished, and each one exists because asking
/// through it would be wrong rather than merely noisy:
///
/// * a turn is in flight — the run is answering; the goal is what it is answering about;
/// * something is queued — somebody is already mid-sentence with this conversation, and a nudge
///   would jump the line;
/// * a question is pending — the run stopped to ask a *person* something, and the answer to "is
///   your goal met" is "I am waiting on you", which it has already said;
/// * an await is registered — the run has said in its own words what it is waiting for and asked to
///   be woken then. Nudging over the top of that is asking a run that already has a plan to
///   re-explain itself.
fn quiet(
    agents: &Agents,
    awaits: &Awaits,
    agent: &str,
    conv: &str,
    goals: &[Goal],
    now: u64,
) -> bool {
    // The floor first: it is two subtractions, and it is false most of the time a conversation is
    // being worked on, which saves every query below it.
    if goals
        .iter()
        .any(|g| now.saturating_sub(g.last_nudge_at) < NUDGE_FLOOR_MS)
    {
        return false;
    }
    let store = agents.sessions();
    let Some(record) = store.get(agent, conv) else {
        return false;
    };
    // Asked of the runner that started *this* conversation, not of the agent's current backend —
    // the two disagree for a simulated run, and a goal set on one is still a goal.
    let Some(runner) = runner_of(&record) else {
        return false;
    };
    // A conversation nothing can be said into cannot be nudged. The goal stays open: it is a
    // record of what the run was for, and re-pointing the agent at an answerable backend makes it
    // live again.
    if !answerable(runner.as_ref()) {
        return false;
    }
    if runner.is_alive(&store.session(agent, conv)) {
        return false;
    }
    if store.queue_len(agent, conv) > 0 {
        return false;
    }
    if store.pending_question(agent, conv).is_some() {
        return false;
    }
    awaits.for_conversation(agent, conv).is_empty()
}

/// The user turn a nudge delivers.
///
/// Written to be read by a model replaying a long transcript, so it repeats the goal in full rather
/// than referring to it: the turn that set it may be hundreds of messages back, or may have been a
/// person typing into a box the model never saw.
///
/// When there is one goal its id is substituted straight into both commands, because a command that
/// can be run as printed is the difference between a run that closes its goal and a run that writes
/// a paragraph about having closed it. With several there is no single id to substitute, so the
/// placeholder stands and the ids are listed directly above it.
fn nudge_message(goals: &[Goal]) -> String {
    let mut text = String::from("[goal check]\n\n");
    let plural = if goals.len() == 1 { "" } else { "s" };
    let _ = writeln!(
        text,
        "This conversation has fallen quiet with {} open goal{plural}, and nothing is queued or \
         waiting on anybody. Is it met?\n",
        goals.len()
    );
    for goal in goals {
        let _ = writeln!(text, "  {}", goal.id);
        for line in goal.text.lines() {
            let _ = writeln!(text, "    {line}");
        }
        text.push('\n');
    }
    let id = match goals {
        [only] => only.id.as_str(),
        _ => "<id>",
    };
    let _ = write!(
        text,
        "If it is met, say so with the evidence:\n  \
         adi-mono goals met {id} --evidence \"what shows it\"\n\n\
         If it cannot be met and you are stopping work on it, say why:\n  \
         adi-mono goals knowingly-give-up {id} --why \"what stopped you\"\n\n\
         Otherwise carry on working toward it — you will be asked again when you next fall quiet. \
         Nothing closes a goal except those two commands, so a goal you neither meet nor give up \
         on is one you will be asked about for ever."
    );
    text
}

/// Write a goal onto a conversation.
///
/// `set_by` is what tells a goal a person set apart from one the run wrote for itself; both are
/// ordinary goals and are nudged identically.
///
/// # Errors
/// [`Error::Arguments`](crate::error::Error::Arguments) for empty or oversized text, and database
/// errors. Notably *not* an error: a conversation that already has goals — see
/// [`store::goals`](crate::store).
pub fn create(agents: &Agents, agent: &str, conv: &str, text: &str, set_by: SetBy) -> Result<Goal> {
    let goal = agents.sessions().create_goal(agent, conv, text, set_by)?;
    agents.emit(
        GOAL_SET,
        &AgentGoalSet {
            agent: goal.agent.clone(),
            conv: goal.conv.clone(),
            goal: goal.id.clone(),
            text: goal.text.clone(),
            set_by: goal.set_by.as_str().to_string(),
        },
    );
    Ok(goal)
}

/// Reword an open goal. `None` when no goal has that id.
///
/// # Errors
/// [`Error::Arguments`](crate::error::Error::Arguments) for empty or oversized text; database
/// errors otherwise.
pub fn edit(agents: &Agents, goal_id: &str, text: &str) -> Result<Option<Goal>> {
    agents.sessions().edit_goal(goal_id, text)
}

/// Close a goal as met, with whatever evidence was offered.
///
/// # Errors
/// Returns database errors only. A goal already closed, or an id nobody minted, is reported through
/// [`GoalClosed`] rather than refused — this path never blocks a caller.
pub fn met(agents: &Agents, goal_id: &str, evidence: &str) -> Result<GoalClosed> {
    close(agents, goal_id, GoalState::Met, evidence)
}

/// Close a goal as given up on, with the reason it could not be met.
///
/// # Errors
/// Returns database errors only — see [`met`].
pub fn give_up(agents: &Agents, goal_id: &str, why: &str) -> Result<GoalClosed> {
    close(agents, goal_id, GoalState::GivenUp, why)
}

/// Every goal of one conversation, oldest first — open and closed alike.
#[must_use]
pub fn of_conversation(agents: &Agents, agent: &str, conv: &str) -> Vec<Goal> {
    agents.sessions().goals(agent, conv)
}

/// One goal by id, wherever it lives.
///
/// # Errors
/// Returns database errors.
pub fn by_id(agents: &Agents, goal_id: &str) -> Result<Option<Goal>> {
    agents.sessions().goal(goal_id)
}

/// Every open goal in the store, oldest first — what is still being worked toward, anywhere.
#[must_use]
pub fn all_open(agents: &Agents) -> Vec<Goal> {
    agents.sessions().all_open_goals()
}

/// The shared half of [`met`] and [`give_up`]: close it, and announce it only if this call is what
/// closed it.
fn close(agents: &Agents, goal_id: &str, state: GoalState, note: &str) -> Result<GoalClosed> {
    let closed = agents.sessions().close_goal(goal_id, state, note)?;
    if let GoalClosed::Now(goal) = &closed {
        agents.emit(
            match state {
                GoalState::Met => GOAL_MET,
                _ => GOAL_GIVEN_UP,
            },
            &AgentGoalClosed {
                agent: goal.agent.clone(),
                conv: goal.conv.clone(),
                goal: goal.id.clone(),
                text: goal.text.clone(),
                note: goal.note.clone(),
                nudges: goal.nudges,
            },
        );
    }
    Ok(closed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::{AskRequest, Goal, GoalState, Question, SetBy};
    use crate::{AgentManifest, Launch, SimBlock};
    use adi_config::Config;

    fn scratch(tag: &str) -> Agents {
        let root = std::env::temp_dir().join(format!(
            "adi-goals-tick-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id(),
        ));
        let _ = std::fs::remove_dir_all(&root);
        Agents::with_config(Config::with_root(root))
    }

    /// A conversation that has ended its turn and is waiting on nothing.
    ///
    /// Simulated rather than spawned: a simulated run is answerable and its liveness is real (the
    /// seat is occupied until the turn ends), so it exercises the actual idle predicate without an
    /// engine — and the nudge it receives is delivered through the ordinary path.
    fn quiet_conversation(agents: &Agents, name: &str) -> String {
        agents
            .save(
                name,
                AgentManifest::<serde_json::Value> {
                    backend: "harness:adi".into(),
                    arguments: serde_json::json!({}),
                    ..AgentManifest::default()
                },
            )
            .expect("save");
        let Launch::Process { run_id, .. } = agents.simulate(name, "get started").expect("simulate")
        else {
            panic!("a simulated run is not a pane");
        };
        // A turn with no calls in it ends the turn and empties the seat — which is what "the chat
        // ended" means to the predicate below.
        agents
            .simulate_turn(name, &run_id, &[SimBlock::Text("on it".into())])
            .expect("end the turn");
        run_id
    }

    /// What the conversation has been told, newest last.
    fn said_to(agents: &Agents, name: &str, conv: &str) -> Vec<String> {
        agents
            .sessions()
            .turns(name, conv)
            .iter()
            .map(|turn| turn.text.clone())
            .collect()
    }

    /// The whole point, end to end: a chat that stopped with a goal open is asked about it, in the
    /// conversation, through the ordinary delivery path.
    #[test]
    fn a_quiet_conversation_with_an_open_goal_is_asked_about_it() {
        let agents = scratch("asked");
        let conv = quiet_conversation(&agents, "worker");
        let goal = create(&agents, "worker", &conv, "the suite is green", SetBy::Human)
            .expect("create");

        let nudged = tick(&agents);

        assert_eq!(nudged.len(), 1, "the one quiet conversation was asked");
        assert_eq!(nudged[0].goals, [goal.id.clone()]);
        assert!(nudged[0].error.is_none(), "{:?}", nudged[0].error);
        let said = said_to(&agents, "worker", &conv);
        let last = said.last().expect("something was said");
        assert!(last.contains("the suite is green"), "{last}");
        assert!(last.contains(&goal.id), "{last}");

        let after = by_id(&agents, &goal.id).expect("read").expect("there");
        assert_eq!(after.nudges, 1);
        assert!(after.last_nudge_at > 0);

        let _ = std::fs::remove_dir_all(agents.config().root());
    }

    /// The floor. A nudge ends up as a turn, and a turn that fails to start leaves the conversation
    /// idle again within milliseconds — without this the sweep would re-ask at its own tick rate.
    #[test]
    fn a_conversation_is_not_asked_twice_inside_the_floor() {
        let agents = scratch("floor");
        let conv = quiet_conversation(&agents, "worker");
        create(&agents, "worker", &conv, "keep going", SetBy::Human).expect("create");

        assert_eq!(tick(&agents).len(), 1);
        assert!(
            tick(&agents).is_empty(),
            "the second sweep in the same instant asks nothing"
        );

        let _ = std::fs::remove_dir_all(agents.config().root());
    }

    /// Each clause of the predicate is a different way of not being finished. A run mid-answer, a
    /// message already waiting, a question put to a person, and a wake the run arranged itself —
    /// none of them is a conversation that has fallen quiet.
    #[test]
    fn nothing_is_nudged_while_the_conversation_is_still_busy() {
        let agents = scratch("busy");
        let store = agents.sessions();

        // Mid-turn: the seat is still occupied, because nothing ended it.
        agents
            .save(
                "midturn",
                AgentManifest::<serde_json::Value> {
                    backend: "harness:adi".into(),
                    arguments: serde_json::json!({}),
                    ..AgentManifest::default()
                },
            )
            .expect("save");
        let Launch::Process { run_id: busy, .. } =
            agents.simulate("midturn", "working").expect("simulate")
        else {
            panic!("expected a headless run");
        };
        create(&agents, "midturn", &busy, "finish", SetBy::Agent).expect("create");

        let queued = quiet_conversation(&agents, "queued");
        create(&agents, "queued", &queued, "finish", SetBy::Human).expect("create");
        store.enqueue("queued", &queued, "one more thing").expect("enqueue");

        let asking = quiet_conversation(&agents, "asking");
        create(&agents, "asking", &asking, "finish", SetBy::Human).expect("create");
        store
            .ask(
                "asking",
                &asking,
                &AskRequest {
                    questions: vec![Question {
                        header: String::new(),
                        question: "which region?".into(),
                        options: Vec::new(),
                        multi_select: false,
                    }],
                    ..AskRequest::default()
                },
            )
            .expect("ask");

        let waiting = quiet_conversation(&agents, "waiting");
        create(&agents, "waiting", &waiting, "finish", SetBy::Agent).expect("create");
        crate::awaits::register(
            &Awaits::with_config(agents.config().clone()),
            "waiting",
            &waiting,
            &crate::awaits::Request {
                note: "wake me when the build lands".into(),
                events: vec!["adi.tasks.**".into()],
                ..crate::awaits::Request::default()
            },
        )
        .expect("register");

        assert!(
            tick(&agents).is_empty(),
            "none of these four has fallen quiet"
        );

        let _ = std::fs::remove_dir_all(agents.config().root());
    }

    /// Closing is what stops the asking — the only thing that does.
    #[test]
    fn a_closed_goal_stops_the_nudges() {
        let agents = scratch("closed");
        let conv = quiet_conversation(&agents, "worker");
        let goal = create(&agents, "worker", &conv, "ship it", SetBy::Agent).expect("create");

        met(&agents, &goal.id, "deployed at 14:02").expect("met");

        assert!(tick(&agents).is_empty());
        assert!(
            all_open(&agents).is_empty(),
            "and it is gone from the sweep's list"
        );

        let _ = std::fs::remove_dir_all(agents.config().root());
    }

    /// A run that gave up is a run that has said so, and it stops being asked on exactly the same
    /// terms as one that finished. That symmetry is the whole reason the give-up verb exists.
    #[test]
    fn giving_up_stops_the_nudges_exactly_as_meeting_it_does() {
        let agents = scratch("gave-up");
        let conv = quiet_conversation(&agents, "worker");
        let goal = create(&agents, "worker", &conv, "reach staging", SetBy::Agent).expect("create");

        give_up(&agents, &goal.id, "no route to the staging host from here").expect("give up");

        assert!(tick(&agents).is_empty());
        let closed = by_id(&agents, &goal.id).expect("read").expect("there");
        assert_eq!(closed.state, GoalState::GivenUp);
        assert_eq!(closed.note, "no route to the staging host from here");

        let _ = std::fs::remove_dir_all(agents.config().root());
    }

    fn goal(id: &str, text: &str) -> Goal {
        Goal {
            id: id.to_string(),
            agent: "watcher".to_string(),
            conv: "conv-1".to_string(),
            text: text.to_string(),
            state: GoalState::Open,
            set_by: SetBy::Human,
            created_at: 0,
            last_nudge_at: 0,
            nudges: 0,
            closed_at: None,
            note: String::new(),
        }
    }

    /// The message is all the run gets: the turn that set the goal may be hundreds of messages back
    /// in a transcript the model replays from the top, or may have been a person typing into a box
    /// it never saw. So the goal travels in full, with the id already in the commands.
    #[test]
    fn a_nudge_carries_the_goal_and_both_ways_out_of_it() {
        let text = nudge_message(&[goal("g-1", "the flaky tests are fixed")]);

        assert!(text.contains("the flaky tests are fixed"), "{text}");
        assert!(text.contains("adi-mono goals met g-1"), "{text}");
        assert!(
            text.contains("adi-mono goals knowingly-give-up g-1"),
            "the way out is offered as plainly as the way through: {text}"
        );
        assert!(
            text.contains("1 open goal,"),
            "one goal reads as one goal: {text}"
        );
    }

    /// Every open goal of a conversation travels together, because a run asked about them one at a
    /// time would answer each without the others in front of it.
    #[test]
    fn several_goals_travel_in_one_message() {
        let text = nudge_message(&[goal("g-1", "tests green"), goal("g-2", "docs updated")]);

        assert!(text.contains("2 open goals"), "{text}");
        assert!(text.contains("g-1") && text.contains("g-2"), "{text}");
        assert!(text.contains("tests green") && text.contains("docs updated"), "{text}");
        assert!(
            text.contains("goals met <id>"),
            "with several there is no one id to substitute, and the list is right above: {text}"
        );
    }
}
