//! Driving one run through one attempt: spawn `bun run run.ts` once (not detached — this waits
//! on it), relay its `ctx` checkpoints to the stores that answer them, and finalize the run's
//! root task once the attempt ends. Every real decision — whether a step is cached, whether a
//! task is done, whether a timeout has elapsed, whether to pour a child run — is made here, in
//! Rust; the `bun` process only interprets the workflow author's own control flow and relays each
//! `await` as one line of JSON. See [`crate::rpc`] for the wire shapes and `@adi/workflows`'
//! `createCtx` for the TS side of the same contract.
//!
//! [`Engine::run_tick`](crate::Engine::run_tick) is the only caller: "attempt every unfinished
//! run, every tick" — the same level-triggered sweep `adi_schedules::tick` already is. A run that
//! suspends is simply tried again next tick; nothing here decides *when* to re-attempt a run,
//! only *what happens* during one attempt.

use std::io::{BufRead, BufReader, Write as _};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{Value, json};

use adi_config::Config;
use adi_events::Events;
use adi_tasks::{NewTask, Tasks, TaskStatus};

use crate::pour::WorkflowRunPourer;
use crate::rpc::Request;
use crate::store::{RunRow, StepKind, StepOutcome, StepRecord, WorkflowStore};

/// How long one attempt (one `bun` process, start to its terminal message) may run before this
/// crate kills it — generous, since a `ctx.step`'s own `fn()` may do real work (an API call), but
/// bounded, so one hung or infinite-looping workflow body can never wedge a tick forever. A
/// workflow that genuinely needs longer than this for one checkpoint belongs in a background
/// trigger or a real task an agent works on in its own time, not a `ctx.step` closure — the same
/// steer `adi-triggers`' own supervised-vs-fired split gives.
const ATTEMPT_TIMEOUT: Duration = Duration::from_secs(300);

/// How this attempt ended.
#[derive(Debug, Clone, PartialEq)]
pub enum Outcome {
    /// The workflow body returned, and its output (if it declared one) validated.
    Completed(Value),
    /// An uncaught error — a bad `input`, a reused step key, a plain exception, a `ctx.task`
    /// output that failed its own zod check, a step value too large for the journal, …
    Failed(String),
    /// A `ctx.task`/`ctx.run`/`ctx.ask` step's `TaskGaveUp` reached the top of the workflow body
    /// uncaught.
    GaveUp(String),
    /// A checkpoint has nothing resolved yet — nothing wrong; try again next tick.
    Suspended,
}

/// Drives one run's attempts: spawns the harness, answers its checkpoints, finalizes the root
/// task. Cheap to build fresh per call — see [`crate::Engine`]'s own `pourer()` for the same
/// pattern this mirrors.
pub struct Attempt {
    config: Config,
    store: WorkflowStore,
    tasks: Tasks,
    events: Events,
}

impl Attempt {
    #[must_use]
    pub fn new(config: Config, store: WorkflowStore, tasks: Tasks, events: Events) -> Self {
        Self {
            config,
            store,
            tasks,
            events,
        }
    }

    fn pourer(&self) -> WorkflowRunPourer {
        WorkflowRunPourer::new(
            self.config.clone(),
            self.store.clone(),
            self.tasks.clone(),
            self.events.clone(),
        )
    }

    /// Run one attempt of `run` to its conclusion and finalize the root task accordingly:
    /// complete it on [`Outcome::Completed`], give it up on [`Outcome::Failed`]/
    /// [`Outcome::GaveUp`] (and emit `adi.workflows.run.completed`/`failed` either way), or leave
    /// everything exactly as it was on [`Outcome::Suspended`] — there is nothing to finalize, and
    /// no event to emit, for "nothing happened this attempt".
    pub fn run(&self, run: &RunRow) -> Outcome {
        let outcome = self.attempt_once(run);
        self.finalize(run, &outcome);
        outcome
    }

