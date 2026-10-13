//! adi-workflows — the WORKFLOWS engine, spec section E of ADI-MONO-141 (ADI-MONO-146, "WF-5").
//!
//! # What this crate builds, so far
//!
//! A workflow is a TypeScript file (`workflows/*.ts`, or `projects/<id>/workflows/*.ts`) that
//! exports `on` — one or several triggers built from `@adi/workflows`' `schedule(cron, tz)` /
//! `event(pattern, where?)` / `manual()`. This crate:
//!
//! * **discovers** every such file ([`discover::workflow_files`]);
//! * **describes** one, by actually running it under `bun` and reading its `on` export back as
//!   JSON ([`describe::describe`]) — never by parsing the TypeScript text, since `on` is arbitrary
//!   JS;
//! * **starts a run** when a trigger fires: [`WorkflowScheduleSource`] feeds `adi-schedules`'
//!   sweep (`schedule_tick`) for the cron half, and [`starters::tick`] (`event_tick`) is this
//!   crate's own cursor-driven consumer of the shared event log for the event half;
//! * **pours** that run — [`pour::WorkflowRunPourer`] creates the run row, its root task, and
//!   emits `adi.workflows.run.started`, exactly once per run key, whichever starter asked.
//!
//! Igor's decision on the cron dialect: a workflow file writes standard 5-field unix cron (the
//! spec's own examples, e.g. `"0 5 * * MON"`), normalized here into `adi-schedules`' 6-field
//! dialect ([`normalize_five_field`]) — anything that isn't exactly 5 fields is refused with a
//! clear error, naming the field count, rather than silently reinterpreted.
//!
//! # What this crate does NOT build yet
//!
//! The actual execution engine — the part that takes a started run's root task and drives the
//! workflow file's own `workflow(async (ctx, input) => …)` body through its `ctx` checkpoints
//! (`step`/`task`/`run`/`waitFor`/`sleep`/`ask`), replaying saved steps instantly on resume,
//! rewinding a run to an earlier step, refusing a reused key, validating `input`/`output` against
//! the file's `zod` shapes, and completing or failing the run's root task (`adi.workflows.run.
//! completed`/`failed`) — **is not built here**. A run this crate starts sits with an open root
//! task forever until that engine exists; `@adi/workflows`' `workflow()` export is a placeholder
//! for exactly this reason (see its own doc comment).
//!
//! That is a genuinely separate, large piece of work (a durable-execution runtime, in both
//! directions of the host/Bun boundary) — ADI-MONO-146 stays open for it. This run landed the
//! *starting* half: discovery, description, the two triggers' starters, and the pour every run
//! begins with, each tested end to end including through a real `bun` process. See the task's own
//! "done / remaining" notes for the precise list.
//!
//! Also deferred: `manual()` triggers (`wf@manual:<requestId>` run keys) have no starter at all
//! yet — there is no caller for one until the CLI/panel (WF-6) exist to ask for a manual run.

mod cron5;
mod describe;
mod discover;
mod error;
mod event;
mod pour;
mod source;
mod starters;
mod store;

pub use cron5::normalize_five_field;
pub use describe::{TriggerDef, describe};
pub use discover::{WorkflowFile, workflow_files};
pub use error::{Error, Result};
pub use event::WorkflowRunStarted;
pub use pour::{ROOT_STEP_KEY, WorkflowRunPourer};
pub use source::WorkflowScheduleSource;
pub use starters::{SUBSCRIBER as EVENT_STARTER_SUBSCRIBER, Started};
pub use store::{RunRow, WorkflowStore};

use adi_config::Config;
use adi_events::Events;
use adi_schedules::{Due, ScheduleStore};
use adi_tasks::Tasks;

/// One run a tick started, whichever starter poured it — [`Engine::schedule_tick`] and
/// [`Engine::event_tick`] both answer in this shape, so a caller logging "what fired" doesn't need
/// to know or care which starter it came from (the same reporting shape `adi-agents`' own
/// dispatch/goals ticks use for their own "what happened").
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StartedRun {
    pub run_key: String,
    pub workflow: String,
}

impl From<Due> for StartedRun {
    fn from(due: Due) -> Self {
        Self {
            run_key: due.run_key,
            workflow: due.workflow,
        }
    }
}

impl From<Started> for StartedRun {
    fn from(started: Started) -> Self {
        Self {
            run_key: started.run_key,
            workflow: started.workflow,
        }
    }
}

/// Bundles every store this crate's starters need, and the two ticks that drive them — the one
/// thing a host process (`adi-app`) holds and calls once a second, beside its dispatch/goals/
/// awaits ticks.
#[derive(Debug, Clone)]
pub struct Engine {
    config: Config,
    runs: WorkflowStore,
    schedules: ScheduleStore,
    tasks: Tasks,
    events: Events,
}

