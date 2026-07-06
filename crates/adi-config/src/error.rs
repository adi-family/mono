//! The crate's error type, hand-rolled to keep the dependency set minimal.

use std::fmt;
use std::path::PathBuf;

/// The result type every fallible `adi-config` operation returns.
pub type Result<T> = std::result::Result<T, Error>;

/// Everything that can go wrong reading, parsing, or writing a store entry.
#[derive(Debug)]
pub enum Error {
    /// An I/O error creating a directory or reading/writing a file.
    Io(std::io::Error),
    /// A config file exists but does not parse as TOML into the target type.
    Parse {
        /// The config file that failed to parse.
        path: PathBuf,
        /// The underlying `toml` decode error.
        source: toml::de::Error,
    },
    /// A value could not be serialized to TOML before writing.
    Encode {
        /// The file the value was being written to.
        path: PathBuf,
        /// The underlying `toml` encode error.
        source: toml::ser::Error,
    },
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(e) => write!(f, "config store I/O error: {e}"),
            Self::Parse { path, source } => {
                write!(f, "config file at {} is invalid TOML: {source}", path.display())
            }
            Self::Encode { path, source } => {
                write!(f, "could not encode config for {}: {source}", path.display())
            }
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(e) => Some(e),
            Self::Parse { source, .. } => Some(source),
            Self::Encode { source, .. } => Some(source),
        }
    }
}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}