    /// The pure mechanics of one attempt — spawn, relay, read the terminal message — with no
    /// side effect on the run's root task (that's [`finalize`](Self::finalize)'s job, so a test
    /// can inspect the raw [`Outcome`] without needing a real task tree underneath it).
    fn attempt_once(&self, run: &RunRow) -> Outcome {
        let pinned = match crate::pin::require_pinned(&self.config, &run.file_hash) {
            Ok(path) => path,
            Err(e) => return Outcome::Failed(format!("couldn't find the pinned workflow source: {e}")),
        };
        let run_dir = match self.ensure_run_dir(&run.run_key) {
            Ok(dir) => dir,
            Err(e) => return Outcome::Failed(format!("couldn't create the run's own directory: {e}")),
        };
        let slot = run
            .cause
            .get("kind")
            .and_then(Value::as_str)
            .filter(|k| *k == "schedule")
            .and_then(|_| run.cause.get("slot"))
            .and_then(Value::as_str)
            .unwrap_or("");

        let script = crate::describe::run_script_path(&self.config);
        let mut child = match Command::new("bun")
            .arg("run")
            .arg(&script)
            .arg(&pinned)
            .arg(run.input.to_string())
            .env("PATH", adi_config::augmented_path())
            .env("ADI_RUN_ID", &run.run_key)
            .env("ADI_RUN_DIR", run_dir.display().to_string())
            .env("ADI_SLOT", slot)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
        {
            Ok(child) => child,
            Err(e) => return Outcome::Failed(format!("couldn't spawn the run harness: {e}")),
        };

        let mut stdin = child.stdin.take().expect("piped stdin");
        let mut stdout = BufReader::new(child.stdout.take().expect("piped stdout"));
        let mut stderr = child.stderr.take().expect("piped stderr");

        // A watchdog that kills the process if one attempt runs unreasonably long — see
        // ATTEMPT_TIMEOUT's own doc for why this is generous rather than tight. Harmless to race
        // against a process that already exited: `kill` on one that's gone just errors, ignored.
        let child = Arc::new(Mutex::new(child));
        let watchdog = Arc::clone(&child);
        std::thread::spawn(move || {
            std::thread::sleep(ATTEMPT_TIMEOUT);
            let _ = watchdog.lock().unwrap_or_else(std::sync::PoisonError::into_inner).kill();
        });

        let outcome = loop {
            let mut line = String::new();
            match stdout.read_line(&mut line) {
                Ok(0) => {
                    use std::io::Read as _;
                    let mut err_text = String::new();
                    let _ = stderr.read_to_string(&mut err_text);
                    break Outcome::Failed(format!(
                        "the run harness closed its output before a terminal message (stderr: {})",
                        err_text.trim()
                    ));
                }
                Ok(_) => {}
                Err(e) => break Outcome::Failed(format!("couldn't read the run harness: {e}")),
            }
            let request: Request = match serde_json::from_str(line.trim()) {
                Ok(r) => r,
                Err(e) => {
                    break Outcome::Failed(format!(
                        "couldn't parse a line from the run harness: {e} (line: {})",
                        line.trim()
                    ));
                }
            };
            if request.is_terminal() {
                break match request {
                    Request::Result { value } => Outcome::Completed(value),
                    Request::Error { message } => Outcome::Failed(message),
                    Request::GaveUp { reason } => Outcome::GaveUp(reason),
                    Request::Suspend {} => Outcome::Suspended,
                    _ => unreachable!("is_terminal() only admits these four variants"),
                };
            }
            let reply = match self.dispatch(run, &request) {
                Ok(reply) => reply,
                Err(reason) => break Outcome::Failed(reason),
            };
            if let Err(e) = writeln!(stdin, "{reply}") {
                break Outcome::Failed(format!("couldn't write a reply to the run harness: {e}"));
            }
            if stdin.flush().is_err() {
                break Outcome::Failed("couldn't flush a reply to the run harness".to_string());
            }
        };

        let _ = child.lock().unwrap_or_else(std::sync::PoisonError::into_inner).wait();
        outcome
    }

    /// Compute the host's reply to one checkpoint request — the whole dispatch table.
    fn dispatch(&self, run: &RunRow, request: &Request) -> Result<Value, String> {
        match request {
            Request::Step { key } => self.on_step(run, key),
            Request::StepSave { key, value } => self.on_step_save(run, key, value),
            Request::Task {
                key,
                title,
                agent,
                person,
                instructions,
                input,
                output_schema,
                timeout_ms,
                project,
                cwd,
            } => self.on_task_like(
                run,
                key,
                StepKind::Task,
                NewTask {
                    title: title.clone(),
                    details: instructions.clone(),
                    project: project.clone(),
                    cwd: cwd.clone(),
                    assignee: agent.clone().or_else(|| person.clone()),
                    input: input.clone(),
                    output_schema: output_schema.clone(),
                    workflow_run: Some(run.run_key.clone()),
                    step_key: Some(key.clone()),
                    ..NewTask::default()
                },
                *timeout_ms,
            ),
            Request::Ask { key, person, text } => self.on_task_like(
                run,
                key,
                StepKind::Ask,
                NewTask {
                    title: text.chars().take(80).collect(),
                    details: Some(text.clone()),
                    assignee: Some(person.clone()),
                    workflow_run: Some(run.run_key.clone()),
                    step_key: Some(key.clone()),
                    ..NewTask::default()
                },
                None,
            ),
            Request::Run { key, workflow, input } => self.on_run(run, key, workflow, input.clone()),
            Request::WaitFor {
                key,
                event,
                where_clause,
                timeout_ms,
            } => self.on_wait_for(run, key, event, where_clause.as_ref(), *timeout_ms),
            Request::Sleep { key, until } => self.on_sleep(run, key, *until),
            Request::Result { .. } | Request::Error { .. } | Request::GaveUp { .. } | Request::Suspend {} => {
                unreachable!("terminal requests never reach dispatch")
            }
        }
    }

    fn on_step(&self, run: &RunRow, key: &str) -> Result<Value, String> {
        match self.store.get_step(&run.run_key, key).map_err(|e| store_err(&e))? {
            Some(record) => match record.outcome {
                StepOutcome::Ok(value) => Ok(json!({"cached": true, "value": value})),
                StepOutcome::GaveUp(_) | StepOutcome::TimedOut => {
                    Err(format!("step {key:?} was journaled with no plain value"))
                }
            },
            None => Ok(json!({"cached": false})),
        }
    }

    fn on_step_save(&self, run: &RunRow, key: &str, value: &Value) -> Result<Value, String> {
        let record = StepRecord::new(key, StepKind::Step, StepOutcome::Ok(value.clone()));
        self.store.put_step(&run.run_key, &record).map_err(|e| store_err(&e))?;
        Ok(json!({"ok": true}))
    }

    /// `ctx.task`/`ctx.ask` share this: look up (or create) the underlying task by stepId, and
    /// answer `ok`/`pending`/`gave_up`/`timed_out` from its current state — exactly the shape
    /// [`crate::rpc`] documents.
    fn on_task_like(
        &self,
        run: &RunRow,
        key: &str,
        kind: StepKind,
        new_task: NewTask,
        timeout_ms: Option<u64>,
    ) -> Result<Value, String> {
        if let Some(record) = self.store.get_step(&run.run_key, key).map_err(|e| store_err(&e))? {
            return Ok(outcome_to_reply(&record.outcome));
        }
        let existing = self
            .tasks
            .find_step(&run.run_key, key)
            .map_err(|e| format!("couldn't look up a step's task: {e}"))?;
        let task = if let Some(task) = existing {
            task
        } else {
            let created = self
                .tasks
                .create_full(new_task)
                .map_err(|e| format!("couldn't create a step's task: {e}"))?;
            // `Task::created_at` is second-granularity — too coarse for a millisecond
            // `timeout` on anything shorter than a couple of seconds. Stamp the real
            // creation moment at millisecond precision in the same pending-marker table
            // `ctx.waitFor` already uses for its own "when did I start" memory;
            // `resolve_task_step` prefers this over `created_at` whenever it's there.
            // `timeout_ms` rides along too (even when `None`) — `crate::readiness` reads it back
            // to know this step's own deadline without re-spawning `bun` to re-ask for it.
            self.store
                .put_pending(
                    &run.run_key,
                    key,
                    &json!({"started_at_ms": crate::now_ms(), "timeout_ms": timeout_ms}),
                )
                .map_err(|e| store_err(&e))?;
            created
        };
        self.resolve_task_step(run, key, kind, &task, timeout_ms)
    }

