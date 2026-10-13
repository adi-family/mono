//! adi-workflows — the WORKFLOWS engine, spec section E of ADI-MONO-141 (ADI-MONO-146, "WF-5").
//!
//! # What this crate builds
//!
//! A workflow is a TypeScript file (`workflows/*.ts`, or `projects/<id>/workflows/*.ts`) that
//! exports `on` — one or several triggers built from `@adi/workflows`' `schedule(cron, tz)` /
//! `event(pattern, where?)` / `manual()` — and a `default workflow(async (ctx, input) => …)`
//! body. This crate:
//!
//! * **discovers** every such file ([`discover::workflow_files`]);
//! * **describes** one, by actually running it under `bun` and reading its `on`/`input`/`output`
//!   exports back as JSON ([`describe::describe`]) — never by parsing the TypeScript text, since
//!   `on` is arbitrary JS;
//! * **starts a run** when a trigger fires: [`WorkflowScheduleSource`] feeds `adi-schedules`'
//!   sweep ([`Engine::schedule_tick`]) for the cron half, [`starters::tick`]
//!   ([`Engine::event_tick`]) is this crate's own cursor-driven consumer of the shared event log
//!   for the event half, and [`Engine::start_manual`] is the one-call starter for an explicit
//!   `wf@manual:<requestId>` run (the CLI/panel command that calls it is WF-6's job, not this
//!   crate's);
//! * **pours** that run — [`pour::WorkflowRunPourer`] creates the run row, its root task, and
//!   emits `adi.workflows.run.started`, exactly once per run key, whichever starter asked;
//! * **drives** it: [`Engine::run_tick`] attempts every unfinished run that's actually *due*
//!   ([`readiness::Readiness`] — a run with nothing new to look at since its last attempt costs a
//!   few cheap reads, never a `bun` spawn), spawning a fresh `bun` process ([`attempt::Attempt`])
//!   that replays the workflow body from the top, relaying each `ctx` checkpoint —
//!   `step`/`task`/`run`/`waitFor`/`sleep`/`ask` — to this crate over a line-delimited JSON pipe
//!   ([`rpc::Request`]; `@adi/workflows`' `createCtx` is the TS side of the same contract) and
//!   journaling every resolution ([`store::WorkflowStore`]'s `step_journal`), so a saved step
//!   replays instantly and a checkpoint that hasn't resolved yet just suspends the attempt
//!   cleanly, tried again once it's due;
//! * **rewinds** a run to an earlier step ([`Engine::rewind`]), discarding its journal past that
//!   point and deleting whatever task or child run each discarded step had created, so the redo
//!   is genuine rather than a replay of the stale answer (see [`rewind`]'s own doc for the one
//!   documented limit on that).
//!
//! Igor's decisions, both load-bearing on how this reads:
//!
//! * the cron dialect — a workflow file writes standard 5-field unix cron (the spec's own
//!   examples, e.g. `"0 5 * * MON"`), normalized here into `adi-schedules`' 6-field dialect
//!   ([`normalize_five_field`]) — anything that isn't exactly 5 fields is refused with a clear
//!   error, naming the field count, rather than silently reinterpreted;
//! * the self-retrigger fix — every `adi.workflows.run.*` event carries `caused_by_run`, and every
//!   task event already carries the same fact as `workflow_run`; [`causation::would_cycle`] walks
//!   that chain, up to 8 hops, so an event starter refuses to start workflow `W` from an event
//!   whose own causation already includes a run of `W`, directly or through a few other
//!   workflows in between.

mod attempt;
mod causation;
mod cron5;
mod describe;
mod discover;
mod error;
mod event;
mod pin;
mod pour;
mod readiness;
mod rewind;
mod rpc;
mod source;
mod starters;
mod store;

pub use attempt::Outcome as AttemptOutcome;
pub use cron5::normalize_five_field;
pub use describe::{TriggerDef, WorkflowShape, describe, run_script_path};
pub use discover::{WorkflowFile, workflow_files};
pub use error::{Error, Result};
pub use event::{WorkflowRunCompleted, WorkflowRunFailed, WorkflowRunStarted, event_types};
pub use pour::{ROOT_STEP_KEY, WorkflowRunPourer};
pub use source::WorkflowScheduleSource;
pub use starters::{SUBSCRIBER as EVENT_STARTER_SUBSCRIBER, Started};
pub use store::{RunRow, StepKind, StepOutcome, StepRecord, WorkflowStore};

