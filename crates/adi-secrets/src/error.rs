//! The crate's error type, hand-rolled to keep the dependency set minimal (mirroring
//! [`adi_config::Error`], which it wraps).

use std::fmt;

/// The result type every fallible `adi-secrets` operation returns.
pub type Result<T> = std::result::Result<T, Error>;

/// Everything that can go wrong storing, reading, or resolving a secret.
#[derive(Debug)]
pub enum Error {
    /// The underlying config store failed (I/O, TOML parse, or TOML encode).
    Config(adi_config::Error),
    /// A secret key name or project id is empty, contains a path separator, or is `.`/`..` —
    /// anything that wouldn't be a safe single path segment under `secrets/`.
    InvalidName(String),
    /// No secret with this name exists in the given scope.
    NotFound(String),
    /// A directory operation (listing, removal, chmod) failed.
    Io(std::io::Error),
    /// Key handling or encryption failed (unreadable/wrong-length key file, cipher failure).
    Crypto(String),
    /// A stored value could not be decrypted — wrong master key, tampering, or a value moved
    /// out of the file it was bound to. Deliberately opaque: it never says which.
    Decrypt,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Config(e) => write!(f, "secret store error: {e}"),
            Self::InvalidName(name) => write!(
                f,
                "invalid secret name {name:?}: use a letter or '_' then letters, digits, or '_' (a valid environment-variable name)"
            ),
            Self::NotFound(name) => write!(f, "no such secret: {name}"),
            Self::Io(e) => write!(f, "secret store I/O error: {e}"),
            Self::Crypto(msg) => write!(f, "secret encryption error: {msg}"),
            Self::Decrypt => write!(f, "could not decrypt secret (wrong key or tampered value)"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Config(e) => Some(e),
            Self::Io(e) => Some(e),
            Self::InvalidName(_) | Self::NotFound(_) | Self::Crypto(_) | Self::Decrypt => None,
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