    fn on_run(&self, run: &RunRow, key: &str, workflow: &str, input: Option<Value>) -> Result<Value, String> {
        if let Some(record) = self.store.get_step(&run.run_key, key).map_err(|e| store_err(&e))? {
            return Ok(outcome_to_reply(&record.outcome));
        }
        let child_run_key = format!("{workflow}@{}/{key}", run.run_key);
        let child = if let Some(row) = self.store.get_run(&child_run_key).map_err(|e| store_err(&e))? {
            row
        } else {
            self.pourer().pour_for_workflow(
                &child_run_key,
                workflow,
                &input.unwrap_or_else(|| Value::Object(serde_json::Map::new())),
                &json!({"kind": "child", "parent": run.run_key, "key": key}),
                Some(&run.run_key),
            );
            self.store
                .get_run(&child_run_key)
                .map_err(|e| store_err(&e))?
                .ok_or_else(|| format!("couldn't start child workflow {workflow:?} — does it exist?"))?
        };
        let Some(task_id) = child.root_task.clone() else {
            return Ok(json!({"state": "pending"}));
        };
        let task = self
            .tasks
            .get(&task_id)
            .map_err(|e| format!("couldn't read a child run's root task: {e}"))?;
        let reply = self.resolve_task_step(run, key, StepKind::Run, &task, None)?;
        if let Value::Object(_) = &reply
            && reply.get("state").and_then(Value::as_str) != Some("pending")
        {
            // Record the child run key too, so `crate::rewind` can delete it alongside the task
            // if this step is later discarded. `resolve_task_step` already journaled the step
            // with `task_id` set; re-journal here with `ref_key` added — cheap (one row, same
            // key, `put_step` always upserts-by-insert-once) and simpler than threading `ref_key`
            // all the way through a function `on_task_like` also calls.
            if let Some(record) = self.store.get_step(&run.run_key, key).map_err(|e| store_err(&e))? {
                let mut record = record;
                record.ref_key = Some(child_run_key.clone());
                self.restamp_step(&run.run_key, &record)?;
            }
        }
        Ok(reply)
    }

    /// Shared tail of `on_task_like`/`on_run`: given a task already in hand, answer from its
    /// current state, journaling a terminal resolution the moment there is one.
    fn resolve_task_step(
        &self,
        run: &RunRow,
        key: &str,
        kind: StepKind,
        task: &adi_tasks::TaskView,
        timeout_ms: Option<u64>,
    ) -> Result<Value, String> {
        if let Some(reason) = &task.task.give_up_reason {
            let record = StepRecord::new(key, kind, StepOutcome::GaveUp(reason.clone()))
                .with_task(task.task.id.clone());
            self.store.put_step(&run.run_key, &record).map_err(|e| store_err(&e))?;
            return Ok(json!({"state": "gave_up", "reason": reason}));
        }
        if task.task.status == TaskStatus::Done {
            let value = task.task.result.clone().unwrap_or(Value::Null);
            let record = StepRecord::new(key, kind, StepOutcome::Ok(value.clone()))
                .with_task(task.task.id.clone());
            self.store.put_step(&run.run_key, &record).map_err(|e| store_err(&e))?;
            return Ok(json!({"state": "ok", "value": value}));
        }
        if let Some(timeout_ms) = timeout_ms {
            let started_at_ms = self
                .store
                .get_pending(&run.run_key, key)
                .map_err(|e| store_err(&e))?
                .and_then(|state| state.get("started_at_ms").and_then(Value::as_u64))
                .unwrap_or_else(|| task.task.created_at.saturating_mul(1000));
            let age_ms = crate::now_ms().saturating_sub(started_at_ms);
            if age_ms >= timeout_ms {
                let record =
                    StepRecord::new(key, kind, StepOutcome::TimedOut).with_task(task.task.id.clone());
                self.store.put_step(&run.run_key, &record).map_err(|e| store_err(&e))?;
                return Ok(json!({"state": "timed_out"}));
            }
        }
        Ok(json!({"state": "pending"}))
    }