use std::sync::Arc;

use adi_config::Config;
use adi_events::Events;
use adi_schedules::{Due, ScheduleStore};
use adi_tasks::Tasks;

/// Now, as epoch milliseconds — shared by [`attempt`] (a `ctx.task`/`ctx.waitFor`'s own pending
/// marker needs sub-second precision; [`adi_config::now_unix`] is too coarse) and [`readiness`]
/// (comparing a cached deadline against the wall clock needs the same clock both were stamped
/// with).
pub(crate) fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::SystemTime::UNIX_EPOCH)
        .map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

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

/// Whether every key in `where_clause` equals the same field in `payload` — `None` (no `where`
/// given) always matches. Shared by the event starter ([`starters::tick`], matching a workflow's
/// `on: event(pattern, where?)`) and a running attempt ([`attempt::Attempt`]'s `ctx.waitFor`),
/// since both are the same question against the same shape of clause.
pub(crate) fn where_matches(
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
    /// Shared (not per-clone) so every clone of this engine — in particular the dedicated
    /// workflow-attempt worker a host process runs [`run_tick_bounded`](Self::run_tick_bounded)
    /// on, off its own thread — agrees on what's already been tried. See [`readiness`] for why
    /// this is a cache, never load-bearing for correctness.
    readiness: Arc<readiness::Readiness>,
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
            readiness: Arc::new(readiness::Readiness::new()),
            config,
        }
    }

    /// Same as [`with_config`](Self::with_config), but with the run-readiness cache's safety
    /// interval overridden — for a test that wants to prove the safety interval actually fires
    /// without sitting through a real 5 minutes.
    #[must_use]
    #[cfg(test)]
    pub(crate) fn with_config_and_safety_interval(config: Config, safety_interval: std::time::Duration) -> Self {
        Self {
            runs: WorkflowStore::with_config(config.clone()),
            schedules: ScheduleStore::with_config(config.clone()),
            tasks: Tasks::with_config(config.clone()),
            events: Events::with_config(config.clone()),
            readiness: Arc::new(readiness::Readiness::with_safety_interval(safety_interval)),
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
        starters::tick(&self.events, &self.config, &self.runs, &pourer)
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

    fn attempt(&self) -> attempt::Attempt {
        attempt::Attempt::new(
            self.config.clone(),
            self.runs.clone(),
            self.tasks.clone(),
            self.events.clone(),
        )
    }

    /// Attempt every unfinished run that is actually *due* — a `bun` attempt only for a run
    /// [`readiness::Readiness::due`] says has something new to look at since its last one (or has
    /// never been attempted, or has hit its own deadline, or is overdue for the safety interval).
    /// One at a time, in [`list_unfinished`](store::WorkflowStore::list_unfinished)'s own oldest-
    /// first order; [`run_tick_bounded`](Self::run_tick_bounded) is the same sweep with several
    /// attempts in flight at once, for a caller (a dedicated worker thread, never the shared
    /// 1-second tick loop dispatch/goals/awaits also run on) that wants that.
    #[must_use]
    pub fn run_tick(&self) -> Vec<RanAttempt> {
        self.run_tick_bounded(1)
    }

    /// The same sweep as [`run_tick`](Self::run_tick), attempting up to `max_concurrency` due runs
    /// at once — what lets one run waiting on a slow person task or a many-hour sleep never cost
    /// every *other* waiting run its own chance at an attempt this sweep. `max_concurrency` of `1`
    /// (what [`run_tick`](Self::run_tick) uses) never spawns a worker thread at all, so the common
    /// case — a handful of runs, most of them not due — pays no threading cost over the old
    /// always-attempt sweep for whichever runs genuinely are due.
    #[must_use]
    pub fn run_tick_bounded(&self, max_concurrency: usize) -> Vec<RanAttempt> {
        let unfinished = match self.runs.list_unfinished() {
            Ok(runs) => runs,
            Err(e) => {
                tracing::warn!(error = %e, "couldn't list unfinished workflow runs");
                return Vec::new();
            }
        };
        let due = self.readiness.due(&self.tasks, &self.runs, &self.events, &unfinished);
        if due.is_empty() {
            return Vec::new();
        }
        let max_concurrency = max_concurrency.clamp(1, due.len());
        if max_concurrency == 1 {
            return due.iter().map(|run| self.attempt_and_record(run)).collect();
        }

        let queue = std::sync::Mutex::new(due.into_iter());
        let results = std::sync::Mutex::new(Vec::new());
        std::thread::scope(|scope| {
            for _ in 0..max_concurrency {
                scope.spawn(|| {
                    loop {
                        let next = queue.lock().unwrap_or_else(std::sync::PoisonError::into_inner).next();
                        let Some(run) = next else { break };
                        let ran = self.attempt_and_record(&run);
                        results
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner)
                            .push(ran);
                    }
                });
            }
        });
        results.into_inner().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// One attempt, with its readiness cache entry refreshed (or cleared, once the run is
    /// terminal) immediately after — see [`readiness::Readiness::record_attempt`].
    fn attempt_and_record(&self, run: &RunRow) -> RanAttempt {
        let outcome = self.attempt().run(run);
        let finished = !matches!(outcome, AttemptOutcome::Suspended);
        self.readiness
            .record_attempt(&self.tasks, &self.runs, &self.events, run, finished);
        RanAttempt {
            run_key: run.run_key.clone(),
            workflow: run.workflow.clone(),
            outcome,
        }
    }

    /// Start workflow `workflow` manually, keyed `wf@manual:<requestId>` — spec section E's run-
    /// key union, and the engine API `adi-mono workflows run` (WF-6) is the one caller of. The
    /// same request id always answers with the same run (claiming it is this call's only job;
    /// [`run_tick`](Self::run_tick) does the actual driving next tick), so retrying an uncertain
    /// `run` command is always safe.
    ///
    /// # Errors
    /// [`Error::Store`] if `workflow` names no file this engine currently discovers.
    pub fn start_manual(
        &self,
        workflow: &str,
        input: &serde_json::Value,
        request_id: &str,
    ) -> Result<StartedRun> {
        let run_key = format!("{workflow}@manual:{request_id}");
        if let Some(existing) = self.runs.get_run(&run_key)? {
            return Ok(StartedRun {
                run_key: existing.run_key,
                workflow: existing.workflow,
            });
        }
        let files = discover::workflow_files(&self.config);
        if !files.iter().any(|f| f.id == workflow) {
            return Err(Error::Store(anyhow::anyhow!("no such workflow: {workflow:?}")));
        }
        self.pourer().pour_for_workflow(
            &run_key,
            workflow,
            input,
            &serde_json::json!({"kind": "manual", "request_id": request_id}),
            None,
        );
        let row = self
            .runs
            .get_run(&run_key)?
            .ok_or_else(|| Error::Store(anyhow::anyhow!("manual pour of {workflow:?} didn't land")))?;
        Ok(StartedRun {
            run_key: row.run_key,
            workflow: row.workflow,
        })
    }

    /// Discard `run_key`'s journal entries after the first `keep`, deleting whatever task or
    /// child run each discarded step created — a redo on the next [`run_tick`](Self::run_tick)
    /// genuinely redoes the work, rather than silently replaying a task that already finished
    /// under the same stepId. See [`rewind`] for exactly what is (and isn't) cleaned up.
    ///
    /// # Errors
    /// [`Error::Store`] on a read or write failure.
    pub fn rewind(&self, run_key: &str, keep: usize) -> Result<()> {
        rewind::rewind(&self.runs, &self.tasks, run_key, keep)
    }
}

/// One run [`Engine::run_tick`] attempted, and what became of it.
#[derive(Debug, Clone, PartialEq)]
pub struct RanAttempt {
    pub run_key: String,
    pub workflow: String,
    pub outcome: AttemptOutcome,
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
