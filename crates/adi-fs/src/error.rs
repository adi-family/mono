//! The crate's error type, hand-rolled to keep the dependency set empty (this crate is a
//! pure-std primitive, so it doesn't reach for `thiserror`).

use std::fmt;
use std::io;

/// The result type every fallible [`Jail`](crate::Jail) operation returns.
pub type Result<T> = std::result::Result<T, Error>;

/// Everything that can go wrong resolving or touching a path inside a [`Jail`](crate::Jail).
#[derive(Debug)]
pub enum Error {
    /// The relative path tries to leave the base directory — it contains a `..` component, is
    /// absolute, has a drive/UNC prefix, or resolves (through a symlink) outside the base.
    /// This is the security boundary: "no going backward".
    Escape(String),
    /// No file or directory exists at the (in-bounds) relative path.
    NotFound(String),
    /// The operation expected a regular file but the path is a directory (or the reverse).
    NotAFile(String),
    /// A file's bytes are not valid UTF-8, so it can't be surfaced as editable text.
    NotText(String),
    /// Something already exists at the path a create asked for. Creates never clobber, so this
    /// is an error rather than a silent overwrite (that is what [`write`](crate::Jail::write)
    /// is for).
    AlreadyExists(String),
    /// An underlying I/O error, tagged with the relative path it happened on.
    Io {
        /// The relative path (within the jail) the error occurred on.
        path: String,
        /// The underlying OS error.
        source: io::Error,
    },
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Escape(path) => write!(
                f,
                "path {path:?} escapes the base directory: use a relative path with no `..` components"
            ),
            Self::NotFound(path) => write!(f, "no such file or directory: {path}"),
            Self::NotAFile(path) => write!(f, "not a file: {path}"),
            Self::NotText(path) => write!(f, "not a UTF-8 text file: {path}"),
            Self::AlreadyExists(path) => write!(f, "already exists: {path}"),
            Self::Io { path, source } => write!(f, "I/O error on {path}: {source}"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            Self::Escape(_)
            | Self::NotFound(_)
            | Self::NotAFile(_)
            | Self::NotText(_)
            | Self::AlreadyExists(_) => None,
        }
    }
}

impl Error {
    /// Map an [`io::Error`] on `path` to a friendly variant: a not-found becomes
    /// [`Error::NotFound`]; everything else keeps the OS error under [`Error::Io`].
    pub(crate) fn io(path: &str, source: io::Error) -> Self {
        match source.kind() {
            io::ErrorKind::NotFound => Self::NotFound(path.to_string()),
            _ => Self::Io {
                path: path.to_string(),
                source,
            },
        }
    }
}
