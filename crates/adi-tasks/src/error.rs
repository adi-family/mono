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
        }
    }
}

impl std::error::Error for Error {}
