//! The crate's error type, hand-rolled to keep the dependency set minimal (mirroring
//! [`adi_config::Error`], which it wraps — the same shape [`adi_tools`](adi_config) uses).

use std::fmt;

/// The result type every fallible `adi-db` operation returns.
pub type Result<T> = std::result::Result<T, Error>;

/// Everything that can go wrong resolving, opening, or querying a scoped database.
#[derive(Debug)]
pub enum Error {
    /// The underlying config store failed (I/O, TOML parse, or TOML encode).
    Config(adi_config::Error),
    /// A project id is empty, contains a path separator, or is `.`/`..` — anything that wouldn't
    /// be a safe single filename under `db/projects/`.
    InvalidProject(String),
    /// The database file for this scope doesn't exist yet (only raised by read-only paths, which
    /// never create one).
    NotFound(String),
    /// SQLite refused the statement or the connection.
    Sqlite(rusqlite::Error),
    /// A directory/file operation (creating `db/`, sizing a file, seeding the Bun client) failed.
    Io(std::io::Error),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Config(e) => write!(f, "db store error: {e}"),
            Self::InvalidProject(id) => {
                write!(f, "invalid project id {id:?}: {}", adi_config::NAME_RULE)
            }
            Self::NotFound(path) => write!(f, "no database at {path}"),
            Self::Sqlite(e) => write!(f, "sqlite error: {e}"),
            Self::Io(e) => write!(f, "db store I/O error: {e}"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Config(e) => Some(e),
            Self::Sqlite(e) => Some(e),
            Self::Io(e) => Some(e),
            Self::InvalidProject(_) | Self::NotFound(_) => None,
        }
    }
}

impl From<adi_config::Error> for Error {
    fn from(e: adi_config::Error) -> Self {
        Self::Config(e)
    }
}

impl From<rusqlite::Error> for Error {
    fn from(e: rusqlite::Error) -> Self {
        Self::Sqlite(e)
    }
}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}
