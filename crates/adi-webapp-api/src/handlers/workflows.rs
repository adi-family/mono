//! `/api/workflows/*` — ADI-MONO-141 §F (WF-6): the workflows list, a workflow's own page
//! (starters, shapes, enable toggle, run now), and a run's timeline (saved steps, derived
//! status, rewind). Every status is derived fresh server-side every call (`adi_core::
//! workflow_run_status`) — nothing here is cached, the same "store facts, COMPUTE judgements"
//! rule the engine itself follows.

use adi_agents::Agents;
use adi_tasks::Tasks;
use adi_workflows::{Engine, Error as WorkflowsError, TriggerDef, WorkflowListing};

use crate::types::{
    RewindRun, RunDetailDto, RunDto, RunWorkflow, RunsQuery, RunsState, StepDto, WorkflowDto,
    WorkflowRef, WorkflowRunRef, WorkflowStarted, WorkflowsState,
};

use super::response::{FromBody, Response, error, mutate, ok_json};

/// `GET /api/workflows` — every discovered workflow file.
#[must_use]
pub fn workflows(engine: &Engine) -> Response {
    ok_json(&WorkflowsState {
        workflows: engine.list_workflows().iter().map(workflow_dto).collect(),
    })
}

/// `POST /api/workflows/enable` — let a workflow's `schedule`/`event` triggers fire it.
#[must_use]
pub fn enable_workflow(engine: &Engine, body: &[u8]) -> Response {
    mutate(
        body,
        |req: WorkflowRef| {
            engine.describe_workflow(&req.id)?;
            engine.set_enabled(&req.id, true)
        },
        || workflows(engine),
    )
}

/// `POST /api/workflows/disable` — stop a workflow's `schedule`/`event` triggers from firing it.
#[must_use]
pub fn disable_workflow(engine: &Engine, body: &[u8]) -> Response {
    mutate(
        body,
        |req: WorkflowRef| {
            engine.describe_workflow(&req.id)?;
            engine.set_enabled(&req.id, false)
        },
        || workflows(engine),
    )
}

/// `POST /api/workflows/run` — start a workflow by hand (`wf@manual:<requestId>`). Works even on
/// a disabled workflow — the one explicit action that is never gated by it.
#[must_use]
pub fn run_workflow(engine: &Engine, body: &[u8]) -> Response {
    let req = require!(body, RunWorkflow);
    let input = req.input.unwrap_or_else(|| serde_json::json!({}));
    let request_id = req
        .request_id
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
    match engine.start_manual(&req.id, &input, &request_id) {
        Ok(started) => ok_json(&WorkflowStarted {
            run_key: started.run_key,
            workflow: started.workflow,
        }),
        Err(e) => Response::from(&e),
    }
}

/// `POST /api/workflows/runs` — runs, with each one's status derived fresh. An empty body lists
/// across every workflow (newest first, capped at 50); naming a `workflow` lists just its own
/// runs (oldest first, the audit-trail order `adi_workflows::WorkflowStore::runs_for` gives).
#[must_use]
pub fn list_runs(engine: &Engine, tasks: &Tasks, agents: &Agents, body: &[u8]) -> Response {
    let query: RunsQuery = super::response::parse_body(body).unwrap_or_default();
    let runs = match &query.workflow {
        Some(id) => engine.runs().runs_for(id),
        None => engine.runs().list_runs(query.limit.unwrap_or(50)),
    };
    match runs {
        Ok(runs) => ok_json(&RunsState {
            runs: runs
                .iter()
                .map(|run| {
                    let status = adi_core::workflow_run_status(engine, tasks, agents, run);
                    RunDto {
                        run_key: run.run_key.clone(),
                        workflow: run.workflow.clone(),
                        status: status.label().to_string(),
                        created_at: run.created_at,
                        finished_at: run.finished_at,
                    }
                })
                .collect(),
        }),
        Err(e) => Response::from(&e),
    }
}

/// `POST /api/workflows/run/detail` — one run's full timeline: its input/cause, derived status,
/// and every journaled step in resolution order (what "rewind to step N" counts against).
#[must_use]
pub fn run_detail(engine: &Engine, tasks: &Tasks, agents: &Agents, body: &[u8]) -> Response {
    let req = require!(body, WorkflowRunRef);
    let run = match engine.runs().get_run(&req.run_key) {
        Ok(Some(run)) => run,
        Ok(None) => return error(404, &format!("no such run: {}", req.run_key)),
        Err(e) => return Response::from(&e),
    };
    let steps = match engine.runs().list_steps(&req.run_key) {
        Ok(steps) => steps,
        Err(e) => return Response::from(&e),
    };
    let status = adi_core::workflow_run_status(engine, tasks, agents, &run);
    ok_json(&RunDetailDto {
        run_key: run.run_key,
        workflow: run.workflow,
        status: status.label().to_string(),
        input: run.input,
        cause: run.cause,
        created_at: run.created_at,
        finished_at: run.finished_at,
        steps: steps.iter().map(step_dto).collect(),
    })
}