    fn on_wait_for(
        &self,
        run: &RunRow,
        key: &str,
        event: &str,
        where_clause: Option<&serde_json::Map<String, Value>>,
        timeout_ms: Option<u64>,
    ) -> Result<Value, String> {
        if let Some(record) = self.store.get_step(&run.run_key, key).map_err(|e| store_err(&e))? {
            return Ok(outcome_to_reply(&record.outcome));
        }
        let now = crate::now_ms();
        let (since_seq, started_at_ms) = if let Some(state) =
            self.store.get_pending(&run.run_key, key).map_err(|e| store_err(&e))?
        {
            (
                state.get("since_seq").and_then(Value::as_u64).unwrap_or(0),
                state.get("started_at_ms").and_then(Value::as_u64).unwrap_or(now),
            )
        } else {
            let head = self.events.head().map_err(|e| format!("couldn't read the event log: {e}"))?;
            // `timeout_ms` rides along here too, same reason as `on_task_like`'s own pending
            // write — `crate::readiness` reads it back without spawning `bun`.
            self.store
                .put_pending(
                    &run.run_key,
                    key,
                    &json!({"since_seq": head, "started_at_ms": now, "timeout_ms": timeout_ms}),
                )
                .map_err(|e| store_err(&e))?;
            (head, now)
        };
        let batch = self
            .events
            .after(since_seq, 1000)
            .map_err(|e| format!("couldn't read the event log: {e}"))?;
        let matched = batch.into_iter().find(|logged| {
            adi_events::matches(event, &logged.record.name) && {
                let payload: Value = serde_json::from_str(&logged.record.payload).unwrap_or(Value::Null);
                crate::where_matches(where_clause, &payload)
            }
        });
        if let Some(logged) = matched {
            let payload: Value = serde_json::from_str(&logged.record.payload).unwrap_or(Value::Null);
            let record = StepRecord::new(key, StepKind::Wait, StepOutcome::Ok(payload.clone()));
            self.store.put_step(&run.run_key, &record).map_err(|e| store_err(&e))?;
            return Ok(json!({"state": "ok", "value": payload}));
        }
        if let Some(timeout_ms) = timeout_ms {
            let age_ms = now.saturating_sub(started_at_ms);
            if age_ms >= timeout_ms {
                let record = StepRecord::new(key, StepKind::Wait, StepOutcome::TimedOut);
                self.store.put_step(&run.run_key, &record).map_err(|e| store_err(&e))?;
                return Ok(json!({"state": "timed_out"}));
            }
        }
        Ok(json!({"state": "pending"}))
    }

    fn on_sleep(&self, run: &RunRow, key: &str, until_ms: i64) -> Result<Value, String> {
        if self.store.get_step(&run.run_key, key).map_err(|e| store_err(&e))?.is_some() {
            return Ok(json!({"state": "ok"}));
        }
        let now = i64::try_from(crate::now_ms()).unwrap_or(i64::MAX);
        if now >= until_ms {
            let record = StepRecord::new(key, StepKind::Sleep, StepOutcome::Ok(Value::Null));
            self.store.put_step(&run.run_key, &record).map_err(|e| store_err(&e))?;
            return Ok(json!({"state": "ok"}));
        }
        // Persisted host-side so `crate::readiness` can tell when this deadline passes without
        // spawning `bun` again just to re-ask the workflow body what `until` was.
        self.store
            .put_pending(&run.run_key, key, &json!({"until_ms": until_ms}))
            .map_err(|e| store_err(&e))?;
        Ok(json!({"state": "pending"}))
    }

    /// Replace an already-journaled record in place (same `run_key`/`step_key`) — the one case
    /// that legitimately rewrites a journal row rather than inserting a fresh one: adding
    /// `ref_key` to a `ctx.run` step just after [`resolve_task_step`](Self::resolve_task_step)
    /// journaled it without one.
    fn restamp_step(&self, run_key: &str, record: &StepRecord) -> Result<(), String> {
        self.store.restamp_step(run_key, record).map_err(|e| store_err(&e))
    }

    /// `<workflows-runs>/attempts/<sha256 of run_key>` — a run key can carry `@`, `:`, `/`, none
    /// of them safe as a bare path component, so the directory name is content-addressed on the
    /// key instead of derived from it textually.
    fn ensure_run_dir(&self, run_key: &str) -> std::io::Result<std::path::PathBuf> {
        use sha2::{Digest, Sha256};
        let dir = self
            .store
            .dir()
            .join("attempts")
            .join(hex::encode(Sha256::digest(run_key.as_bytes())));
        std::fs::create_dir_all(&dir)?;
        Ok(dir)
    }

    /// Complete or give up the run's root task and emit `adi.workflows.run.completed`/`failed` —
    /// everything [`Outcome::Suspended`] deliberately skips, since nothing happened this attempt.
    fn finalize(&self, run: &RunRow, outcome: &Outcome) {
        let Some(root_task) = run.root_task.clone() else {
            return;
        };
        match outcome {
            Outcome::Suspended => {}
            Outcome::Completed(value) => {
                if let Err(e) = self.tasks.complete(&root_task, Some(value.clone())) {
                    tracing::warn!(run_key = %run.run_key, error = %e, "a workflow returned, but its root task refused the result; failing the run instead");
                    self.fail(run, &root_task, &format!("the workflow's result didn't fit its own output shape: {e}"));
                    return;
                }
                let _ = self.store.mark_finished(&run.run_key);
                let _ = self.events.emit_event(&crate::event::WorkflowRunCompleted {
                    run_key: run.run_key.clone(),
                    workflow: run.workflow.clone(),
                    root_task,
                    result: value.clone(),
                    caused_by_run: run.run_key.clone(),
                });
            }
            Outcome::Failed(reason) => self.fail(run, &root_task, reason),
            Outcome::GaveUp(reason) => {
                self.fail(run, &root_task, &format!("task given up on: {reason}"));
            }
        }
    }

    fn fail(&self, run: &RunRow, root_task: &str, reason: &str) {
        let _ = self.tasks.give_up(root_task, reason);
        let _ = self.store.mark_finished(&run.run_key);
        let _ = self.events.emit_event(&crate::event::WorkflowRunFailed {
            run_key: run.run_key.clone(),
            workflow: run.workflow.clone(),
            root_task: root_task.to_string(),
            reason: reason.to_string(),
            caused_by_run: run.run_key.clone(),
        });
    }
}

/// The journaled resolution of a `ctx.task`/`ctx.run`/`ctx.ask`/`ctx.waitFor` step, replayed as
/// the exact reply shape [`crate::rpc`] documents — a cache hit never needs to touch the task or
/// event stores at all.
fn outcome_to_reply(outcome: &StepOutcome) -> Value {
    match outcome {
        StepOutcome::Ok(value) => json!({"state": "ok", "value": value}),
        StepOutcome::GaveUp(reason) => json!({"state": "gave_up", "reason": reason}),
        StepOutcome::TimedOut => json!({"state": "timed_out"}),
    }
}