impl Default for Engine {
    fn default() -> Self {
        Self::open()
    }
}

impl Engine {
    /// Open the engine backed by the standard store (`~/.adi/mono`, honoring `$ADI_DIR`).
    #[must_use]
    pub fn open() -> Self {
        Self::with_config(Config::open())
    }

    /// Open the engine backed by a caller-supplied [`Config`] — for tests or alternate installs.
    #[must_use]
    pub fn with_config(config: Config) -> Self {
        Self {
            runs: WorkflowStore::with_config(config.clone()),
            schedules: ScheduleStore::with_config(config.clone()),
            tasks: Tasks::with_config(config.clone()),
            events: Events::with_config(config.clone()),
            config,
        }
    }

    /// Seed the `@adi/workflows` Bun client and the describe harness. Call once at startup,
    /// alongside `adi_db::Db::bootstrap` — every later `describe` call assumes this already ran.
    ///
    /// # Errors
    /// [`Error::Store`] if either can't be written.
    pub fn bootstrap(&self) -> Result<()> {
        describe::bootstrap(&self.config)
    }

    fn pourer(&self) -> WorkflowRunPourer {
        WorkflowRunPourer::new(
            self.config.clone(),
            self.runs.clone(),
            self.tasks.clone(),
            self.events.clone(),
        )
    }

    /// Sweep every workflow's `on: schedule(...)` against `now`, pouring whatever's newly due.
    /// Drop-in for the same once-a-second loop `adi-agents`' dispatch/goals/awaits ticks run on.
    #[must_use]
    pub fn schedule_tick(&self, now: chrono::DateTime<chrono::Utc>) -> Vec<StartedRun> {
        let source = WorkflowScheduleSource::new(self.config.clone());
        let pourer = self.pourer();
        adi_schedules::tick(&self.schedules, &source, &pourer, now)
            .into_iter()
            .map(StartedRun::from)
            .collect()
    }

    /// Catch up the shared event log and pour a run for every workflow whose `on: event(...)`
    /// matches something new since the last call.
    #[must_use]
    pub fn event_tick(&self) -> Vec<StartedRun> {
        let pourer = self.pourer();
        starters::tick(&self.events, &self.config, &pourer)
            .into_iter()
            .map(StartedRun::from)
            .collect()
    }

    /// The run store — read access for a caller that wants to show what's started (the panel,
    /// `adi-mono workflows runs`; both WF-6's job).
    #[must_use]
    pub fn runs(&self) -> &WorkflowStore {
        &self.runs
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> Config {
        let root = std::env::temp_dir().join(format!(
            "adi-workflows-engine-{tag}-{}-{:?}",
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

    /// End-to-end: bootstrap, write a schedule-triggered workflow, tick it twice (the first tick
    /// only ever establishes the cursor — same as `adi_schedules::tick`'s own documented
    /// behaviour) and see a real run land with a real root task, through an actual `bun` process.
    #[test]
    fn a_schedule_tick_starts_a_real_run_end_to_end() {
        let config = scratch("schedule-e2e");
        let engine = Engine::with_config(config.clone());
        engine.bootstrap().expect("bootstrap");
        write_workflow(
            &config,
            "weekly.ts",
            "import { schedule } from \"@adi/workflows\";\n\
             export const on = schedule(\"* * * * *\", \"UTC\");\n",
        );

        let first = chrono::DateTime::parse_from_rfc3339("2026-01-01T12:00:00Z")
            .unwrap()
            .with_timezone(&chrono::Utc);
        assert!(engine.schedule_tick(first).is_empty());

        let second = first + chrono::Duration::minutes(1);
        let started = engine.schedule_tick(second);
        assert_eq!(started.len(), 1);
        assert_eq!(started[0].workflow, "weekly");

        let run = engine
            .runs()
            .get_run(&started[0].run_key)
            .expect("read")
            .expect("exists");
        assert!(run.root_task.is_some());
    }

    /// The same end-to-end shape for the event starter.
    #[test]
    fn an_event_tick_starts_a_real_run_end_to_end() {
        let config = scratch("event-e2e");
        let engine = Engine::with_config(config.clone());
        engine.bootstrap().expect("bootstrap");
        write_workflow(
            &config,
            "on-task.ts",
            "import { event } from \"@adi/workflows\";\n\
             export const on = event(\"adi.tasks.created\");\n",
        );

        Events::with_config(config)
            .emit("adi.tasks.created", "{}")
            .expect("emit");
        let started = engine.event_tick();
        assert_eq!(started.len(), 1);
        assert_eq!(started[0].workflow, "on-task");
    }
}
