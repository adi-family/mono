//! The event starter: a workflow's `on: event(pattern, where?)` fires a brand-new run, keyed
//! `${workflow}@evt:<seq>` — spec section E's run-key union, and the explicit WF-5 ask "event
//! starters consume the log through a cursor."
//!
//! Structurally this is `adi-triggers`' own [`adi_triggers::EventDispatcher`] pattern, not
//! `adi-schedules`' (there is no per-rule due-slot to compute; an event either matches or it
//! doesn't, the instant it's read off the log) — a plain [`adi_events::Events::catch_up`] /
//! [`adi_events::Events::ack`] cursor is the whole mechanism. The run-key itself is the exactly-
//! once claim: [`crate::store::WorkflowStore::create_run`]'s `UNIQUE` on `run_key` is what makes
//! redelivering an unacked batch after a crash safe to just redo in full.

use adi_events::Events;

use crate::causation;
use crate::describe::TriggerDef;
use crate::discover::{self, WorkflowFile};
use crate::pour::WorkflowRunPourer;
use crate::store::WorkflowStore;

/// This starter's cursor name on the shared event log. Load-bearing across restarts — never
/// rename once a real install has caught up under it (same rule as
/// `adi_triggers::SUBSCRIBER`).
pub const SUBSCRIBER: &str = "workflows.event-starter";

/// How many log rows one tick reads at most — bounds a single tick's work the same way
/// `adi_triggers::dispatch`'s own `MAX_PER_TICK` does for its event dispatch.
const MAX_PER_TICK: usize = 200;

/// One run this tick started.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Started {
    pub run_key: String,
    pub workflow: String,
}

/// Catch up on the shared log, pour a run for every workflow whose `event` trigger matches a
/// batch row, and ack the whole batch — once, regardless of how many runs it started. A workflow
/// file that fails to describe (bad syntax, a broken import) is logged and treated as having no
/// event triggers for this tick, exactly like `adi_schedules::tick` skipping one bad rule: it
/// must never stop the rest of the batch from being matched against every other workflow.
///
/// Before pouring, reads the matched event's own causer (`caused_by_run`/`workflow_run` on its
/// payload — see [`causation::causer_of_payload`]) and refuses the match — logging why, starting
/// nothing — when [`causation::would_cycle`] finds that causer's own chain already includes a run
/// of the very workflow this match would start: Igor's fix for a workflow retriggering itself,
/// directly or through a few hops of other workflows.
pub fn tick(
    events: &Events,
    config: &adi_config::Config,
    store: &WorkflowStore,
    pourer: &WorkflowRunPourer,
) -> Vec<Started> {
    let batch = match events.catch_up(SUBSCRIBER, MAX_PER_TICK) {
        Ok(batch) => batch,
        Err(e) => {
            tracing::warn!(error = %e, "couldn't catch up the workflow event starter");
            return Vec::new();
        }
    };
    if batch.is_empty() {
        return Vec::new();
    }

    let files = discover::workflow_files(config);
    let mut started = Vec::new();
    for logged in &batch {
        let payload: serde_json::Value =
            serde_json::from_str(&logged.record.payload).unwrap_or(serde_json::Value::Null);
        let causer = causation::causer_of_payload(&payload);
        for file in &files {
            for trigger in event_triggers(config, file) {
                if !adi_events::matches(&trigger.pattern, &logged.record.name) {
                    continue;
                }
                if !where_matches(trigger.where_clause.as_ref(), &payload) {
                    continue;
                }
                if let Some(causer_run) = &causer
                    && causation::would_cycle(store, causer_run, &file.id)
                {
                    tracing::warn!(
                        workflow = %file.id, event = %logged.record.name, causer = %causer_run,
                        "refusing to start a run: this event's own causation chain already \
                         includes a run of this workflow"
                    );
                    continue;
                }
                let run_key = format!("{}@evt:{}", file.id, logged.seq);
                pourer.pour_run(
                    &run_key,
                    file,
                    &payload,
                    &serde_json::json!({
                        "kind": "event",
                        "seq": logged.seq,
                        "name": logged.record.name,
                    }),
                    causer.as_deref(),
                );
                started.push(Started {
                    run_key,
                    workflow: file.id.clone(),
                });
            }
        }
    }

    if let Some(last) = batch.last()
        && let Err(e) = events.ack(SUBSCRIBER, last.seq)
    {
        tracing::warn!(error = %e, "couldn't ack the workflow event starter's cursor");
    }
    started
}

