//! The crate's error type, hand-rolled to keep the dependency set minimal (mirroring
//! [`adi_tools::Error`], which it is closest in shape to).

use std::fmt;

/// The result type every fallible `adi-mcp` operation returns.
pub type Result<T> = std::result::Result<T, Error>;

/// Everything that can go wrong registering, reading, or editing an MCP server definition.
#[derive(Debug)]
pub enum Error {
    /// The underlying config store failed (I/O, TOML parse, or TOML encode).
    Config(adi_config::Error),
    /// A server id is empty, contains a path separator, or is `.`/`..` — anything that
    /// wouldn't be a safe single directory name under `mcp/`.
    InvalidId(String),
    /// No server with this id is registered.
    NotFound(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Config(e) => write!(f, "mcp store error: {e}"),
            Self::InvalidId(id) => {
                write!(f, "invalid mcp server id {id:?}: {}", adi_config::NAME_RULE)
            }
            Self::NotFound(id) => write!(f, "no such mcp server: {id}"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Config(e) => Some(e),
            Self::InvalidId(_) | Self::NotFound(_) => None,
        }
    }
}

impl From<adi_config::Error> for Error {
    fn from(e: adi_config::Error) -> Self {
        Self::Config(e)
    }
}
