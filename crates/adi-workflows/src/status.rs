//! Deriving what an in-flight run is blocked on, from the stores alone — no stored "waiting on
//! X" anywhere, same level-triggered principle as the rest of this crate. WF-6's `adi-mono
//! workflows runs` and the panel's run page both read this to show "waiting on task/event/sleep/
//! child" without re-running the workflow body under `bun`.
//!
//! What a replay actually leaves behind, per block type, is what [`waiting_on`] tells apart:
//! - `ctx.task`/`ctx.ask`: a `step_pending` row shaped `{started_at_ms, timeout_ms}` — no
//!   `since_seq`, no `until_ms`.
//! - `ctx.waitFor`: a `step_pending` row shaped `{since_seq, started_at_ms, timeout_ms}`.
//! - `ctx.sleep`: a `step_pending` row shaped `{until_ms}` only.
//! - `ctx.run`: **no** `step_pending` row at all — the child run's own (unfinished) existence is
//!   the only memory of it between attempts, which is why this is the one case `waiting_on` has
//!   to ask [`WorkflowStore::has_unfinished_child`] rather than read a pending marker.
//!
//! A run with none of the above (nothing pending, no unfinished child) is between attempts —
//! freshly poured, or about to be retried on the next tick — reported as [`WaitingOn::None`] by a
//! caller that wants to tell that apart from "blocked on something" (CLI/panel: "running").

use adi_tasks::Tasks;

use crate::error::Result;
use crate::store::WorkflowStore;

/// What a run is blocked on right now, derived fresh — never itself stored. See the module doc
/// for exactly how each is told apart from the others.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum WaitingOn {
    /// A `ctx.task`/`ctx.ask` step's underlying task hasn't resolved yet. `task_id` is the real
    /// task to act on (`None` only if the step just suspended this instant, before its pending
    /// marker's step key could be resolved back to a task — vanishingly rare, and never wrong to
    /// treat as "no task to point at yet").
    Task { task_id: Option<String> },
    /// A `ctx.waitFor` hasn't seen a matching event yet.
    Event,
    /// A `ctx.sleep` hasn't reached its wake time yet.
    Sleep,
    /// A `ctx.run` child hasn't finished yet.
    Child,
}

