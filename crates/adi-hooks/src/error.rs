//! The crate's error type, hand-rolled to keep the dependency set minimal (mirroring the
//! other `adi-*` store crates).

use std::fmt;
use std::path::PathBuf;

/// The result type every fallible `adi-hooks` operation returns.
pub type Result<T> = std::result::Result<T, Error>;

/// Everything that can go wrong reading, running, or registering hooks and workspaces.
#[derive(Debug)]
pub enum Error {
    /// A hook or workspace name is empty, contains a path separator, starts with a dot, or is
    /// the reserved `logs` — anything that wouldn't be a safe single file name under
    /// `.adi/hooks/` (names are joined onto paths, so this is the traversal boundary).
    InvalidName(String),
    /// A hook file or workspace (name or target directory) already exists.
    Exists(String),
    /// No hook file / registered workspace with this name.
    NotFound(String),
    /// The lifecycle hook a workspace create needs (`init` or `workspace`) has no file yet.
    NoHook(String),
    /// The hook file exists but is blank, so there is nothing to run.
    EmptyHook(String),
    /// The `workspace` hook runs inside the primary workspace, but its directory is missing
    /// on disk (the first workspace is still being created, or its creation failed).
    PrimaryMissing,
    /// An explicit workspace path must be absolute.
    NotAbsolute(PathBuf),
    /// A local workspace link target is missing or not a directory.
    NotADir(PathBuf),
    /// Spawning the hook's shell failed (shell missing, log unwritable, …).
    Launch(String),
    /// The workspaces registry (`.adi/workspaces.toml`) failed to parse or serialize.
    Registry(String),
    /// A filesystem operation failed.
    Io(std::io::Error),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidName(name) => write!(
                f,
                "invalid name {name:?}: use a single segment of letters, digits, '.', '-', or '_' (no leading dot, not 'logs')"
            ),
            Self::Exists(what) => write!(f, "already exists: {what}"),
            Self::NotFound(name) => write!(f, "not found: {name}"),
            Self::NoHook(name) => write!(f, "no {name} hook file — create one under .adi/hooks first"),
            Self::EmptyHook(name) => write!(f, "hook {name} is blank, nothing to run"),
            Self::PrimaryMissing => write!(
                f,
                "the primary workspace's directory isn't on disk yet — wait for the first workspace to finish (or fix it) before adding another"
            ),
            Self::NotAbsolute(p) => write!(f, "workspace path must be absolute: {}", p.display()),
            Self::NotADir(p) => write!(f, "not an existing directory: {}", p.display()),
            Self::Launch(msg) => write!(f, "failed to run hook: {msg}"),
            Self::Registry(msg) => write!(f, "workspaces registry error: {msg}"),
            Self::Io(e) => write!(f, "hooks I/O error: {e}"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(e) => Some(e),
            _ => None,
        }
    }
}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}
