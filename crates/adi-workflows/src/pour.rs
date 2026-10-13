//! Turning a due slot or a matched event into a real run: the row, its root task, and the
//! `adi.workflows.run.started` event — spec section E, "Each run is a root task in the tree …
//! Emit `adi.workflows.run.started`." Shared by both starters ([`adi_schedules::RunPourer`] for
//! schedules, [`crate::starters`] for events) through [`WorkflowRunPourer::pour_run`], so "what a
//! pour actually does" has exactly one implementation.

use std::path::Path;

use sha2::{Digest, Sha256};

use adi_config::Config;
use adi_events::Events;
use adi_tasks::{NewTask, Tasks};

use crate::discover::{self, WorkflowFile};

/// The reserved `step_key` a run's own root task is filed under — `ctx.task` (once the execution
/// engine exists) must refuse a workflow author trying to use this key for a step of their own,
/// since it would collide with the run's own root.
pub const ROOT_STEP_KEY: &str = "_root";

/// Builds and pours real runs for whichever starter (schedule, event) decided one is due.
#[derive(Debug, Clone)]
pub struct WorkflowRunPourer {
    config: Config,
    store: crate::store::WorkflowStore,
    tasks: Tasks,
    events: Events,
}

impl WorkflowRunPourer {
    #[must_use]
    pub fn new(
        config: Config,
        store: crate::store::WorkflowStore,
        tasks: Tasks,
        events: Events,
    ) -> Self {
        Self {
            config,
            store,
            tasks,
            events,
        }
    }

    /// Pour `run_key` for `workflow` — looked up fresh among the files [`discover::workflow_files`]
    /// currently finds, so a schedule firing for a file since deleted is reported and skipped
    /// rather than poured against nothing.
    pub(crate) fn pour_for_workflow(
        &self,
        run_key: &str,
        workflow: &str,
        input: &serde_json::Value,
        cause: &serde_json::Value,
        caused_by_run: Option<&str>,
    ) {
        let files = discover::workflow_files(&self.config);
        let Some(file) = files.into_iter().find(|f| f.id == workflow) else {
            tracing::warn!(
                %workflow, %run_key,
                "a starter fired for a workflow file that no longer exists; nothing poured"
            );
            return;
        };
        self.pour_run(run_key, &file, input, cause, caused_by_run);
    }

    /// The actual pour, once the workflow file is in hand: claim the run key, hash the file
    /// (pinning this run to its current content, into a content-addressed copy so a later edit of
    /// the live file can never change what an attempt reads — see [`crate::pin`]), create the root
    /// task (its `output_schema` is the workflow's own `output` zod shape, converted to JSON
    /// Schema by [`describe::describe`] — `None` for a workflow that declares no `output`), record
    /// it on the run row, and emit `adi.workflows.run.started`. Every failure is logged and
    /// returns — never panics, and never retried by this call (the next tick will simply see the
    /// run as never-started and try again, same as any other best-effort pour).
    pub(crate) fn pour_run(
        &self,
        run_key: &str,
        file: &WorkflowFile,
        input: &serde_json::Value,
        cause: &serde_json::Value,
        caused_by_run: Option<&str>,
    ) {
        let hash = match hash_file(&file.path) {
            Ok(h) => h,
            Err(e) => {
                tracing::warn!(%run_key, path = %file.path.display(), error = %e, "couldn't hash a workflow file to pour its run");
                return;
            }
        };
        let content = match std::fs::read(&file.path) {
            Ok(bytes) => bytes,
            Err(e) => {
                tracing::warn!(%run_key, path = %file.path.display(), error = %e, "couldn't read a workflow file to pin its version");
                return;
            }
        };
        if let Err(e) = crate::pin::ensure_pinned(&self.config, &hash, &content) {
            tracing::warn!(%run_key, error = %e, "couldn't pin a workflow file's version");
            return;
        }
        let output_schema = match crate::describe::describe(&self.config, &file.path) {
            Ok(shape) => shape.output_schema,
            Err(e) => {
                tracing::warn!(%run_key, error = %e, "couldn't describe a workflow's output shape; pouring without one");
                None
            }
        };
        if let Err(e) = self.store.create_run(
            run_key,
            &file.id,
            &file.path.display().to_string(),
            &hash,
            input,
            cause,
            caused_by_run,
        ) {
            tracing::warn!(%run_key, error = %e, "couldn't claim the run row");
            return;
        }

        let new_task = NewTask {
            title: format!("workflow run: {}", file.id),
            project: file.project.clone(),
            workflow_run: Some(run_key.to_string()),
            step_key: Some(ROOT_STEP_KEY.to_string()),
            input: Some(input.clone()),
            output_schema,
            ..NewTask::default()
        };
        let task = match self.tasks.create_full(new_task) {
            Ok(view) => view,
            Err(e) => {
                tracing::warn!(%run_key, error = %e, "couldn't create the run's root task");
                return;
            }
        };
        if let Err(e) = self.store.set_root_task(run_key, &task.task.id) {
            tracing::warn!(%run_key, task = %task.task.id, error = %e, "couldn't record the root task on the run row");
        }

        let _ = self.events.emit_event(&crate::event::WorkflowRunStarted {
            run_key: run_key.to_string(),
            workflow: file.id.clone(),
            root_task: task.task.id.clone(),
            cause: cause.clone(),
            caused_by_run: caused_by_run.map(String::from),
        });
    }
}

