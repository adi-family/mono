//! What can go wrong in the knowledge store.

/// Errors this crate returns.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// A base id, note id, project id, or agent name that is not a safe path segment.
    #[error("invalid name: {0}")]
    InvalidName(String),

    /// A base id that does not parse — `global/notes`, `project:acme/runbook`,
    /// `agent:solver/memory`.
    #[error("cannot parse knowledge base id: {0}")]
    BadBaseId(String),

    /// A base that has not been created.
    #[error("no such knowledge base: {0}")]
    NoSuchBase(String),

    /// A base id that is already taken.
    #[error("knowledge base already exists: {0}")]
    BaseExists(String),

    /// A note id that is not in the base.
    #[error("no such knowledge: {0}")]
    NoSuchKnowledge(String),

    /// A provider name nothing is registered under.
    #[error("unknown knowledge backend provider: {0}")]
    NoSuchProvider(String),

    /// The reader may not touch this base at all, or may only read it.
    #[error("{reader} may not {verb} {base}")]
    Denied {
        /// Who asked.
        reader: String,
        /// What they asked to do — `read` or `write`.
        verb: &'static str,
        /// The base they asked about.
        base: String,
    },

    /// A note with no title and no body: there is nothing to embed and nothing to find.
    #[error("knowledge needs a title or a body")]
    Empty,

    /// The embedder could not be built, or could not embed.
    #[error("embedding failed: {0}")]
    Embed(String),

    /// A backend failed — SQLite, a file, or whatever a third-party provider talks to.
    #[error("knowledge backend: {0}")]
    Backend(String),

    /// The config store could not read or write a base manifest.
    #[error(transparent)]
    Config(#[from] adi_config::Error),

    /// A filesystem failure outside the config store — creating a base's data dir, dropping it.
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

impl From<rusqlite::Error> for Error {
    fn from(e: rusqlite::Error) -> Self {
        Self::Backend(e.to_string())
    }
}

/// The result type every fallible call in this crate returns.
pub type Result<T> = std::result::Result<T, Error>;
