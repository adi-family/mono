//! The crate's error type, hand-rolled to keep the dependency set minimal (mirroring
//! [`adi_config::Error`], which it wraps — the same shape [`adi_projects`] uses).

use std::fmt;

/// The result type every fallible `adi-tools` operation returns.
pub type Result<T> = std::result::Result<T, Error>;

/// Everything that can go wrong registering, reading, editing, or running a tool.
#[derive(Debug)]
pub enum Error {
    /// The underlying config store failed (I/O, TOML parse, or TOML encode).
    Config(adi_config::Error),
    /// A tool id is empty, contains a path separator, or is `.`/`..` — anything that
    /// wouldn't be a safe single directory name under `tools/`.
    InvalidId(String),
    /// No tool with this id is registered.
    NotFound(String),
    /// The runtime string isn't one this build understands (`sh` or `ts`).
    InvalidRuntime(String),
    /// A linked tool points at a path that doesn't exist (or isn't readable) on disk.
    LinkedMissing(String),
    /// A built-in system tool cannot be hard-deleted (archive it to disable it instead).
    SystemProtected(String),
    /// A directory/file operation (listing, script read/write, removal, `.bin` sync) failed.
    Io(std::io::Error),
    /// Launching a tool's interpreter failed.
    Launch(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Config(e) => write!(f, "tool store error: {e}"),
            Self::InvalidId(id) => write!(
                f,
                "invalid tool id {id:?}: use a single path segment of letters, digits, '.', '-', or '_'"
            ),
            Self::NotFound(id) => write!(f, "no such tool: {id}"),
            Self::InvalidRuntime(r) => write!(f, "unknown tool runtime {r:?}: use 'sh' or 'ts'"),
            Self::LinkedMissing(path) => write!(f, "linked tool file not found: {path}"),
            Self::SystemProtected(id) => {
                write!(f, "{id} is a built-in system tool: archive it instead of deleting")
            }
            Self::Io(e) => write!(f, "tool store I/O error: {e}"),
            Self::Launch(msg) => write!(f, "could not run tool: {msg}"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Config(e) => Some(e),
            Self::Io(e) => Some(e),
            Self::InvalidId(_)
            | Self::NotFound(_)
            | Self::InvalidRuntime(_)
            | Self::LinkedMissing(_)
            | Self::SystemProtected(_)
            | Self::Launch(_) => None,
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
