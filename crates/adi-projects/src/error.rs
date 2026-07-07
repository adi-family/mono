//! The crate's error type, hand-rolled to keep the dependency set minimal (mirroring
//! [`adi_config::Error`], which it wraps).

use std::fmt;

/// The result type every fallible `adi-projects` operation returns.
pub type Result<T> = std::result::Result<T, Error>;

/// Everything that can go wrong registering, reading, or mutating a project.
#[derive(Debug)]
pub enum Error {
    /// The underlying config store failed (I/O, TOML parse, or TOML encode).
    Config(adi_config::Error),
    /// A project id is empty, contains a path separator, or is `.`/`..` — anything that
    /// wouldn't be a safe single directory name under `projects/`.
    InvalidId(String),
    /// No project with this id is registered.
    NotFound(String),
    /// A project with this id already exists (on create).
    Exists(String),
    /// A directory operation (listing, removal) failed.
    Io(std::io::Error),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Config(e) => write!(f, "project store error: {e}"),
            Self::InvalidId(id) => write!(
                f,
                "invalid project id {id:?}: use a single path segment of letters, digits, '.', '-', or '_'"
            ),
            Self::NotFound(id) => write!(f, "no such project: {id}"),
            Self::Exists(id) => write!(f, "project already exists: {id}"),
            Self::Io(e) => write!(f, "project store I/O error: {e}"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Config(e) => Some(e),
            Self::Io(e) => Some(e),
            Self::InvalidId(_) | Self::NotFound(_) | Self::Exists(_) => None,
        }
    }
}

impl From<adi_config::Error> for Error {
    fn from(e: adi_config::Error) -> Self {
        Self::Config(e)
    }
}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}
