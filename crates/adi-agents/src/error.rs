use std::fmt;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug)]
pub enum Error {
    Config(adi_config::Error),
    Arguments(String),
    InvalidName(String),
    NotFound(String),
    Exists(String),
    Io(std::io::Error),
    NotRunnable(String),
    AlreadyRunning(String),
    Launch(String),
    NotRunning(String),
    InvalidKey(String),
    Tmux(String),
    Process(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Config(e) => write!(f, "agent store error: {e}"),
            Self::Arguments(message) => write!(f, "invalid agent arguments: {message}"),
            Self::InvalidName(name) => write!(
                f,
                "invalid agent name {name:?}: use a single segment of letters, digits, '.', '-', or '_'"
            ),
            Self::NotFound(name) => write!(f, "no such agent: {name}"),
            Self::Exists(name) => write!(f, "an agent named {name} already exists"),
            Self::Io(e) => write!(f, "agent store I/O error: {e}"),
            Self::NotRunnable(backend) => write!(
                f,
                "backend {backend:?} can't be run yet — tmux/process Claude and Codex agents, harness:claude-sdk, and wasm agents launch today"
            ),
            Self::AlreadyRunning(name) => write!(f, "agent {name} is already running"),
            Self::Launch(msg) => write!(f, "failed to launch agent: {msg}"),
            Self::NotRunning(name) => write!(f, "agent {name} isn't running"),
            Self::InvalidKey(key) => write!(
                f,
                "invalid key name {key:?}: use a single tmux key token like Enter, Escape, Up, or C-c"
            ),
            Self::Tmux(msg) => write!(f, "tmux error: {msg}"),
            Self::Process(msg) => write!(f, "process error: {msg}"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Config(e) => Some(e),
            Self::Io(e) => Some(e),
            Self::Arguments(_)
            | Self::InvalidName(_)
            | Self::NotFound(_)
            | Self::Exists(_)
            | Self::NotRunnable(_)
            | Self::AlreadyRunning(_)
            | Self::Launch(_)
            | Self::NotRunning(_)
            | Self::InvalidKey(_)
            | Self::Tmux(_)
            | Self::Process(_) => None,
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
