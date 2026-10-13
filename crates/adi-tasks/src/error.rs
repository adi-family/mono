//! The crate's error type. The first four variants are *client* errors (a caller passed a bad
//! id or would break the tree); [`Error::Store`] wraps an internal I/O / (de)serialize failure.

use std::fmt;

/// The result type every fallible `adi-tasks` store operation returns.
pub type Result<T> = std::result::Result<T, Error>;

/// A failure from a [`Tasks`](crate::Tasks) operation.
#[derive(Debug)]
pub enum Error {
    /// No task with this id.
    NotFound(String),
    /// A referenced parent id does not exist.
    ParentMissing(String),
    /// Setting the requested parent would create a cycle.
    Cycle,
    /// Tried to complete an archived task.
    ReopenFirst,
    /// Underlying store I/O or (de)serialization failure.
    Store(anyhow::Error),
    /// A task's `output_schema` is not valid JSON Schema.
    InvalidOutputSchema(String),
    /// The task's `output_schema` makes a result mandatory, and `complete` was called without one.
    ResultRequired,
    /// The given result does not satisfy the task's `output_schema`.
    ResultInvalid {
        /// The JSON pointer into the result where validation failed (e.g. `/a/b`).
        path: String,
        /// The validator's own message for that failure.
        message: String,
    },
    /// The requested `workflow_run`/`step_key` repeats a stepId already in use.
    DuplicateStepId(String),
    /// Tried to give up on a task that is not open (already `done` or `archived`).
    NotOpen(String),
}

impl From<anyhow::Error> for Error {
    fn from(e: anyhow::Error) -> Self {
        Error::Store(e)
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::NotFound(id) => write!(f, "no task with id {id:?}"),
            Error::ParentMissing(id) => write!(f, "no parent task with id {id:?}"),
            Error::Cycle => write!(f, "setting that parent would create a cycle"),
            Error::ReopenFirst => write!(f, "task is archived; reopen it first"),
            Error::Store(e) => write!(f, "task store error: {e}"),
            Error::InvalidOutputSchema(detail) => {
                write!(f, "output_schema is not valid JSON Schema: {detail}")
            }
            Error::ResultRequired => {
                write!(f, "this task's output_schema requires a --result")
            }
            Error::ResultInvalid { path, message } => {
                write!(f, "result does not satisfy output_schema at {path}: {message}")
            }
            Error::DuplicateStepId(step_id) => {
                write!(f, "step {step_id:?} already exists for this run")
            }
            Error::NotOpen(status) => write!(f, "task is {status}; cannot give up on it"),
        }
    }
}

impl std::error::Error for Error {}
