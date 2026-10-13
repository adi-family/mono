//! "Rewind to step N" — spec section E: "discards journal entries from N onward." Journal
//! truncation alone ([`WorkflowStore::truncate_steps`]) would let a *replayed* `ctx.task`/
//! `ctx.run` immediately see its own already-finished task or child run still sitting there
//! (`stepId`/run-key uniqueness is a `UNIQUE` constraint, not a version history) and re-resolve
//! to the exact same stale result — a rewind that looks like it happened but never actually
//! redoes anything. This module is the rest of it: for every discarded step that created a task
//! ([`StepKind::Task`]/[`StepKind::Ask`]) or a child run ([`StepKind::Run`]), delete that task
//! (or that child run's row and its own root task) too, so the next attempt's `ctx.task`/
//! `ctx.run` genuinely creates a fresh one under the freed `stepId`/run key.
//!
//! **What this deliberately does not do**, and why that's an accepted limit rather than an
//! oversight: a discarded `ctx.run` step's child run may itself have grandchildren (its own
//! journal, its own further child runs) — this deletes only the child's own row and root task,
//! leaving any deeper state as orphaned rows nobody references again. A recursive cascade is a
//! real follow-up (flag it for `guides/workflows.md`, WF-6), not required by the spec's one
//! sentence about rewind, and the orphaned rows are harmless dead weight, never read by anything.

use crate::store::{StepKind, WorkflowStore};
use crate::{Error, Result};

/// Discard `run_key`'s journal past the first `keep` entries (by resolution order — "step N" in
/// the spec's own words), cleaning up whatever each discarded step created.
///
/// # Errors
/// [`Error::Store`] on a read or write failure partway through — a step whose task was already
/// deleted by an earlier partial rewind call is treated as already clean, not re-raised.
pub fn rewind(store: &WorkflowStore, tasks: &adi_tasks::Tasks, run_key: &str, keep: usize) -> Result<()> {
    let discarded = store.truncate_steps(run_key, keep)?;
    for step in discarded {
        match step.kind {
            StepKind::Task | StepKind::Ask => {
                if let Some(task_id) = &step.task_id {
                    delete_task_if_present(tasks, task_id)?;
                }
            }
            StepKind::Run => {
                if let Some(task_id) = &step.task_id {
                    delete_task_if_present(tasks, task_id)?;
                }
                if let Some(child_run_key) = &step.ref_key {
                    store.delete_run(child_run_key)?;
                }
            }
            StepKind::Step | StepKind::Wait | StepKind::Sleep => {}
        }
    }
    // A rewound run needs picking back up: clear its terminal mark (if it had reached one) and
    // reopen its root task (if it had been completed or given up on) so the tree reflects "back
    // in progress", not stale success or failure from before the rewind.
    store.unfinish(run_key)?;
    if let Some(run) = store.get_run(run_key)?
        && let Some(root_task) = run.root_task
    {
        tasks
            .reopen(&root_task)
            .map_err(|e| Error::Store(anyhow::anyhow!("couldn't reopen the root task: {e}")))?;
    }
    Ok(())
}