impl adi_schedules::RunPourer for WorkflowRunPourer {
    fn pour(&self, due: &adi_schedules::Due) {
        self.pour_for_workflow(
            &due.run_key,
            &due.workflow,
            &serde_json::Value::Object(serde_json::Map::new()),
            &serde_json::json!({
                "kind": "schedule",
                "slot": due.slot.to_rfc3339(),
            }),
            // A schedule has no causer of its own — see the crate's causation module doc.
            None,
        );
    }
}

/// Hex SHA-256 of a workflow file's current bytes — the "version" half of version pinning.
fn hash_file(path: &Path) -> std::io::Result<String> {
    let bytes = std::fs::read(path)?;
    Ok(hex::encode(Sha256::digest(&bytes)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use adi_schedules::Due;
    use chrono::Utc;

    fn scratch(tag: &str) -> Config {
        let root = std::env::temp_dir().join(format!(
            "adi-workflows-pour-{tag}-{}-{:?}",
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

    fn pourer(config: &Config) -> WorkflowRunPourer {
        WorkflowRunPourer::new(
            config.clone(),
            crate::store::WorkflowStore::with_config(config.clone()),
            Tasks::with_config(config.clone()),
            Events::with_config(config.clone()),
        )
    }

    #[test]
    fn a_schedule_pour_creates_the_run_row_root_task_and_event() {
        let config = scratch("pour");
        write_workflow(&config, "weekly.ts", "export const on = [];");
        let pourer = pourer(&config);

        let due = Due {
            workflow: "weekly".into(),
            slot: Utc::now(),
            run_key: "weekly@2026-01-01T00:00:00Z".into(),
        };
        adi_schedules::RunPourer::pour(&pourer, &due);

        let store = crate::store::WorkflowStore::with_config(config.clone());
        let run = store.get_run(&due.run_key).expect("read").expect("exists");
        assert_eq!(run.workflow, "weekly");
        assert!(run.root_task.is_some());
        assert_eq!(run.cause["kind"], "schedule");

        let tasks = Tasks::with_config(config.clone());
        let task = tasks.get(run.root_task.as_deref().unwrap()).expect("task");
        assert_eq!(task.task.workflow_run.as_deref(), Some(due.run_key.as_str()));
        assert_eq!(task.task.step_key.as_deref(), Some(ROOT_STEP_KEY));

        let events = Events::with_config(config);
        let caught = events.catch_up("test", 10).expect("catch up");
        assert!(
            caught
                .iter()
                .any(|e| e.record.name == "adi.workflows.run.started"),
            "{caught:?}"
        );
    }

    #[test]
    fn a_pour_for_a_deleted_workflow_file_is_skipped_without_panicking() {
        let config = scratch("missing-file");
        let pourer = pourer(&config);
        let due = Due {
            workflow: "ghost".into(),
            slot: Utc::now(),
            run_key: "ghost@2026-01-01T00:00:00Z".into(),
        };
        adi_schedules::RunPourer::pour(&pourer, &due);

        let store = crate::store::WorkflowStore::with_config(config);
        assert_eq!(store.get_run(&due.run_key).expect("read"), None);
    }

    #[test]
    fn the_stored_file_hash_pins_the_content_at_pour_time() {
        let config = scratch("hash");
        write_workflow(&config, "weekly.ts", "export const on = [];");
        let pourer = pourer(&config);
        let due = Due {
            workflow: "weekly".into(),
            slot: Utc::now(),
            run_key: "weekly@2026-01-01T00:00:00Z".into(),
        };
        adi_schedules::RunPourer::pour(&pourer, &due);

        let store = crate::store::WorkflowStore::with_config(config.clone());
        let run_before = store.get_run(&due.run_key).unwrap().unwrap();

        // Editing the live file after the run started must not change what was pinned.
        write_workflow(&config, "weekly.ts", "export const on = []; // edited\n");
        let run_after = store.get_run(&due.run_key).unwrap().unwrap();
        assert_eq!(run_before.file_hash, run_after.file_hash);
    }

    /// The pinned copy — not just its hash — survives a later edit to the live file, since
    /// that's what an attempt actually reads.
    #[test]
    fn the_pinned_copy_survives_a_later_edit_to_the_live_file() {
        let config = scratch("pin");
        write_workflow(&config, "weekly.ts", "export const on = [];");
        let pourer = pourer(&config);
        let due = Due {
            workflow: "weekly".into(),
            slot: Utc::now(),
            run_key: "weekly@2026-01-01T00:00:00Z".into(),
        };
        adi_schedules::RunPourer::pour(&pourer, &due);

        let store = crate::store::WorkflowStore::with_config(config.clone());
        let run = store.get_run(&due.run_key).unwrap().unwrap();
        let pinned = crate::pin::require_pinned(&config, &run.file_hash).expect("pinned copy");
        assert_eq!(
            std::fs::read_to_string(pinned).unwrap(),
            "export const on = [];"
        );
    }

    /// A `ctx.run` child pour names its causer — the parent run — on both the row and the
    /// `adi.workflows.run.started` event, which is what [`crate::causation`] walks.
    #[test]
    fn a_pour_with_a_causer_records_it_on_the_run_and_the_started_event() {
        let config = scratch("caused-by");
        write_workflow(&config, "child.ts", "export const on = [];");
        let pourer = pourer(&config);

        pourer.pour_for_workflow(
            "child@parent@1/step",
            "child",
            &serde_json::json!({}),
            &serde_json::json!({"kind": "child", "parent": "parent@1", "key": "step"}),
            Some("parent@1"),
        );

        let store = crate::store::WorkflowStore::with_config(config.clone());
        let run = store.get_run("child@parent@1/step").unwrap().unwrap();
        assert_eq!(run.caused_by_run.as_deref(), Some("parent@1"));

        let events = Events::with_config(config);
        let caught = events.catch_up("test", 10).expect("catch up");
        let started = caught
            .iter()
            .find(|e| e.record.name == "adi.workflows.run.started")
            .expect("run.started");
        let payload: serde_json::Value = serde_json::from_str(&started.record.payload).unwrap();
        assert_eq!(payload["caused_by_run"], "parent@1");
    }

    /// `output_schema` on the root task comes from the workflow's own declared `output` zod
    /// shape, converted to JSON Schema — spec section E: "Store the JSON Schema on tasks via
    /// `create_full`."
    #[test]
    fn the_roots_task_output_schema_comes_from_the_workflows_declared_output() {
        let config = scratch("output-schema");
        crate::describe::bootstrap(&config).expect("bootstrap");
        write_workflow(
            &config,
            "typed.ts",
            "import { z } from \"zod\";\n\
             export const on = [];\n\
             export const output = z.object({ ok: z.boolean() });\n",
        );
        let pourer = pourer(&config);
        let due = Due {
            workflow: "typed".into(),
            slot: Utc::now(),
            run_key: "typed@2026-01-01T00:00:00Z".into(),
        };
        adi_schedules::RunPourer::pour(&pourer, &due);

        let store = crate::store::WorkflowStore::with_config(config.clone());
        let run = store.get_run(&due.run_key).unwrap().unwrap();
        let tasks = Tasks::with_config(config);
        let task = tasks.get(run.root_task.as_deref().unwrap()).expect("task");
        let schema = task.task.output_schema.expect("output schema stored");
        assert_eq!(schema["properties"]["ok"]["type"], "boolean");
    }
}
