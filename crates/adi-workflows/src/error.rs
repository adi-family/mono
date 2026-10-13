//! The crate's error type.

use std::fmt;

/// The result type every fallible `adi-workflows` operation returns.
pub type Result<T> = std::result::Result<T, Error>;

/// A failure from discovering, describing, or starting a workflow run.
#[derive(Debug)]
pub enum Error {
    /// A workflow's `schedule(cron, tz)` cron expression is not exactly the 5 whitespace-
    /// separated fields (`min hour dom month dow`) of standard unix cron.
    InvalidCron(String, String),
    /// A `run_key` that [`crate::store::WorkflowStore::create_run`] was asked to claim is already
    /// in use — a starter pouring the same run twice, which must never happen (each starter claims
    /// its own run keys exactly once; see the schedule and event starters).
    DuplicateRunKey(String),
    /// A workflow id named by [`crate::Engine::describe_workflow`], [`crate::Engine::start_manual`]
    /// or the `enable`/`disable` facts names no file this engine currently discovers — a 404 for
    /// anything built over this crate (`adi-mono workflows show`, the panel's `/api/workflows/*`).
    UnknownWorkflow(String),
    /// Running `bun` to read a workflow file's `on` export failed, or it did not answer with the
    /// JSON this crate's describe harness is seeded to print.
    Describe(String),
    /// A step key was journaled twice within the same run — either a real bug in this crate (the
    /// TS `ctx` library's own in-process check is supposed to catch a workflow author's repeated
    /// key first, before it ever reaches here), or two attempts of the same run racing, which
    /// [`crate::Engine::run_tick`] is built not to do.
    DuplicateStepKey(String),
    /// A checkpoint's resolved value is larger than
    /// [`crate::store::MAX_STEP_VALUE_BYTES`] — spec section E: "bigger data goes as a path in
    /// `ctx.dir`", not the journal.
    JournalEntryTooLarge(String),
    /// A workflow attempt ended with an error the harness reported — a thrown exception, a zod
    /// validation failure on `input`/`output`, or a reused step key the TS side caught itself.
    AttemptFailed(String),
    /// A ctx.task/ctx.ask/ctx.run step's underlying task was given up on, and the workflow never
    /// caught the resulting `TaskGaveUp` itself — the run as a whole is what gives up.
    AttemptGaveUp(String),
    /// Underlying store I/O, SQLite, or filesystem failure.
    Store(anyhow::Error),
}

impl From<anyhow::Error> for Error {
    fn from(e: anyhow::Error) -> Self {
        Error::Store(e)
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::InvalidCron(expr, detail) => {
                write!(f, "{expr:?} is not a valid 5-field cron expression: {detail}")
            }
            Error::DuplicateRunKey(key) => write!(f, "run key {key:?} already exists"),
            Error::UnknownWorkflow(id) => write!(f, "no such workflow: {id:?}"),
            Error::Describe(why) => write!(f, "couldn't read the workflow file's `on` export: {why}"),
            Error::DuplicateStepKey(key) => write!(f, "step key {key:?} is already journaled for this run"),
            Error::JournalEntryTooLarge(key) => {
                write!(f, "step {key:?}'s resolved value is too large for the journal")
            }
            Error::AttemptFailed(why) => write!(f, "workflow attempt failed: {why}"),
            Error::AttemptGaveUp(why) => write!(f, "workflow run given up: {why}"),
            Error::Store(e) => write!(f, "workflow store error: {e}"),
        }
    }
}

impl std::error::Error for Error {}
