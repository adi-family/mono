//! Everything that can go wrong in this crate, folded into one type so a caller matches on one
//! enum rather than threading four crates' own errors through every signature.

/// The result type every fallible call in this crate returns.
pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug)]
pub enum Error {
    /// An unsafe or empty connection id.
    InvalidId(String),
    /// No connection is registered under that id.
    NotFound(String),
    /// The on-disk store failed to read or write.
    Config(adi_config::Error),
    /// A call into the agent store failed — launching, replying, or reading a session.
    Agents(adi_agents::Error),
    /// A call into the event bus failed — publishing `adi.channels.message`, or draining the
    /// spool for the run-finished watcher.
    Events(adi_events::Error),
    /// A call into the secrets store failed — reading or writing a node token (`src/token.rs`).
    Secrets(adi_secrets::Error),
    /// A call into the tools store failed — seeding or refreshing the `channel-reply` tool.
    Tools(adi_tools::Error),
    /// The router (or a stand-in for it in a test) answered something this node can't use: a
    /// non-2xx status, or a body that isn't the JSON shape expected.
    Router(String),
    /// The HTTP call to the router itself failed (DNS, connect, timeout, …).
    Http(String),
    /// The WebSocket handshake or framing broke — a bad upgrade response, an oversized or
    /// malformed frame, or the socket closing mid-read.
    Protocol(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidId(id) => write!(f, "invalid connection id {id:?}"),
            Self::NotFound(id) => write!(f, "no connection named {id:?}"),
            Self::Config(e) => write!(f, "channels store error: {e}"),
            Self::Agents(e) => write!(f, "agent store error: {e}"),
            Self::Events(e) => write!(f, "event bus error: {e}"),
            Self::Secrets(e) => write!(f, "secrets store error: {e}"),
            Self::Tools(e) => write!(f, "tools store error: {e}"),
            Self::Router(msg) => write!(f, "the router refused the call: {msg}"),
            Self::Http(msg) => write!(f, "couldn't reach the router: {msg}"),
            Self::Protocol(msg) => write!(f, "websocket protocol error: {msg}"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Config(e) => Some(e),
            Self::Agents(e) => Some(e),
            Self::Events(e) => Some(e),
            Self::Secrets(e) => Some(e),
            Self::Tools(e) => Some(e),
            Self::InvalidId(_)
            | Self::NotFound(_)
            | Self::Router(_)
            | Self::Http(_)
            | Self::Protocol(_) => None,
        }
    }
}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Self::Protocol(e.to_string())
    }
}

impl From<adi_config::Error> for Error {
    fn from(e: adi_config::Error) -> Self {
        Self::Config(e)
    }
}

impl From<adi_agents::Error> for Error {
    fn from(e: adi_agents::Error) -> Self {
        Self::Agents(e)
    }
}

impl From<adi_events::Error> for Error {
    fn from(e: adi_events::Error) -> Self {
        Self::Events(e)
    }
}

impl From<adi_secrets::Error> for Error {
    fn from(e: adi_secrets::Error) -> Self {
        Self::Secrets(e)
    }
}

impl From<adi_tools::Error> for Error {
    fn from(e: adi_tools::Error) -> Self {
        Self::Tools(e)
    }
}
