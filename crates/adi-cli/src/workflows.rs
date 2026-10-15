//! The `workflows` command group: `adi-mono workflows list/show/run/runs/rewind/enable/disable`
//! — spec section F of ADI-MONO-141 (WF-6). Every verb reads or writes facts the engine already
//! stores; nothing here computes anything the panel (also WF-6) doesn't compute the same way.

use adi_core::{Adi, TriggerDef, WorkflowListing};
use clap::Subcommand;

use crate::format::{clean, print_json};

/// What an agent is told about this command group, folded into its own `llm help` output.
///
/// Order matches what a caller needs in sequence: write the file, know the rules that keep a
/// replay honest, then how a *dispatched* task (the common case an agent actually touches) is
/// read and finished. The proof-post shape at the end is the one worked example spec section G
/// names — not code that exists yet (that's WF-7's job), but the pattern every multi-step,
/// human-reviewed workflow here follows.
const LLM_HELP: &str = r#"adi-workflows — a TypeScript file per workflow, run with checkpoints so it
resumes exactly where it left off after a crash or a restart, never from the top.

A WORKFLOW FILE (workflows/*.ts, or projects/<id>/workflows/*.ts)
  import { schedule, manual, workflow } from "@adi/workflows";
  import { z } from "zod";
  export const on = schedule("0 5 * * MON", "America/New_York"); // or event(...), manual(), [..]
  export const input = z.object({ topic: z.string() });  // optional
  export const output = z.object({ ok: z.boolean() });   // optional
  export default workflow(async (ctx, input) => {
    const r = await ctx.step({ key: "fetch", fn: () => fetchSomething() });
    return { ok: true };
  });

ON: schedule(cron, tz, {catchUp}?) | event(pattern, where?) | manual() — one, or an array.
  - cron is STANDARD 5-FIELD unix cron ("min hour dom month dow"), e.g. "0 5 * * MON" — this
    engine normalizes it to its own dialect; write 5 fields, never 6.
  - tz is an IANA zone name ("America/New_York"), never a bare UTC offset.
  - catchUp: "fire" (default — the latest missed slot fires once after downtime) or "skip".
  - event(pattern, where?): pattern matches dotted names (`*` one segment, `**` the tail).

EVERY ctx.* CALL IS A CHECKPOINT. `key` is REQUIRED and UNIQUE within one run — a saved
checkpoint replays instantly from its journal; reusing a key fails the whole run.
  ctx.step({key, fn, retry?})        run fn() once and save its return. It CAN run more than
                                      once if the process dies after fn()'s own effect but
                                      before the save lands — make fn() idempotent (check-
                                      before-act, or a dedupe key) whenever it has a real effect.
  ctx.task({key,title,agent|person,instructions?,input?,output?,timeout?,project?,cwd?})
                                      a real task; resolves your `output` shape or
                                      {timed_out:true} (which also closes the task as a fact, so
                                      it never lingers in "Needs your input"); throws if the task
                                      is given up on.
  ctx.ask({key, person, text})       a human question, as a task with the same shapes.
  ctx.run({key, workflow, input?})   a child run, keyed to this one; waits for its result.
  ctx.waitFor({key, event, where?, timeout?})   waits for a matching platform event.
  ctx.sleep({key, until})            `until` MUST compute the SAME value on every attempt — from
                                      ctx.slot or the input — NEVER `Date.now() + ms`, which
                                      drifts forward each retry and can starve.
  ctx.dir, ctx.runId, ctx.slot        the run's own directory, its id, its due slot (if any).

NEVER CALL Date.now() / Math.random() / crypto.randomUUID() OUTSIDE ctx.step. A replay has to
compute the identical thing up to wherever it resumes; only a ctx.step's own fn() — and only
the first time it runs — may touch the clock, randomness, or the outside world.

A CRASHED ATTEMPT IS RETRIED, NOT FAILED. Only your own code failing the workflow (a thrown
error, an uncaught TaskGaveUp, an output shape rejecting the result) fails the run. The `bun`
process dying with no message at all (OOM, a segfault, kill -9, this engine's own watchdog) is
transient: retried with backoff (30s, then 2min), and only the 3rd consecutive crash in a row
actually fails the run. Real progress on a later attempt resets that count to zero — reason
enough, again, for every ctx.step with a real effect to be idempotent.

RETRIGGER GUARD: an event-started run carries its own causer. Starting a workflow from an event
whose causation chain already includes a run of that SAME workflow (up to 8 hops through other
workflows) is refused — so a workflow reacting to its own side effects can't loop forever.

EVENTS keep ~30 days, and are never dropped past the slowest subscriber's own cursor — a
ctx.waitFor a week behind still sees everything that happened since it started waiting.

REWIND (adi-mono workflows rewind <run> --to <n>) discards the journal after step n and redoes
it for real — a fresh task, a fresh child run. One limit: a discarded ctx.run step's own CHILD
run keeps its own journal untouched; rewind never cascades into it.

AS AN AGENT, A DISPATCHED ctx.task READS LIKE ANY OTHER TASK:
  adi-mono tasks show <id>                        instructions, input, the exact output shape
  adi-mono tasks complete <id> --result '<json>'   validated against that shape; refuses with
                                                   the exact path if it doesn't fit
  adi-mono tasks give-up <id> --reason '...'       when you genuinely can't do it

OPT AN AGENT INTO DISPATCH, so a ready task assigned to it launches automatically:
  adi-mono agents save <name> --dispatch

A WORKED SHAPE (proof-post in adi-launch, spec section G): research (ctx.step) -> draft, up to
3 ctx.task revise rounds -> ctx.ask Igor to approve/revise/drop (48h timeout) -> a child
ctx.run publish-post whose own ctx.step check-before-acts so a retry never double-posts -> a
ctx.step that verifies the post is actually live. Nothing here posts anywhere without the
human approval step actually resolving "approve" first.

CLI: adi-mono workflows list/show/run/runs/rewind/enable/disable. `show <id>` prints the file's
triggers and input/output shapes. `run <id> --input '<json>'` starts one by hand — always
works, even on a disabled workflow, which is how you try a draft before trusting it with real
triggers. `runs` derives each run's status fresh every time (running / waiting on task, event,
sleep, or child / done / failed / stuck) — nothing is cached. A workflow file nobody has told
this store about yet is DISABLED by default — `adi-mono workflows enable <id>` lets its
schedule/event triggers actually fire.
"#;

#[derive(Debug, Subcommand)]
pub(crate) enum WorkflowsCommand {
    /// List every discovered workflow file: its triggers and whether it is enabled.
    List {
        #[arg(long)]
        json: bool,
    },
    /// Show one workflow's file, its starters, its input/output shapes, and whether it's enabled.
    Show {
        /// The workflow id (its file stem, or `<project>/<stem>` for a project-scoped one).
        id: String,
        #[arg(long)]
        json: bool,
    },
    /// Start a workflow by hand — `wf@manual:<requestId>`. Works even on a disabled workflow.
    Run {
        /// The workflow id.
        id: String,
        /// The workflow's own input, as JSON. Defaults to `{}`.
        #[arg(long)]
        input: Option<String>,
        /// Reuse this request id for an idempotent retry — the same id always answers with the
        /// same run rather than starting a second one. Omit to mint a fresh one.
        #[arg(long = "request-id")]
        request_id: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// List runs, with each one's status derived fresh: running, waiting on task/event/sleep/
    /// child, done, failed, or stuck.
    Runs {
        /// Only this workflow's runs (oldest first). Omit to list across every workflow (newest
        /// first).
        #[arg(long)]
        workflow: Option<String>,
        /// How many to show, newest first — only applies with no `--workflow`.
        #[arg(long, default_value_t = 50)]
        limit: usize,
        #[arg(long)]
        json: bool,
    },
    /// Discard a run's journal after step `--to`, so the next attempt genuinely redoes the
    /// discarded work (a fresh task, a fresh child run) rather than replaying a stale answer.
    Rewind {
        /// The run key (see `workflows runs`).
        run: String,
        /// How many of the run's own journaled steps to keep — `0` discards everything.
        #[arg(long = "to")]
        to: usize,
    },
    /// Let a workflow's `schedule`/`event` triggers fire it. A workflow this store has never been
    /// told about starts disabled — `run` still works on it regardless.
    Enable { id: String },
    /// Stop a workflow's `schedule`/`event` triggers from firing it. `run` still works on it.
    Disable { id: String },
    /// Help written for a language model rather than for a person. Bare `llm` prints it too —
    /// `Help` is the only variant there is, and a caller shouldn't have to spell it out.
    #[command(disable_help_subcommand = true)]
    Llm {
        #[command(subcommand)]
        command: Option<LlmCommand>,
    },
}

#[derive(Debug, Subcommand)]
pub(crate) enum LlmCommand {
    /// Everything an agent needs to write and work with a workflow correctly, and nothing else.
    Help,
}

/// Dispatch a `workflows` subcommand over the shared engine.
pub(crate) fn run_workflows(adi: Adi, command: WorkflowsCommand) -> Result<(), String> {
    if let WorkflowsCommand::Llm { .. } = command {
        print!("{LLM_HELP}");
        return Ok(());
    }

    let engine = adi.workflows();
    match command {
        WorkflowsCommand::List { json } => {
            let listings = engine.list_workflows();
            if json {
                print_json(&listings);
            } else if listings.is_empty() {
                println!("No workflow files found.");
            } else {
                for listing in &listings {
                    print_listing(listing, false);
                }
            }
        }
        WorkflowsCommand::Show { id, json } => {
            let listing = engine.describe_workflow(&id).map_err(|e| e.to_string())?;
            if json {
                print_json(&listing);
            } else {
                print_listing(&listing, true);
            }
        }
        WorkflowsCommand::Run {
            id,
            input,
            request_id,
            json,
        } => {
            let input: serde_json::Value = match clean(input) {
                Some(raw) => serde_json::from_str(&raw)
                    .map_err(|e| format!("--input is not valid JSON: {e}"))?,
                None => serde_json::json!({}),
            };
            let request_id = request_id.unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
            let started = engine
                .start_manual(&id, &input, &request_id)
                .map_err(|e| e.to_string())?;
            if json {
                print_json(&started);
            } else {
                println!("Started {} as {}.", started.workflow, started.run_key);
            }
        }
        WorkflowsCommand::Runs {
            workflow,
            limit,
            json,
        } => {
            let runs = match &workflow {
                Some(id) => engine.runs().runs_for(id).map_err(|e| e.to_string())?,
                None => engine.runs().list_runs(limit).map_err(|e| e.to_string())?,
            };
            print_runs(adi, &engine, &runs, json);
        }
        WorkflowsCommand::Rewind { run, to } => {
            engine.rewind(&run, to).map_err(|e| e.to_string())?;
            println!("Rewound {run} to step {to}.");
        }
        WorkflowsCommand::Enable { id } => {
            engine.describe_workflow(&id).map_err(|e| e.to_string())?;
            engine.set_enabled(&id, true).map_err(|e| e.to_string())?;
            println!("Enabled {id}.");
        }
        WorkflowsCommand::Disable { id } => {
            engine.describe_workflow(&id).map_err(|e| e.to_string())?;
            engine.set_enabled(&id, false).map_err(|e| e.to_string())?;
            println!("Disabled {id}.");
        }
        WorkflowsCommand::Llm { .. } => unreachable!("answered above, before the engine is opened"),
    }
    Ok(())
}

/// Print a list of runs, each with its status derived fresh (never cached) via
/// [`adi_core::workflow_run_status`] — the compact human form, or one JSON array.
fn print_runs(adi: Adi, engine: &adi_core::WorkflowEngine, runs: &[adi_core::WorkflowRun], json: bool) {
    let tasks = adi.tasks();
    let agents = adi.agents();
    if json {
        let rows: Vec<_> = runs
            .iter()
            .map(|run| {
                let status = adi_core::workflow_run_status(engine, &tasks, &agents, run);
                serde_json::json!({
                    "run_key": run.run_key,
                    "workflow": run.workflow,
                    "created_at": run.created_at,
                    "finished_at": run.finished_at,
                    "status": status.label(),
                })
            })
            .collect();
        print_json(&rows);
        return;
    }
    if runs.is_empty() {
        println!("No runs.");
        return;
    }
    for run in runs {
        let status = adi_core::workflow_run_status(engine, &tasks, &agents, run);
        println!("{} [{}] workflow={}", run.run_key, status.label(), run.workflow);
    }
}

/// Print one workflow's listing in the compact human CLI format. `detailed` (used by `show`,
/// never `list`) also prints the input/output JSON Schema when the file declares one.
fn print_listing(listing: &WorkflowListing, detailed: bool) {
    let state = if listing.enabled { "enabled" } else { "disabled" };
    print!("{} [{state}]", listing.id);
    if let Some(project) = &listing.project {
        print!(" (project {project})");
    }
    println!();
    if detailed {
        println!("  file: {}", listing.path.display());
    }
    match &listing.shape {
        Some(shape) => {
            if shape.on.is_empty() {
                println!("  triggers: none");
            }
            for trigger in &shape.on {
                println!("  {}", trigger_line(trigger));
            }
            if detailed {
                print_schema("input", shape.input_schema.as_ref());
                print_schema("output", shape.output_schema.as_ref());
            }
        }
        None => {
            if let Some(err) = &listing.describe_error {
                println!("  ✕ couldn't read this file's `on` export: {err}");
            }
        }
    }
}

fn trigger_line(trigger: &TriggerDef) -> String {
    match trigger {
        TriggerDef::Schedule { cron, tz, catch_up } => {
            let mut line = format!("schedule: {cron} {tz}");
            if catch_up.as_deref() == Some("skip") {
                line.push_str(" (catchUp: skip)");
            }
            line
        }
        TriggerDef::Event { pattern, where_clause } => {
            use std::fmt::Write as _;
            let mut line = format!("event: {pattern}");
            if let Some(clause) = where_clause {
                let _ = write!(line, " where {}", serde_json::Value::Object(clause.clone()));
            }
            line
        }
        TriggerDef::Manual => "manual".to_string(),
    }
}

fn print_schema(label: &str, schema: Option<&serde_json::Value>) {
    match schema {
        Some(schema) => {
            println!("  {label}:");
            for line in serde_json::to_string_pretty(schema).unwrap_or_default().lines() {
                println!("    {line}");
            }
        }
        None => println!("  {label}: none"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::{CommandFactory as _, Parser as _};

    #[derive(Debug, clap::Parser)]
    struct Harness {
        #[command(subcommand)]
        command: WorkflowsCommand,
    }

    #[test]
    fn the_command_tree_is_well_formed() {
        Harness::command().debug_assert();
    }

    #[test]
    fn llm_help_is_reachable_and_names_every_ctx_block() {
        let cli = Harness::try_parse_from(["workflows", "llm", "help"]).expect("parses");
        assert!(matches!(
            cli.command,
            WorkflowsCommand::Llm {
                command: Some(LlmCommand::Help)
            }
        ));
        for needle in [
            "ctx.step", "ctx.task", "ctx.ask", "ctx.run", "ctx.waitFor", "ctx.sleep",
            "5-FIELD", "Date.now", "8 hops", "30 days", "tasks complete", "--dispatch",
            "RETRIED, NOT FAILED", "3rd consecutive crash",
        ] {
            assert!(LLM_HELP.contains(needle), "missing {needle:?} in LLM_HELP");
        }
    }

    #[test]
    fn bare_llm_parses_and_answers_the_same_help() {
        let cli = Harness::try_parse_from(["workflows", "llm"]).expect("parses");
        assert!(matches!(cli.command, WorkflowsCommand::Llm { command: None }));
    }

    #[test]
    fn run_takes_input_and_an_optional_request_id() {
        let cli = Harness::try_parse_from([
            "workflows",
            "run",
            "weekly-report",
            "--input",
            "{\"topic\":\"x\"}",
            "--request-id",
            "r1",
        ])
        .expect("parses");
        assert!(matches!(
            cli.command,
            WorkflowsCommand::Run {
                ref id,
                input: Some(ref input),
                request_id: Some(ref request_id),
                json: false,
            } if id == "weekly-report" && input == "{\"topic\":\"x\"}" && request_id == "r1"
        ));
    }

    #[test]
    fn rewind_takes_the_run_and_a_keep_count() {
        let cli = Harness::try_parse_from(["workflows", "rewind", "wf@manual:1", "--to", "2"])
            .expect("parses");
        assert!(matches!(
            cli.command,
            WorkflowsCommand::Rewind { ref run, to: 2 } if run == "wf@manual:1"
        ));
    }

    #[test]
    fn runs_takes_an_optional_workflow_filter_and_a_limit() {
        let cli = Harness::try_parse_from(["workflows", "runs"]).expect("parses");
        assert!(matches!(
            cli.command,
            WorkflowsCommand::Runs {
                workflow: None,
                limit: 50,
                json: false,
            }
        ));
        let cli =
            Harness::try_parse_from(["workflows", "runs", "--workflow", "wf", "--limit", "5"])
                .expect("parses");
        assert!(matches!(
            cli.command,
            WorkflowsCommand::Runs {
                workflow: Some(ref w),
                limit: 5,
                ..
            } if w == "wf"
        ));
    }
}