/// What `run_key` is blocked on, or `None` for a run between attempts (nothing pending, no
/// unfinished child) — freshly poured, or simply due for its next retry.
///
/// # Errors
/// [`crate::error::Error::Store`] on a read failure.
pub fn waiting_on(store: &WorkflowStore, tasks: &Tasks, run_key: &str) -> Result<Option<WaitingOn>> {
    for (step_key, state) in store.list_pending(run_key)? {
        if state.get("since_seq").is_some() {
            return Ok(Some(WaitingOn::Event));
        }
        if state.get("until_ms").is_some() {
            return Ok(Some(WaitingOn::Sleep));
        }
        if state.get("started_at_ms").is_some() {
            let task_id = tasks
                .find_step(run_key, &step_key)
                .ok()
                .flatten()
                .map(|view| view.task.id);
            return Ok(Some(WaitingOn::Task { task_id }));
        }
    }
    if store.has_unfinished_child(run_key)? {
        return Ok(Some(WaitingOn::Child));
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use adi_config::Config;

    use super::*;
    use crate::Engine;

    fn scratch(tag: &str) -> Config {
        let root = std::env::temp_dir().join(format!(
            "adi-workflows-status-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id(),
        ));
        let _ = std::fs::remove_dir_all(&root);
        Config::with_root(root)
    }

    fn write_workflow(config: &Config, name: &str, body: &str) {
        let dir = config.module("workflows").dir().to_path_buf();
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(name), body).unwrap();
    }

    #[test]
    fn a_fresh_run_with_no_attempt_yet_waits_on_nothing() {
        let config = scratch("fresh");
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
        let tasks = Tasks::with_config(config.clone());
        let store = crate::store::WorkflowStore::with_config(config);
        assert_eq!(waiting_on(&store, &tasks, &started.run_key).unwrap(), None);
    }

    #[test]
    fn a_person_task_step_waits_on_task_naming_the_real_task_id() {
        let config = scratch("task");
        let engine = Engine::with_config(config.clone());
        engine.bootstrap().expect("bootstrap");
        write_workflow(
            &config,
            "gate.ts",
            "import { manual, workflow } from \"@adi/workflows\";\n\
             export const on = manual();\n\
             export default workflow(async (ctx) => {\n\
             \x20 await ctx.task({ key: \"gate\", title: \"review\", person: \"igor\" });\n\
             \x20 return {};\n\
             });\n",
        );
        let started = engine.start_manual("gate", &json!({}), "req-1").expect("start");
        assert_eq!(engine.run_tick().len(), 1, "the one attempt that suspends on the task");

        let tasks = Tasks::with_config(config.clone());
        let store = crate::store::WorkflowStore::with_config(config);
        let waiting = waiting_on(&store, &tasks, &started.run_key).unwrap();
        let Some(WaitingOn::Task { task_id: Some(task_id) }) = waiting else {
            panic!("expected WaitingOn::Task with a real task id, got {waiting:?}");
        };
        let view = tasks.get(&task_id).expect("the task exists");
        assert_eq!(view.task.title, "review");
    }

    #[test]
    fn a_wait_for_step_waits_on_event() {
        let config = scratch("event");
        let engine = Engine::with_config(config.clone());
        engine.bootstrap().expect("bootstrap");
        write_workflow(
            &config,
            "waiter.ts",
            "import { manual, workflow } from \"@adi/workflows\";\n\
             export const on = manual();\n\
             export default workflow(async (ctx) => {\n\
             \x20 await ctx.waitFor({ key: \"w\", event: \"test.go\" });\n\
             \x20 return {};\n\
             });\n",
        );
        let started = engine.start_manual("waiter", &json!({}), "req-1").expect("start");
        assert_eq!(engine.run_tick().len(), 1);

        let tasks = Tasks::with_config(config.clone());
        let store = crate::store::WorkflowStore::with_config(config);
        assert_eq!(
            waiting_on(&store, &tasks, &started.run_key).unwrap(),
            Some(WaitingOn::Event)
        );
    }

    #[test]
    fn a_sleep_step_waits_on_sleep() {
        let config = scratch("sleep");
        let engine = Engine::with_config(config.clone());
        engine.bootstrap().expect("bootstrap");
        write_workflow(
            &config,
            "napper.ts",
            "import { manual, workflow } from \"@adi/workflows\";\n\
             export const on = manual();\n\
             export default workflow(async (ctx, input) => {\n\
             \x20 await ctx.sleep({ key: \"nap\", until: input.until });\n\
             \x20 return {};\n\
             });\n",
        );
        let far_future = crate::now_ms() + 3_600_000;
        let started = engine
            .start_manual("napper", &json!({"until": far_future}), "req-1")
            .expect("start");
        assert_eq!(engine.run_tick().len(), 1);

        let tasks = Tasks::with_config(config.clone());
        let store = crate::store::WorkflowStore::with_config(config);
        assert_eq!(
            waiting_on(&store, &tasks, &started.run_key).unwrap(),
            Some(WaitingOn::Sleep)
        );
    }

    #[test]
    fn a_run_step_waits_on_child_with_no_pending_row_at_all() {
        let config = scratch("child");
        let engine = Engine::with_config(config.clone());
        engine.bootstrap().expect("bootstrap");
        write_workflow(
            &config,
            "child.ts",
            "import { workflow } from \"@adi/workflows\";\n\
             export const on = [];\n\
             export default workflow(async (ctx) => {\n\
             \x20 await ctx.task({ key: \"gate\", title: \"review\", person: \"igor\" });\n\
             \x20 return {};\n\
             });\n",
        );
        write_workflow(
            &config,
            "parent.ts",
            "import { manual, workflow } from \"@adi/workflows\";\n\
             export const on = manual();\n\
             export default workflow(async (ctx) => {\n\
             \x20 await ctx.run({ key: \"c1\", workflow: \"child\" });\n\
             \x20 return {};\n\
             });\n",
        );
        let started = engine.start_manual("parent", &json!({}), "req-1").expect("start");
        assert_eq!(engine.run_tick().len(), 1, "the parent suspends on its child");

        let tasks = Tasks::with_config(config.clone());
        let store = crate::store::WorkflowStore::with_config(config);
        assert_eq!(
            waiting_on(&store, &tasks, &started.run_key).unwrap(),
            Some(WaitingOn::Child)
        );
    }
}
