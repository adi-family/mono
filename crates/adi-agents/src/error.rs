//! The crate's error type, hand-rolled to keep the dependency set minimal (mirroring
//! [`adi_config::Error`], which it wraps).

use std::fmt;

/// The result type every fallible `adi-agents` operation returns.
pub type Result<T> = std::result::Result<T, Error>;

/// Everything that can go wrong reading or mutating an agent definition.
#[derive(Debug)]
pub enum Error {
    /// The underlying config store failed (I/O, TOML parse, or TOML encode).
    Config(adi_config::Error),
    /// An agent name is empty, contains a path separator, or is `.`/`..` — anything that
    /// wouldn't be a safe single file name under `agents/`.
    InvalidName(String),
    /// No agent with this name is registered.
    NotFound(String),
    /// A directory operation (listing, removal) failed.
    Io(std::io::Error),
    /// The agent's backend has no run adapter yet (only tmux executors launch today).
    NotRunnable(String),
    /// A live session for this agent already exists.
    AlreadyRunning(String),
    /// Spawning the agent failed (tmux missing, session refused, …).
    Launch(String),
    /// The agent has no live session to interact with.
    NotRunning(String),
    /// A key name isn't a single tmux key token (`Enter`, `Up`, `C-c`, …).
    InvalidKey(String),
    /// A tmux command against a live session failed.
    Tmux(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Config(e) => write!(f, "agent store error: {e}"),
            Self::InvalidName(name) => write!(
                f,
                "invalid agent name {name:?}: use a single segment of letters, digits, '.', '-', or '_'"
            ),
            Self::NotFound(name) => write!(f, "no such agent: {name}"),
            Self::Io(e) => write!(f, "agent store I/O error: {e}"),
            Self::NotRunnable(backend) => write!(
                f,
                "backend {backend:?} can't be run yet — only tmux-backed agents (tmux:claude, tmux:codex) launch today"
            ),
            Self::AlreadyRunning(name) => write!(f, "agent {name} is already running"),
            Self::Launch(msg) => write!(f, "failed to launch agent: {msg}"),
            Self::NotRunning(name) => write!(f, "agent {name} isn't running"),
            Self::InvalidKey(key) => write!(
                f,
                "invalid key name {key:?}: use a single tmux key token like Enter, Escape, Up, or C-c"
            ),
            Self::Tmux(msg) => write!(f, "tmux error: {msg}"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Config(e) => Some(e),
            Self::Io(e) => Some(e),
            Self::InvalidName(_)
            | Self::NotFound(_)
            | Self::NotRunnable(_)
            | Self::AlreadyRunning(_)
            | Self::Launch(_)
            | Self::NotRunning(_)
            | Self::InvalidKey(_)
            | Self::Tmux(_) => None,
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
