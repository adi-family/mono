//! The `goals` command group: what a conversation is for, and the two ways it ends.
//!
//! Five verbs and no more, because this surface is typed by a language model as often as by a
//! person — every extra verb is another thing for a run to get wrong at the moment it is trying to
//! say "I am done".
//!
//! # It addresses itself
//!
//! Every subcommand that names a conversation falls back to `ADI_AGENT` and `ADI_RUN_ID`, which are
//! in the environment of every turn (see `adi_agents::goals`). So a run sets, reads and closes its
//! own goals with no arguments beyond the text — and the same commands work from a terminal by
//! naming `--agent` / `--session` explicitly.
//!
//! # Nothing here refuses
//!
//! Closing a goal that is already closed, or one closed the other way, reports what actually
//! happened and exits 0. That is not laxity: the caller is usually a model reading this as a tool
//! result, and a failed tool result is something it will retry or argue with rather than accept. The
//! single exception is an id that matches no goal at all, which is a typo and exits 1 — there is
//! nothing to accept, and silence would let a run believe it had closed something.

use adi_core::{Adi, Goal, GoalClosed, GoalState, SetBy, goals};
use clap::Subcommand;

use crate::format::print_json;

/// The environment a turn is given, which is what makes every `--agent` / `--session` optional.
const AGENT_ENV: &str = "ADI_AGENT";
const CONV_ENV: &str = "ADI_RUN_ID";

