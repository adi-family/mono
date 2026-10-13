//! Deriving a workflow run's status — computed fresh every call, never itself stored (spec
//! section E's own principle: "store facts, COMPUTE judgements").
//!
//! This is the one join `adi-workflows` cannot make on its own: it deliberately carries no
//! dependency on `adi-agents` (see that crate's own module doc), so it can say a run is
//! *waiting on a task* but not whether that task is itself *stuck* — telling those two apart
//! needs `adi-agents`' own `task_is_stuck`. `adi-core` already depends on both (the same reason
//! [`crate::event_catalog`] is where the task/agent/channel/workflow event catalogs join), so
//! this is where the two halves become one answer.

use adi_agents::Agents;
use adi_tasks::{TaskStatus, Tasks};
use adi_workflows::{Engine, RunRow, WaitingOn};

/// A run's status, derived fresh from the runs/tasks/step-journal tables every call — spec
/// section F: "runs lists runs with DERIVED status: running / waiting on <task|event|sleep|
/// child> / done / failed / stuck." Deleting every cache this (and everything it calls) reads
/// costs nothing but a slower next call — never a wrong answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunStatus {
    /// Nothing pending and no unfinished child — freshly poured, or simply due for its next
    /// attempt.
    Running,
    /// Blocked on a `ctx.task`/`ctx.ask` step's underlying task, which is itself proceeding
    /// normally (not [`Stuck`](Self::Stuck)). `task_id` is `None` only in the vanishingly rare
    /// case the pending marker hasn't resolved back to a task id yet.
    WaitingOnTask { task_id: Option<String> },
    /// Blocked on a `ctx.waitFor` that hasn't seen a matching event yet.
    WaitingOnEvent,
    /// Blocked on a `ctx.sleep` that hasn't reached its wake time yet.
    WaitingOnSleep,
    /// Blocked on a `ctx.run` child that hasn't finished yet.
    WaitingOnChild,
    /// The run's root task completed.
    Done,
    /// The run's root task was given up on — an uncaught `TaskGaveUp`, or any other uncaught
    /// error from the workflow body.
    Failed,
    /// Blocked on a `ctx.task`/`ctx.ask` step's underlying task, which is itself stuck per the
    /// dispatch tick's own definition (`adi_agents::Agents::task_is_stuck`) — an unresolved stuck
    /// ask is already out for it, so nothing here raises a second one.
    Stuck { task_id: String },
}

impl RunStatus {
    /// The spec's own wording for this status, exactly as section F states it — what `adi-mono
    /// workflows runs` prints and what the panel's run list shows.
    #[must_use]
    pub fn label(&self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::WaitingOnTask { .. } => "waiting on task",
            Self::WaitingOnEvent => "waiting on event",
            Self::WaitingOnSleep => "waiting on sleep",
            Self::WaitingOnChild => "waiting on child",
            Self::Done => "done",
            Self::Failed => "failed",
            Self::Stuck { .. } => "stuck",
        }
    }
}

/// Derive `run`'s status — see the module doc for why this needs both `engine` and `agents`.
#[must_use]
pub fn run_status(engine: &Engine, tasks: &Tasks, agents: &Agents, run: &RunRow) -> RunStatus {
    if run.finished_at.is_some() {
        return match run.root_task.as_deref().and_then(|id| tasks.get(id).ok()) {
            Some(view) if view.task.status == TaskStatus::Done => RunStatus::Done,
            _ => RunStatus::Failed,
        };
    }
    match engine.waiting_on(&run.run_key) {
        Ok(Some(WaitingOn::Task { task_id: Some(id) })) if agents.task_is_stuck(&id) => {
            RunStatus::Stuck { task_id: id }
        }
        Ok(Some(WaitingOn::Task { task_id })) => RunStatus::WaitingOnTask { task_id },
        Ok(Some(WaitingOn::Event)) => RunStatus::WaitingOnEvent,
        Ok(Some(WaitingOn::Sleep)) => RunStatus::WaitingOnSleep,
        Ok(Some(WaitingOn::Child)) => RunStatus::WaitingOnChild,
        Ok(None) | Err(_) => RunStatus::Running,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn scratch(tag: &str) -> adi_config::Config {
        let root = std::env::temp_dir().join(format!(
            "adi-core-workflows-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id(),
        ));
        let _ = std::fs::remove_dir_all(&root);
        adi_config::Config::with_root(root)
    }

    fn write_workflow(config: &adi_config::Config, name: &str, body: &str) {
        let dir = config.module("workflows").dir().to_path_buf();
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(name), body).unwrap();
    }

    #[test]
    fn a_run_waiting_on_a_plain_task_is_waiting_not_stuck() {
        let config = scratch("waiting");
        let engine = Engine::with_config(config.clone());
        engine.bootstrap().expect("bootstrap");
        write_workflow(
            &config,
            "gate.ts",
            "import { manual, workflow } from \"@adi/workflows\";\n\
             export const on = manual();\n\
             export default workflow(async (ctx) => {\n\
             \x20 await ctx.task({ key: \"gate\", title: \"review\", agent: \"solver\" });\n\
             \x20 return {};\n\
             });\n",
        );
        let started = engine.start_manual("gate", &json!({}), "req-1").expect("start");
        assert_eq!(engine.run_tick().len(), 1);

        let tasks = Tasks::with_config(config.clone());
        let agents = Agents::with_config(config);
        let run = engine.runs().get_run(&started.run_key).unwrap().unwrap();
        assert_eq!(
            run_status(&engine, &tasks, &agents, &run),
            RunStatus::WaitingOnTask {
                task_id: tasks.find_step(&started.run_key, "gate").unwrap().map(|v| v.task.id)
            }
        );
    }

    // The `Stuck` branch itself is a direct, three-line translation of `Agents::task_is_stuck`
    // (unit-tested in `adi-agents`, where a dispatch row is its own crate's to seed) composed with
    // `Engine::waiting_on`'s `WaitingOn::Task` (unit-tested in `adi-workflows`) — reproducing a
    // genuinely stuck task here would mean re-running the whole dispatch tick's own retry/give-up
    // machinery end to end, which is what `adi_agents::dispatch`'s own test suite already does.

    #[test]
    fn a_finished_run_reports_done_or_failed_from_its_root_task() {
        let config = scratch("finished");
        let engine = Engine::with_config(config.clone());
        engine.bootstrap().expect("bootstrap");
        write_workflow(
            &config,
            "plain.ts",
            "import { manual, workflow } from \"@adi/workflows\";\n\
             export const on = manual();\n\
             export default workflow(async () => ({}));\n",
        );
        let started = engine.start_manual("plain", &json!({}), "req-1").expect("start");
        assert_eq!(engine.run_tick().len(), 1, "completes in one tick");

        let tasks = Tasks::with_config(config.clone());
        let agents = Agents::with_config(config);
        let run = engine.runs().get_run(&started.run_key).unwrap().unwrap();
        assert_eq!(run_status(&engine, &tasks, &agents, &run), RunStatus::Done);
    }
}