/// Just the `event(...)` triggers out of a workflow file's full `on` list — a describe failure is
/// logged and reads as "none", so one broken file never stops the rest of the batch matching.
fn event_triggers(config: &adi_config::Config, file: &WorkflowFile) -> Vec<EventPattern> {
    match crate::describe::describe(config, &file.path) {
        Ok(shape) => shape
            .on
            .into_iter()
            .filter_map(|t| match t {
                TriggerDef::Event {
                    pattern,
                    where_clause,
                } => Some(EventPattern {
                    pattern,
                    where_clause,
                }),
                TriggerDef::Schedule { .. } | TriggerDef::Manual => None,
            })
            .collect(),
        Err(e) => {
            tracing::warn!(workflow = %file.id, path = %file.path.display(), error = %e, "couldn't describe a workflow file; treating it as having no event triggers this tick");
            Vec::new()
        }
    }
}

struct EventPattern {
    pattern: String,
    where_clause: Option<serde_json::Map<String, serde_json::Value>>,
}

/// Whether every key in `where_clause` equals the same field in `payload` — `None` (no `where`
/// given) always matches, since the pattern alone was the whole ask.
fn where_matches(
    where_clause: Option<&serde_json::Map<String, serde_json::Value>>,
    payload: &serde_json::Value,
) -> bool {
    let Some(clause) = where_clause else {
        return true;
    };
    clause
        .iter()
        .all(|(key, want)| payload.get(key) == Some(want))
}

#[cfg(test)]
mod tests {
    use super::*;
    use adi_config::Config;
    use adi_tasks::Tasks;