#[derive(Debug, Subcommand)]
pub(crate) enum GoalsCommand {
    /// Set a goal on a conversation — what would make it done.
    ///
    /// From inside a turn this needs only the text: the conversation is read from the environment,
    /// and the goal is put back to the run every time it falls quiet.
    Create {
        /// What done means, in a sentence.
        text: String,
        /// The agent whose conversation this is. Defaults to `$ADI_AGENT`.
        #[arg(long)]
        agent: Option<String>,
        /// The conversation id. Defaults to `$ADI_RUN_ID`.
        #[arg(long)]
        session: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Reword an open goal, leaving everything else about it alone.
    Edit {
        /// The goal id, from `goals show` or from the goal check you were sent.
        id: String,
        #[arg(long)]
        text: String,
        #[arg(long)]
        json: bool,
    },
    /// Show a goal, this conversation's goals, or every open goal on the machine.
    Show {
        /// One goal by id. Omit for this conversation's goals.
        id: Option<String>,
        /// Every open goal anywhere, whichever conversation it belongs to.
        #[arg(long)]
        all: bool,
        /// The agent whose conversation to read. Defaults to `$ADI_AGENT`.
        #[arg(long)]
        agent: Option<String>,
        /// The conversation id. Defaults to `$ADI_RUN_ID`.
        #[arg(long)]
        session: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Close a goal as met.
    Met {
        id: String,
        /// What shows it is met — the test that passes, the file that exists, the endpoint that
        /// answers. Kept with the goal, and worth writing: it is what somebody reads later to
        /// decide whether to believe this.
        #[arg(long)]
        evidence: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Close a goal you cannot meet, saying what stopped you.
    ///
    /// Spelled out in full on purpose. This is the one way work stops without being finished, and
    /// nothing else in the platform will ever write it for you — a goal nobody closes is asked
    /// about for ever.
    KnowinglyGiveUp {
        id: String,
        /// What stopped you. Required in spirit and empty-able in practice: an unexplained give-up
        /// is still better than a run that goes quiet.
        #[arg(long)]
        why: Option<String>,
        #[arg(long)]
        json: bool,
    },
}

pub(crate) fn run_goals(adi: Adi, command: GoalsCommand) -> Result<(), String> {
    let store = adi.agents();
    match command {
        GoalsCommand::Create {
            text,
            agent,
            session,
            json,
        } => {
            let (agent, conv, from_env) = address(agent, session)?;
            // A goal set by a run addressing itself is a run deciding what it is doing, and that
            // reads differently afterward from one it was handed. The environment is what tells
            // them apart: nothing else here can.
            let set_by = if from_env { SetBy::Agent } else { SetBy::Human };
            let goal =
                goals::create(&store, &agent, &conv, &text, set_by).map_err(|e| e.to_string())?;
            if json {
                print_json(&goal);
            } else {
                println!("Goal set on {agent} / {conv}.");
                print_goal(&goal);
                println!(
                    "\nClose it with:\n  adi-mono goals met {} --evidence \"…\"\n  \
                     adi-mono goals knowingly-give-up {} --why \"…\"",
                    goal.id, goal.id
                );
            }
        }
        GoalsCommand::Edit { id, text, json } => {
            let edited = goals::edit(&store, &id, &text).map_err(|e| e.to_string())?;
            let Some(goal) = edited else {
                return Err(format!("no goal with id {id}"));
            };
            if json {
                print_json(&goal);
            } else if goal.text == text.trim() {
                println!("Goal {id} reworded.");
                print_goal(&goal);
            } else {
                // The store leaves a closed goal exactly as it was decided, so the edit is a no-op
                // rather than an error — say which, or it reads as having worked.
                println!(
                    "Goal {id} is already {}, so it was left as it was decided.",
                    goal.state.as_str()
                );
                print_goal(&goal);
            }
        }
        GoalsCommand::Show {
            id,
            all,
            agent,
            session,
            json,
        } => {
            let found = match (id, all) {
                (Some(id), _) => goals::by_id(&store, &id)
                    .map_err(|e| e.to_string())?
                    .map(|g| vec![g])
                    .ok_or_else(|| format!("no goal with id {id}"))?,
                (None, true) => goals::all_open(&store),
                (None, false) => {
                    let (agent, conv, _) = address(agent, session)?;
                    goals::of_conversation(&store, &agent, &conv)
                }
            };
            if json {
                print_json(&found);
            } else if found.is_empty() {
                println!("No goals.");
            } else {
                for goal in &found {
                    print_goal(goal);
                }
            }
        }
        GoalsCommand::Met { id, evidence, json } => {
            let closed = goals::met(&store, &id, evidence.as_deref().unwrap_or_default())
                .map_err(|e| e.to_string())?;
            report_closed(&id, &closed, "met", json)?;
        }
        GoalsCommand::KnowinglyGiveUp { id, why, json } => {
            let closed = goals::give_up(&store, &id, why.as_deref().unwrap_or_default())
                .map_err(|e| e.to_string())?;
            report_closed(&id, &closed, "given up on", json)?;
        }
    }
    Ok(())
}

/// The conversation a command is about: what was named, or what the environment says.
///
/// The third element is whether the environment answered — which is how [`SetBy`] is decided, and
/// the only thing on this surface that behaves differently for a run than for a person.
fn address(
    agent: Option<String>,
    session: Option<String>,
) -> Result<(String, String, bool), String> {
    let from_env = agent.is_none() && session.is_none();
    let agent = agent
        .or_else(|| std::env::var(AGENT_ENV).ok())
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| {
            format!(
                "no agent: pass --agent, or run this from inside a turn where {AGENT_ENV} is set"
            )
        })?;
    let conv = session
        .or_else(|| std::env::var(CONV_ENV).ok())
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| {
            format!(
                "no conversation: pass --session, or run this from inside a turn where {CONV_ENV} \
                 is set"
            )
        })?;
    Ok((agent, conv, from_env))
}

/// Say what a close actually did.
///
/// The three answers are genuinely different and a caller acts on each differently, which is why
/// they are not flattened into one line: this call closed it, somebody had already closed it, or
/// there is no such goal.
fn report_closed(id: &str, closed: &GoalClosed, verb: &str, json: bool) -> Result<(), String> {
    match closed {
        GoalClosed::Now(goal) => {
            if json {
                print_json(goal);
            } else {
                println!("Goal {id} {verb}.");
                print_goal(goal);
            }
            Ok(())
        }
        GoalClosed::Already(goal) => {
            if json {
                print_json(goal);
            } else {
                println!(
                    "Goal {id} was already {} — the first ending stands, and nothing changed.",
                    goal.state.as_str()
                );
                print_goal(goal);
            }
            Ok(())
        }
        // The one failure on this surface: there is nothing to have closed, so saying "done" would
        // let a run believe it had finished.
        GoalClosed::Unknown => Err(format!("no goal with id {id}")),
    }
}

fn print_goal(goal: &Goal) {
    let state = match goal.state {
        GoalState::Open => "open".to_string(),
        GoalState::Met => "met".to_string(),
        GoalState::GivenUp => "given up".to_string(),
    };
    println!("\n{}  [{state}]", goal.id);
    println!("  {}", goal.text);
    println!(
        "  {} / {} · set by {} · nudged {}×",
        goal.agent,
        goal.conv,
        goal.set_by.as_str(),
        goal.nudges
    );
    if !goal.note.trim().is_empty() {
        let label = if goal.state == GoalState::Met {
            "evidence"
        } else {
            "why"
        };
        println!("  {label}: {}", goal.note.trim());
    }
}