/// `POST /api/workflows/run/rewind` — discard a run's journal after step `to`, so the next
/// attempt genuinely redoes the discarded work rather than replaying a stale answer.
#[must_use]
pub fn rewind_run(engine: &Engine, tasks: &Tasks, agents: &Agents, body: &[u8]) -> Response {
    let req = require!(body, RewindRun);
    if let Err(e) = engine.rewind(&req.run_key, req.to) {
        return Response::from(&e);
    }
    let run = match engine.runs().get_run(&req.run_key) {
        Ok(Some(run)) => run,
        Ok(None) => return error(404, &format!("no such run: {}", req.run_key)),
        Err(e) => return Response::from(&e),
    };
    let steps = match engine.runs().list_steps(&req.run_key) {
        Ok(steps) => steps,
        Err(e) => return Response::from(&e),
    };
    let status = adi_core::workflow_run_status(engine, tasks, agents, &run);
    ok_json(&RunDetailDto {
        run_key: run.run_key,
        workflow: run.workflow,
        status: status.label().to_string(),
        input: run.input,
        cause: run.cause,
        created_at: run.created_at,
        finished_at: run.finished_at,
        steps: steps.iter().map(step_dto).collect(),
    })
}

fn workflow_dto(listing: &WorkflowListing) -> WorkflowDto {
    WorkflowDto {
        id: listing.id.clone(),
        project: listing.project.clone(),
        enabled: listing.enabled,
        triggers: listing
            .shape
            .as_ref()
            .map(|shape| shape.on.iter().map(trigger_line).collect())
            .unwrap_or_default(),
        input_schema: listing.shape.as_ref().and_then(|s| s.input_schema.clone()),
        output_schema: listing.shape.as_ref().and_then(|s| s.output_schema.clone()),
        describe_error: listing.describe_error.clone(),
    }
}

/// A trigger definition as one human-readable line — the same rendering the CLI's `show` prints.
fn trigger_line(trigger: &TriggerDef) -> String {
    match trigger {
        TriggerDef::Schedule { cron, tz, catch_up } => {
            let mut line = format!("schedule: {cron} {tz}");
            if catch_up.as_deref() == Some("skip") {
                line.push_str(" (catchUp: skip)");
            }
            line
        }
        TriggerDef::Event { pattern, where_clause } => match where_clause {
            Some(clause) => format!(
                "event: {pattern} where {}",
                serde_json::Value::Object(clause.clone())
            ),
            None => format!("event: {pattern}"),
        },
        TriggerDef::Manual => "manual".to_string(),
    }
}

fn step_dto(step: &adi_workflows::StepRecord) -> StepDto {
    use adi_workflows::StepOutcome;
    let (outcome, value) = match &step.outcome {
        StepOutcome::Ok(value) => ("ok", Some(value.clone())),
        StepOutcome::GaveUp(reason) => ("gave_up", Some(serde_json::Value::String(reason.clone()))),
        StepOutcome::TimedOut => ("timed_out", None),
    };
    StepDto {
        step_key: step.step_key.clone(),
        kind: step.kind.as_str().to_string(),
        outcome: outcome.to_string(),
        value,
        task_id: step.task_id.clone(),
        ref_key: step.ref_key.clone(),
    }
}

// Map a workflow-engine error to an HTTP status: an unknown workflow or run is a 404, a bad
// request shape is a 400, everything else (store I/O, a failed/given-up attempt) is a 500.
impl From<&WorkflowsError> for Response {
    fn from(e: &WorkflowsError) -> Self {
        let status = match e {
            WorkflowsError::UnknownWorkflow(_) => 404,
            WorkflowsError::InvalidCron(..) | WorkflowsError::JournalEntryTooLarge(_) => 400,
            WorkflowsError::DuplicateRunKey(_) | WorkflowsError::DuplicateStepKey(_) => 409,
            WorkflowsError::Describe(_)
            | WorkflowsError::AttemptFailed(_)
            | WorkflowsError::AttemptGaveUp(_)
            | WorkflowsError::Store(_) => 500,
        };
        error(status, &e.to_string())
    }
}

impl FromBody for RunWorkflow {
    const EXPECTED: &'static str = "expected JSON body { \"id\": \"…\" } with a non-empty id";

    fn is_complete(&self) -> bool {
        !self.id.trim().is_empty()
    }
}

impl FromBody for WorkflowRef {
    const EXPECTED: &'static str = "expected JSON body { \"id\": \"…\" } with a non-empty id";

    fn is_complete(&self) -> bool {
        !self.id.trim().is_empty()
    }
}

impl FromBody for WorkflowRunRef {
    const EXPECTED: &'static str = "expected JSON body { \"run_key\": \"…\" }";

    fn is_complete(&self) -> bool {
        !self.run_key.trim().is_empty()
    }
}

impl FromBody for RewindRun {
    const EXPECTED: &'static str =
        "expected JSON body { \"run_key\": \"…\", \"to\": <number> }";

    fn is_complete(&self) -> bool {
        !self.run_key.trim().is_empty()
    }
}
