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
    /// Running `bun` to read a workflow file's `on` export failed, or it did not answer with the
    /// JSON this crate's describe harness is seeded to print.
    Describe(String),
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
            Error::Describe(why) => write!(f, "couldn't read the workflow file's `on` export: {why}"),
            Error::Store(e) => write!(f, "workflow store error: {e}"),
        }
    }
}

impl std::error::Error for Error {}
