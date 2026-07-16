//! The crate's error type, hand-rolled to keep the dependency set minimal (mirroring
//! [`adi_config::Error`], which it wraps).

use std::fmt;

/// The result type every fallible `adi-triggers` operation returns.
pub type Result<T> = std::result::Result<T, Error>;

/// Everything that can go wrong reading, mutating, or firing a trigger definition.
#[derive(Debug)]
pub enum Error {
    /// The underlying config store failed (I/O, TOML parse, or TOML encode).
    Config(adi_config::Error),
    /// A trigger name is empty, contains a path separator, or is `.`/`..` — anything that
    /// wouldn't be a safe single file name under `triggers/`.
    InvalidName(String),
    /// No trigger with this name is registered.
    NotFound(String),
    /// A directory operation (listing, removal) failed.
    Io(std::io::Error),
    /// The trigger has no code block to run, so there is nothing to fire.
    NoCode(String),
    /// Spawning the trigger's code block failed (shell missing, log unwritable, …).
    Launch(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Config(e) => write!(f, "trigger store error: {e}"),
            Self::InvalidName(name) => write!(
                f,
                "invalid trigger name {name:?}: use a single segment of letters, digits, '.', '-', or '_'"
            ),
            Self::NotFound(name) => write!(f, "no such trigger: {name}"),
            Self::Io(e) => write!(f, "trigger store I/O error: {e}"),
            Self::NoCode(name) => write!(f, "trigger {name} has no code block to run"),
            Self::Launch(msg) => write!(f, "failed to fire trigger: {msg}"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Config(e) => Some(e),
            Self::Io(e) => Some(e),
            Self::InvalidName(_) | Self::NotFound(_) | Self::NoCode(_) | Self::Launch(_) => None,
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