fn store_err(e: &crate::error::Error) -> String {
    format!("couldn't read or write the step journal: {e}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Engine;
    use adi_config::Config;
    use serde_json::json;

    fn scratch(tag: &str) -> Config {
        let root = std::env::temp_dir().join(format!(
            "adi-workflows-attempt-{tag}-{}-{:?}",
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

    /// Complete, with a trivial result, every still-open task under `run_key` whose title is in
    /// `titles` — the test's stand-in for "a person/agent finishes the step", called after every
    /// tick so a `ctx.task`/`ctx.ask` step gets to resolve on whichever later tick reaches it.
    fn complete_person_steps(tasks: &adi_tasks::Tasks, run_key: &str, titles: &[&str]) {
        let Ok(all) = tasks.list(None, None, Some(TaskStatus::Open), None) else {
            return;
        };
        for view in all {
            if view.task.workflow_run.as_deref() == Some(run_key) && titles.contains(&view.task.title.as_str()) {
                let _ = tasks.complete(&view.task.id, Some(json!({"ok": true})));
            }
        }
    }

    /// Drive every unfinished run (ticking the whole engine, not just `run_key` — a child run
    /// needs its own tick same as the parent) up to `max_ticks` times, completing any `review`/
    /// `ask1` person step as soon as it appears and re-emitting `event_name` on the log every
    /// tick (harmless to repeat — `ctx.waitFor` only needs *a* matching row after it started
    /// looking, and re-emitting is simpler than timing one emission to land in the single tick
    /// between "just started waiting" and "next attempt") — stopping early the moment `run_key`'s
    /// root task is no longer open.
    fn drive_to_done(
        engine: &Engine,
        tasks: &adi_tasks::Tasks,
        events: &Events,
        run_key: &str,
        event_name: &str,
        max_ticks: usize,
    ) {
        for _ in 0..max_ticks {
            // A real pause between simulated ticks: with the run-readiness cache (ADI-MONO-150),
            // a tick that finds nothing changed and no deadline due costs a few cheap reads, not
            // a `bun` spawn — so, unlike before this fix, nothing here reliably burns the real
            // wall-clock time this workflow's `ctx.sleep` needs to pass on its own. 40 * 20ms
            // comfortably clears the 300ms `sleepUntil` the "tour" workflow below sleeps until.
            std::thread::sleep(Duration::from_millis(20));
            let _ = engine.run_tick();
            complete_person_steps(tasks, run_key, &["review", "ask1"]);
            let _ = events.emit(event_name, "{}");
            if let Ok(Some(run)) = engine.runs().get_run(run_key)
                && let Some(root) = &run.root_task
                && let Ok(view) = tasks.get(root)
                && view.task.status != TaskStatus::Open
            {
                return;
            }
        }
    }

    #[test]
    fn a_plain_step_workflow_completes_in_one_tick() {
        let config = scratch("plain-step");
        let engine = Engine::with_config(config.clone());
        engine.bootstrap().expect("bootstrap");
        write_workflow(
            &config,
            "plain.ts",
            "import { manual, workflow } from \"@adi/workflows\";\n\
             export const on = manual();\n\
             export default workflow(async (ctx, input) => {\n\
             \x20 const a = await ctx.step({ key: \"a\", fn: () => 1 });\n\
             \x20 const b = await ctx.step({ key: \"b\", fn: () => 2 });\n\
             \x20 return { sum: a + b };\n\
             });\n",
        );

        let started = engine.start_manual("plain", &json!({}), "req-1").expect("start");
        let tasks = Tasks::with_config(config.clone());
        let ran = engine.run_tick();
        assert_eq!(ran.len(), 1);
        assert_eq!(ran[0].run_key, started.run_key);
        assert_eq!(ran[0].outcome, Outcome::Completed(json!({"sum": 3})));

        let root = engine
            .runs()
            .get_run(&started.run_key)
            .expect("read")
            .expect("exists")
            .root_task
            .expect("root task");
        let view = tasks.get(&root).expect("task");
        assert_eq!(view.task.status, TaskStatus::Done);
        assert_eq!(view.task.result, Some(json!({"sum": 3})));
    }

    /// The whole tour: every block type (`step`/`task`/`waitFor`/`sleep`/`ask`/`run`), each
    /// suspending at least once and resumed by a *fresh* attempt (every `run_tick` spawns a brand
    /// new `bun` process — there is no other kind of "kill and replay" to simulate here, since
    /// that fresh-process-per-attempt *is* the architecture). Asserts the one thing that matters
    /// about replay: `ctx.step`'s `fn()` — a real side effect, a file write — runs exactly once,
    /// no matter how many attempts it takes the rest of the workflow to finish.
    #[test]
    fn every_block_type_resumes_correctly_across_several_fresh_attempts() {
        let config = scratch("tour");
        let engine = Engine::with_config(config.clone());
        engine.bootstrap().expect("bootstrap");
        write_workflow(
            &config,
            "child-tour.ts",
            "import { workflow } from \"@adi/workflows\";\n\
             export const on = [];\n\
             export default workflow(async (ctx, input) => {\n\
             \x20 return { child: true, x: input.x };\n\
             });\n",
        );
        write_workflow(
            &config,
            "tour.ts",
            "import { workflow } from \"@adi/workflows\";\n\
             import { appendFileSync } from \"node:fs\";\n\
             export const on = [];\n\
             export default workflow(async (ctx, input) => {\n\
             \x20 await ctx.step({ key: \"mark\", fn: () => { appendFileSync(`${ctx.dir}/marks`, \"x\"); return \"marked\"; } });\n\
             \x20 const taskResult = await ctx.task({ key: \"review\", title: \"review\", person: \"igor\" });\n\
             \x20 const waited = await ctx.waitFor({ key: \"wait\", event: \"tour.go\" });\n\
             \x20 await ctx.sleep({ key: \"nap\", until: input.sleepUntil });\n\
             \x20 const asked = await ctx.ask({ key: \"ask1\", person: \"igor\", text: \"ask1\" });\n\
             \x20 const child = await ctx.run({ key: \"child1\", workflow: \"child-tour\", input: { x: 1 } });\n\
             \x20 return { taskResult, waited, asked, child };\n\
             });\n",
        );

        let sleep_until = (std::time::SystemTime::now() + Duration::from_millis(300))
            .duration_since(std::time::SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_millis();
        let started = engine
            .start_manual("tour", &json!({"sleepUntil": sleep_until}), "req-1")
            .expect("start");

        let tasks = Tasks::with_config(config.clone());
        let events = Events::with_config(config.clone());
        drive_to_done(&engine, &tasks, &events, &started.run_key, "tour.go", 40);

        let run = engine.runs().get_run(&started.run_key).expect("read").expect("exists");
        let root = run.root_task.clone().expect("root task");
        let view = tasks.get(&root).expect("task");
        assert_eq!(
            view.task.status,
            TaskStatus::Done,
            "the tour must finish within the tick budget; task tree: {:#?}",
            tasks.list(None, None, None, None)
        );
        assert_eq!(
            view.task.result,
            Some(json!({
                "taskResult": {"ok": true},
                "waited": {},
                "asked": {"ok": true},
                "child": {"child": true, "x": 1},
            }))
        );

        // The step's side effect ran exactly once, no matter how many attempts resumed it.
        let marks = std::fs::read_to_string(
            ensure_run_dir_for_test(&config, &started.run_key).join("marks"),
        )
        .expect("marks file");
        assert_eq!(marks, "x");
    }

    /// A key reused within one execution pass fails the whole attempt, with a message the TS
    /// `ctx` library's own `claim()` wrote — never silently papered over by a cache hit, since a
    /// genuine replay never reaches the second use of the same key on the very same pass that
    /// also used it the first time.
    #[test]
    fn a_duplicate_step_key_within_one_attempt_fails_the_run() {
        let config = scratch("dup-key");
        let engine = Engine::with_config(config.clone());
        engine.bootstrap().expect("bootstrap");
        write_workflow(
            &config,
            "dup.ts",
            "import { manual, workflow } from \"@adi/workflows\";\n\
             export const on = manual();\n\
             export default workflow(async (ctx, input) => {\n\
             \x20 await ctx.step({ key: \"x\", fn: () => 1 });\n\
             \x20 await ctx.step({ key: \"x\", fn: () => 2 });\n\
             \x20 return {};\n\
             });\n",
        );

        let started = engine.start_manual("dup", &json!({}), "req-1").expect("start");
        let ran = engine.run_tick();
        assert_eq!(ran.len(), 1);
        match &ran[0].outcome {
            Outcome::Failed(message) => assert!(
                message.contains("duplicate step key"),
                "unexpected message: {message}"
            ),
            other => panic!("expected Failed, got {other:?}"),
        }

        let tasks = Tasks::with_config(config);
        let root = engine
            .runs()
            .get_run(&started.run_key)
            .unwrap()
            .unwrap()
            .root_task
            .unwrap();
        let view = tasks.get(&root).expect("task");
        assert_eq!(view.task.status, TaskStatus::Open, "give-up never changes stored status");
        assert!(view.task.give_up_reason.unwrap().contains("duplicate step key"));
    }

    /// A run stays pinned to the exact content it started with — editing the live file mid-run
    /// (between the attempt that suspends on a `ctx.task` and the one that resumes after it)
    /// must never change what a later attempt of the *same* run executes.
    #[test]
    fn a_run_stays_pinned_to_its_version_through_a_replay() {
        let config = scratch("pin-replay");
        let engine = Engine::with_config(config.clone());
        engine.bootstrap().expect("bootstrap");
        write_workflow(
            &config,
            "versioned.ts",
            "import { manual, workflow } from \"@adi/workflows\";\n\
             export const on = manual();\n\
             export default workflow(async (ctx, input) => {\n\
             \x20 await ctx.task({ key: \"gate\", title: \"review\", person: \"igor\" });\n\
             \x20 const v = await ctx.step({ key: \"v\", fn: () => \"v1\" });\n\
             \x20 return { v };\n\
             });\n",
        );

        let started = engine.start_manual("versioned", &json!({}), "req-1").expect("start");
        let tasks = Tasks::with_config(config.clone());
        // First attempt: creates the "review" task and suspends before ever reaching `v`.
        let first = engine.run_tick();
        assert_eq!(first[0].outcome, Outcome::Suspended);

        // Edit the live file after the run has started — a later attempt must still read "v1".
        write_workflow(
            &config,
            "versioned.ts",
            "import { manual, workflow } from \"@adi/workflows\";\n\
             export const on = manual();\n\
             export default workflow(async (ctx, input) => {\n\
             \x20 await ctx.task({ key: \"gate\", title: \"review\", person: \"igor\" });\n\
             \x20 const v = await ctx.step({ key: \"v\", fn: () => \"v2 -- edited after the run started\" });\n\
             \x20 return { v };\n\
             });\n",
        );

        complete_person_steps(&tasks, &started.run_key, &["review"]);
        let second = engine.run_tick();
        assert_eq!(second[0].outcome, Outcome::Completed(json!({"v": "v1"})));
    }

    /// `ctx.task`'s `timeout` resolves to `{ timed_out: true }` once it elapses, without ever
    /// validating that sentinel against the step's own `output` shape.
    #[test]
    fn a_task_timeout_resolves_to_timed_out() {
        let config = scratch("timeout");
        let engine = Engine::with_config(config.clone());
        engine.bootstrap().expect("bootstrap");
        write_workflow(
            &config,
            "slow.ts",
            "import { manual, workflow } from \"@adi/workflows\";\n\
             export const on = manual();\n\
             export default workflow(async (ctx, input) => {\n\
             \x20 const r = await ctx.task({ key: \"gate\", title: \"review\", person: \"igor\", timeout: 3000 });\n\
             \x20 return { r };\n\
             });\n",
        );

        // A generous timeout, well past anything a `bun` spawn plus `describe()`'s own (pour
        // already ran one to fetch the output shape) could plausibly cost even under load —
        // the point under test is the *timing logic*, not how fast this machine is right now.
        let started = engine.start_manual("slow", &json!({}), "req-1").expect("start");
        let first = engine.run_tick();
        assert_eq!(first[0].outcome, Outcome::Suspended, "too soon for the 3s timeout");

        std::thread::sleep(Duration::from_millis(3200));
        let second = engine.run_tick();
        assert_eq!(second[0].run_key, started.run_key);
        assert_eq!(second[0].outcome, Outcome::Completed(json!({"r": {"timed_out": true}})));
    }

    /// An uncaught `TaskGaveUp` fails the whole run: the root task is given up on (never
    /// completed — there is no result that would fit its own shape), and `adi.workflows.run.
    /// failed` is emitted.
    #[test]
    fn an_uncaught_give_up_fails_the_run_and_emits_run_failed() {
        let config = scratch("give-up");
        let engine = Engine::with_config(config.clone());
        engine.bootstrap().expect("bootstrap");
        write_workflow(
            &config,
            "gives-up.ts",
            "import { manual, workflow } from \"@adi/workflows\";\n\
             export const on = manual();\n\
             export default workflow(async (ctx, input) => {\n\
             \x20 await ctx.task({ key: \"gate\", title: \"review\", person: \"igor\" });\n\
             \x20 return {};\n\
             });\n",
        );

        let started = engine.start_manual("gives-up", &json!({}), "req-1").expect("start");
        let tasks = Tasks::with_config(config.clone());
        let events = Events::with_config(config);
        assert_eq!(engine.run_tick()[0].outcome, Outcome::Suspended);

        let review = tasks
            .list(None, None, Some(TaskStatus::Open), None)
            .unwrap()
            .into_iter()
            .find(|t| t.task.title == "review")
            .expect("review task exists");
        tasks.give_up(&review.task.id, "no API key").expect("give up");

        let second = engine.run_tick();
        match &second[0].outcome {
            Outcome::GaveUp(reason) => assert_eq!(reason, "no API key"),
            other => panic!("expected GaveUp, got {other:?}"),
        }

        let root = engine
            .runs()
            .get_run(&started.run_key)
            .unwrap()
            .unwrap()
            .root_task
            .unwrap();
        let view = tasks.get(&root).expect("task");
        assert_eq!(view.task.status, TaskStatus::Open);
        assert!(view.task.give_up_reason.unwrap().contains("no API key"));

        let caught = events.catch_up("test-giveup", 50).expect("catch up");
        assert!(
            caught.iter().any(|e| e.record.name == "adi.workflows.run.failed"),
            "{caught:?}"
        );
    }

    /// "Rewind to step N, discards journal entries from N onward" — and, since those steps are
    /// what created the task this redo's `ctx.task` would otherwise just find already resolved,
    /// the rewound run genuinely redoes the work rather than replaying the same old answer.
    #[test]
    fn rewind_causes_a_genuine_redo_not_a_replay_of_the_stale_answer() {
        let config = scratch("rewind");
        let engine = Engine::with_config(config.clone());
        engine.bootstrap().expect("bootstrap");
        write_workflow(
            &config,
            "redo.ts",
            "import { manual, workflow } from \"@adi/workflows\";\n\
             export const on = manual();\n\
             export default workflow(async (ctx, input) => {\n\
             \x20 const r = await ctx.task({ key: \"gate\", title: \"review\", person: \"igor\" });\n\
             \x20 return { r };\n\
             });\n",
        );

        let started = engine.start_manual("redo", &json!({}), "req-1").expect("start");
        let tasks = Tasks::with_config(config.clone());
        assert_eq!(engine.run_tick()[0].outcome, Outcome::Suspended);

        let first_review = tasks
            .list(None, None, Some(TaskStatus::Open), None)
            .unwrap()
            .into_iter()
            .find(|t| t.task.title == "review")
            .expect("review task");
        tasks
            .complete(&first_review.task.id, Some(json!({"answer": "first"})))
            .expect("complete first");
        assert_eq!(
            engine.run_tick()[0].outcome,
            Outcome::Completed(json!({"r": {"answer": "first"}}))
        );

        // Rewind past the "gate" step entirely (keep 0) and run again.
        engine.rewind(&started.run_key, 0).expect("rewind");
        assert!(
            tasks.get(&first_review.task.id).is_err(),
            "the first review task must be gone so its stepId is free"
        );

        let resumed = engine.run_tick();
        assert_eq!(resumed[0].outcome, Outcome::Suspended, "the redo creates a brand-new task");
        let second_review = tasks
            .list(None, None, Some(TaskStatus::Open), None)
            .unwrap()
            .into_iter()
            .find(|t| t.task.title == "review")
            .expect("a fresh review task");
        assert_ne!(second_review.task.id, first_review.task.id);
        tasks
            .complete(&second_review.task.id, Some(json!({"answer": "second"})))
            .expect("complete second");
        assert_eq!(
            engine.run_tick()[0].outcome,
            Outcome::Completed(json!({"r": {"answer": "second"}})),
            "the redo answers with the NEW completion, not the stale first one"
        );
    }

    /// ADI-MONO-150: a run waiting on a person task must not spawn `bun` on every tick — only the
    /// first (nothing cached yet) and the one the person's completion actually wakes.
    #[test]
    fn a_waiting_run_spawns_bun_only_on_a_real_change() {
        let config = scratch("readiness-person-task");
        let engine = Engine::with_config(config.clone());
        engine.bootstrap().expect("bootstrap");
        write_workflow(
            &config,
            "gate.ts",
            "import { manual, workflow } from \"@adi/workflows\";\n\
             export const on = manual();\n\
             export default workflow(async (ctx, input) => {\n\
             \x20 const r = await ctx.task({ key: \"gate\", title: \"review\", person: \"igor\" });\n\
             \x20 return { r };\n\
             });\n",
        );
        let started = engine.start_manual("gate", &json!({}), "req-1").expect("start");
        let tasks = Tasks::with_config(config.clone());

        let first = engine.run_tick();
        assert_eq!(first.len(), 1, "never attempted before — the first tick always tries");
        assert_eq!(first[0].outcome, Outcome::Suspended);

        let mut extra_spawns = 0;
        for _ in 0..30 {
            extra_spawns += engine.run_tick().len();
        }
        assert_eq!(
            extra_spawns, 0,
            "nothing changed across 30 simulated ticks — no bun spawn should have happened"
        );

        let review = tasks
            .list(None, None, Some(TaskStatus::Open), None)
            .unwrap()
            .into_iter()
            .find(|t| t.task.title == "review")
            .expect("review task");
        tasks
            .complete(&review.task.id, Some(json!({"ok": true})))
            .expect("complete");

        let woke = engine.run_tick();
        assert_eq!(woke.len(), 1, "the task's own completion must wake the run within one tick");
        assert_eq!(woke[0].outcome, Outcome::Completed(json!({"r": {"ok": true}})));
        let _ = started;
    }

    /// ADI-MONO-150: even with nothing changed, a run is re-attempted once the safety interval
    /// has passed — the fallback for a signal the readiness cache doesn't model.
    #[test]
    fn the_safety_interval_forces_a_reattempt_even_with_nothing_changed() {
        let config = scratch("readiness-safety");
        let safety_interval = Duration::from_millis(120);
        let engine = Engine::with_config_and_safety_interval(config.clone(), safety_interval);
        engine.bootstrap().expect("bootstrap");
        write_workflow(
            &config,
            "stuck.ts",
            "import { manual, workflow } from \"@adi/workflows\";\n\
             export const on = manual();\n\
             export default workflow(async (ctx, input) => {\n\
             \x20 const r = await ctx.task({ key: \"gate\", title: \"review\", person: \"igor\" });\n\
             \x20 return { r };\n\
             });\n",
        );
        let started = engine.start_manual("stuck", &json!({}), "req-1").expect("start");

        let first = engine.run_tick();
        assert_eq!(first.len(), 1);
        assert_eq!(first[0].outcome, Outcome::Suspended);

        assert!(
            engine.run_tick().is_empty(),
            "immediately again, nothing changed, well inside the safety interval"
        );

        std::thread::sleep(safety_interval + Duration::from_millis(80));
        let safety_tick = engine.run_tick();
        assert_eq!(
            safety_tick.len(), 1,
            "the safety interval must force a re-attempt even though nothing visible changed"
        );
        assert_eq!(
            safety_tick[0].outcome,
            Outcome::Suspended,
            "still nothing real to resolve — just re-tried"
        );
        let _ = started;
    }

    /// ADI-MONO-150: a `ctx.sleep` deadline wakes its run on its own, without any earlier spawn,
    /// and on time.
    #[test]
    fn a_sleep_deadline_wakes_the_run_without_an_earlier_spawn() {
        let config = scratch("readiness-sleep");
        let engine = Engine::with_config(config.clone());
        engine.bootstrap().expect("bootstrap");
        write_workflow(
            &config,
            "napper.ts",
            "import { manual, workflow } from \"@adi/workflows\";\n\
             export const on = manual();\n\
             export default workflow(async (ctx, input) => {\n\
             \x20 await ctx.sleep({ key: \"nap\", until: input.sleepUntil });\n\
             \x20 return { done: true };\n\
             });\n",
        );
        // Generous relative to the first attempt's own real `bun` spawn overhead (which the
        // "too soon" checks below must survive without crossing the deadline early) — the point
        // under test is the timing *logic*, not how fast this machine's first spawn happens to be.
        let sleep_for = Duration::from_millis(800);
        let start_instant = std::time::Instant::now();
        let sleep_until = (std::time::SystemTime::now() + sleep_for)
            .duration_since(std::time::SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_millis();
        let started = engine
            .start_manual("napper", &json!({"sleepUntil": sleep_until}), "req-1")
            .expect("start");

        let first = engine.run_tick();
        assert_eq!(first.len(), 1);
        assert_eq!(first[0].outcome, Outcome::Suspended);

        // Keep checking — no spawn should happen — until comfortably before the deadline itself.
        while start_instant.elapsed() + Duration::from_millis(150) < sleep_for {
            assert!(
                engine.run_tick().is_empty(),
                "too soon for the sleep's own deadline — no spawn yet"
            );
        }

        while start_instant.elapsed() < sleep_for {
            std::thread::sleep(Duration::from_millis(10));
        }
        let woke = engine.run_tick();
        assert_eq!(woke.len(), 1, "the deadline passing must wake the run on its own");
        assert_eq!(woke[0].outcome, Outcome::Completed(json!({"done": true})));
        let _ = started;
    }

    /// `attempt_once`'s own `ensure_run_dir` is private; the test needs the exact same path to
    /// check the side effect it wrote, so it's recomputed here rather than threading a new public
    /// accessor through the crate for one test.
    fn ensure_run_dir_for_test(config: &Config, run_key: &str) -> std::path::PathBuf {
        use sha2::{Digest, Sha256};
        WorkflowStore::with_config(config.clone())
            .dir()
            .join("attempts")
            .join(hex::encode(Sha256::digest(run_key.as_bytes())))
    }
}
