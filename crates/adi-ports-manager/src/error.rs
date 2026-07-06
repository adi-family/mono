//! The crate's error type, hand-rolled to keep the dependency set minimal.

use std::fmt;
use std::ops::RangeInclusive;
use std::path::PathBuf;

/// The result type every fallible `adi-ports-manager` operation returns.
pub type Result<T> = std::result::Result<T, Error>;

/// Everything that can go wrong allocating or persisting a port.
#[derive(Debug)]
pub enum Error {
    /// Every port in the configured range is reserved or already in use.
    Exhausted {
        /// The range that was scanned end to end without finding a free port.
        range: RangeInclusive<u16>,
    },
    /// The registry lock could not be acquired before the timeout.
    LockTimeout {
        /// The lock file that could not be taken.
        path: PathBuf,
    },
    /// An I/O error reading or writing the registry or its lock file.
    Io(std::io::Error),
    /// The registry file exists but does not hold valid JSON.
    Corrupt {
        /// The registry file that failed to parse.
        path: PathBuf,
        /// The underlying `serde_json` decode error.
        source: serde_json::Error,
    },
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Exhausted { range } => write!(
                f,
                "no free port available in range {}..={}",
                range.start(),
                range.end()
            ),
            Self::LockTimeout { path } => {
                write!(
                    f,
                    "timed out acquiring port registry lock at {}",
                    path.display()
                )
            }
            Self::Io(e) => write!(f, "port registry I/O error: {e}"),
            Self::Corrupt { path, source } => {
                write!(
                    f,
                    "port registry at {} is corrupt: {source}",
                    path.display()
                )
            }
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(e) => Some(e),
            Self::Corrupt { source, .. } => Some(source),
            Self::Exhausted { .. } | Self::LockTimeout { .. } => None,
        }
    }
}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}