/// Delete a task, treating "already gone" (another rewind, or a person deleting it by hand) as
/// success rather than an error — the goal is "this stepId is free again", which a missing task
/// already satisfies.
fn delete_task_if_present(tasks: &adi_tasks::Tasks, task_id: &str) -> Result<()> {
    match tasks.delete(task_id) {
        Ok(()) | Err(adi_tasks::Error::NotFound(_)) => Ok(()),
        Err(e) => Err(Error::Store(anyhow::anyhow!("couldn't delete a rewound step's task: {e}"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use adi_config::Config;
    use adi_tasks::Tasks;
    use serde_json::json;

    use crate::store::{StepOutcome, StepRecord};

    fn scratch(tag: &str) -> Config {
        let root = std::env::temp_dir().join(format!(
            "adi-workflows-rewind-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id(),
        ));
        let _ = std::fs::remove_dir_all(&root);
        Config::with_root(root)
    }

    #[test]
    fn rewind_discards_steps_after_keep_and_deletes_their_tasks() {
        let config = scratch("basic");
        let store = WorkflowStore::with_config(config.clone());
        let tasks = Tasks::with_config(config.clone());
        store
            .create_run("r@1", "r", "/r.ts", "h", &json!({}), &json!({}), None)
            .expect("create run");

        store
            .put_step("r@1", &StepRecord::new("a", StepKind::Step, StepOutcome::Ok(json!(1))))
            .expect("step a");
        let task = tasks
            .create_full(adi_tasks::NewTask {
                title: "b".into(),
                workflow_run: Some("r@1".into()),
                step_key: Some("b".into()),
                ..adi_tasks::NewTask::default()
            })
            .expect("create task b");
        store
            .put_step(
                "r@1",
                &StepRecord::new("b", StepKind::Task, StepOutcome::Ok(json!({"ok": true})))
                    .with_task(task.task.id.clone()),
            )
            .expect("step b");
        store
            .put_step("r@1", &StepRecord::new("c", StepKind::Step, StepOutcome::Ok(json!(3))))
            .expect("step c");

        rewind(&store, &tasks, "r@1", 1).expect("rewind");

        let remaining = store.list_steps("r@1").expect("list");
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].step_key, "a");

        // b's task is gone — the stepId is free for a genuine redo.
        assert!(matches!(tasks.get(&task.task.id), Err(adi_tasks::Error::NotFound(_))));
        assert!(
            tasks
                .find_step("r@1", "b")
                .expect("find_step")
                .is_none()
        );
    }

    #[test]
    fn rewind_also_deletes_a_discarded_child_runs_row_and_root_task() {
        let config = scratch("child");
        let store = WorkflowStore::with_config(config.clone());
        let tasks = Tasks::with_config(config.clone());
        store
            .create_run("parent@1", "parent", "/p.ts", "h", &json!({}), &json!({}), None)
            .expect("parent");

        let child_task = tasks
            .create_full(adi_tasks::NewTask {
                title: "child root".into(),
                workflow_run: Some("child@parent@1/step".into()),
                step_key: Some("_root".into()),
                ..adi_tasks::NewTask::default()
            })
            .expect("child root task");
        store
            .create_run(
                "child@parent@1/step",
                "child",
                "/c.ts",
                "h",
                &json!({}),
                &json!({}),
                Some("parent@1"),
            )
            .expect("child run row");
        store
            .set_root_task("child@parent@1/step", &child_task.task.id)
            .expect("set root");
        store
            .put_step(
                "parent@1",
                &StepRecord::new("step", StepKind::Run, StepOutcome::Ok(json!({})))
                    .with_task(child_task.task.id.clone())
                    .with_ref("child@parent@1/step"),
            )
            .expect("journal the run step");

        rewind(&store, &tasks, "parent@1", 0).expect("rewind to the very start");

        assert_eq!(store.get_run("child@parent@1/step").expect("read"), None);
        assert!(matches!(
            tasks.get(&child_task.task.id),
            Err(adi_tasks::Error::NotFound(_))
        ));
    }

    #[test]
    fn rewind_past_the_end_is_a_no_op() {
        let config = scratch("noop");
        let store = WorkflowStore::with_config(config.clone());
        let tasks = Tasks::with_config(config.clone());
        store
            .create_run("r@1", "r", "/r.ts", "h", &json!({}), &json!({}), None)
            .expect("create run");
        store
            .put_step("r@1", &StepRecord::new("a", StepKind::Step, StepOutcome::Ok(json!(1))))
            .expect("step a");

        rewind(&store, &tasks, "r@1", 10).expect("rewind");
        assert_eq!(store.list_steps("r@1").expect("list").len(), 1);
    }

    #[test]
    fn rewinding_a_step_whose_task_was_already_deleted_is_not_an_error() {
        let config = scratch("already-gone");
        let store = WorkflowStore::with_config(config.clone());
        let tasks = Tasks::with_config(config.clone());
        store
            .create_run("r@1", "r", "/r.ts", "h", &json!({}), &json!({}), None)
            .expect("create run");
        store
            .put_step(
                "r@1",
                &StepRecord::new("b", StepKind::Task, StepOutcome::Ok(json!({})))
                    .with_task("ghost-task"),
            )
            .expect("step b");

        rewind(&store, &tasks, "r@1", 0).expect("rewind must not fail on a missing task");
    }
}