    fn scratch(tag: &str) -> Config {
        let root = std::env::temp_dir().join(format!(
            "adi-workflows-starters-{tag}-{}-{:?}",
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

    fn store(config: &Config) -> WorkflowStore {
        WorkflowStore::with_config(config.clone())
    }

    fn pourer(config: &Config) -> WorkflowRunPourer {
        WorkflowRunPourer::new(
            config.clone(),
            store(config),
            Tasks::with_config(config.clone()),
            Events::with_config(config.clone()),
        )
    }

    #[test]
    fn a_matching_event_starts_a_run_keyed_by_its_seq() {
        let config = scratch("match");
        crate::describe::bootstrap(&config).expect("bootstrap");
        write_workflow(
            &config,
            "on-task.ts",
            "import { event } from \"@adi/workflows\";\n\
             export const on = event(\"adi.tasks.created\");\n",
        );
        let events = Events::with_config(config.clone());
        events.emit("adi.tasks.created", r#"{"id":"t1"}"#).expect("emit");

        let started = tick(&events, &config, &store(&config), &pourer(&config));
        assert_eq!(started.len(), 1);
        assert_eq!(started[0].workflow, "on-task");
        assert!(started[0].run_key.starts_with("on-task@evt:"));

        let store = crate::store::WorkflowStore::with_config(config);
        let run = store.get_run(&started[0].run_key).unwrap().unwrap();
        assert_eq!(run.input, serde_json::json!({"id": "t1"}));
    }

    #[test]
    fn a_where_clause_narrows_the_match() {
        let config = scratch("where");
        crate::describe::bootstrap(&config).expect("bootstrap");
        write_workflow(
            &config,
            "on-demo.ts",
            "import { event } from \"@adi/workflows\";\n\
             export const on = event(\"adi.tasks.created\", { project: \"demo\" });\n",
        );
        let events = Events::with_config(config.clone());
        events
            .emit("adi.tasks.created", r#"{"project":"other"}"#)
            .expect("emit non-matching");
        events
            .emit("adi.tasks.created", r#"{"project":"demo"}"#)
            .expect("emit matching");

        let started = tick(&events, &config, &store(&config), &pourer(&config));
        assert_eq!(started.len(), 1, "only the matching payload starts a run");
    }

    #[test]
    fn a_non_matching_pattern_starts_nothing_but_still_acks() {
        let config = scratch("no-match");
        crate::describe::bootstrap(&config).expect("bootstrap");
        write_workflow(
            &config,
            "on-other.ts",
            "import { event } from \"@adi/workflows\";\n\
             export const on = event(\"adi.projects.*\");\n",
        );
        let events = Events::with_config(config.clone());
        events.emit("adi.tasks.created", "{}").expect("emit");

        assert!(tick(&events, &config, &store(&config), &pourer(&config)).is_empty());
        assert!(
            events.catch_up(SUBSCRIBER, 10).unwrap().is_empty(),
            "the batch was still acked even though nothing matched"
        );
    }

    #[test]
    fn the_same_event_never_starts_the_same_workflow_twice() {
        let config = scratch("idempotent");
        crate::describe::bootstrap(&config).expect("bootstrap");
        // Deliberately not `adi.tasks.created`: pouring a run itself creates a root task, which
        // emits exactly that event — a workflow listening for its own starter's by-product would
        // never stop retriggering, which is a hazard for a *real* workflow author to guard with a
        // `where` clause, not something this test should trip over by coincidence.
        write_workflow(
            &config,
            "on-custom.ts",
            "import { event } from \"@adi/workflows\";\n\
             export const on = event(\"test.custom.happened\");\n",
        );
        let events = Events::with_config(config.clone());
        events.emit("test.custom.happened", "{}").expect("emit");
        let store = store(&config);
        let pourer = pourer(&config);

        let first = tick(&events, &config, &store, &pourer);
        assert_eq!(first.len(), 1);
        // A second tick with nothing new on the log starts nothing more.
        let second = tick(&events, &config, &store, &pourer);
        assert!(second.is_empty());
    }

    #[test]
    fn a_workflow_file_that_fails_to_describe_never_blocks_the_rest_of_the_batch() {
        let config = scratch("broken-skip");
        crate::describe::bootstrap(&config).expect("bootstrap");
        write_workflow(&config, "broken.ts", "export const on = (;\n");
        write_workflow(
            &config,
            "fine.ts",
            "import { event } from \"@adi/workflows\";\n\
             export const on = event(\"adi.tasks.created\");\n",
        );
        let events = Events::with_config(config.clone());
        events.emit("adi.tasks.created", "{}").expect("emit");

        let started = tick(&events, &config, &store(&config), &pourer(&config));
        assert_eq!(started.len(), 1);
        assert_eq!(started[0].workflow, "fine");
    }

    /// The direct self-retrigger case Igor flagged: a workflow listening on `adi.tasks.created`
    /// would otherwise retrigger itself forever once its own pour creates a root task. The task
    /// event's `workflow_run` field (already on every task event, from WF-1) names the run that
    /// caused it — this starter must see that it's a run of the very workflow matching the event,
    /// and refuse.
    #[test]
    fn a_workflow_is_refused_from_an_event_its_own_run_caused() {
        let config = scratch("self-retrigger-direct");
        crate::describe::bootstrap(&config).expect("bootstrap");
        write_workflow(
            &config,
            "on-task.ts",
            "import { event } from \"@adi/workflows\";\n\
             export const on = event(\"adi.tasks.created\");\n",
        );
        let events = Events::with_config(config.clone());
        let store = store(&config);
        let pourer = pourer(&config);

        // Simulate a task-created event whose payload already carries `workflow_run` naming a
        // real run of "on-task" — exactly what pouring "on-task" itself would emit.
        store
            .create_run("on-task@evt:0", "on-task", "/f.ts", "h", &serde_json::json!({}), &serde_json::json!({}), None)
            .expect("seed the causing run");
        events
            .emit(
                "adi.tasks.created",
                r#"{"workflow_run":"on-task@evt:0","step_key":"_root"}"#,
            )
            .expect("emit");

        let started = tick(&events, &config, &store, &pourer);
        assert!(
            started.is_empty(),
            "a self-caused event must never start another run of the same workflow"
        );
        // The batch is still acked (same "never block the rest of the log" rule as a non-match).
        assert!(events.catch_up(SUBSCRIBER, 10).unwrap().is_empty());
    }

    /// The refusal is scoped to the *matching* workflow, not every event with a causer: a
    /// different workflow reacting to the very same task-created event must still be started.
    #[test]
    fn a_different_workflow_is_not_refused_by_another_workflows_self_cause() {
        let config = scratch("self-retrigger-other");
        crate::describe::bootstrap(&config).expect("bootstrap");
        write_workflow(
            &config,
            "on-task.ts",
            "import { event } from \"@adi/workflows\";\n\
             export const on = event(\"adi.tasks.created\");\n",
        );
        write_workflow(
            &config,
            "watcher.ts",
            "import { event } from \"@adi/workflows\";\n\
             export const on = event(\"adi.tasks.created\");\n",
        );
        let events = Events::with_config(config.clone());
        let store = store(&config);
        let pourer = pourer(&config);

        store
            .create_run("on-task@evt:0", "on-task", "/f.ts", "h", &serde_json::json!({}), &serde_json::json!({}), None)
            .expect("seed");
        events
            .emit("adi.tasks.created", r#"{"workflow_run":"on-task@evt:0"}"#)
            .expect("emit");

        let started = tick(&events, &config, &store, &pourer);
        let workflows: Vec<&str> = started.iter().map(|s| s.workflow.as_str()).collect();
        assert_eq!(workflows, vec!["watcher"], "only the self-caused one is refused");
    }
}
