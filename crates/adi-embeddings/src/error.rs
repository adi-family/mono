//! The store's own error type, distinct from [`adi_indexer::embed::EmbedError`]: that one is
//! what a runtime says when it fails to *turn text into a vector*, this one is what the store
//! says when a definition on disk cannot mean what it says. `resolve` returns the runtime's
//! error directly rather than wrapping it, so a caller matching on `EmbedError::Unavailable`
//! (as `adi_knowledge`/`adi_facts` already do) keeps working unchanged once phase B wires them
//! through this crate.

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("embedding store error: {0}")]
    Config(#[from] adi_config::Error),
    #[error(
        "invalid embedding backend name {0:?}: {rule}",
        rule = adi_config::NAME_RULE
    )]
    InvalidName(String),
    #[error("invalid embedding backend: {0}")]
    Arguments(String),
    #[error("no such embedding backend: {0}")]
    NotFound(String),
    #[error("no embedding backend is assigned to {0:?}")]
    Unassigned(String),
    #[error(transparent)]
    Embed(#[from] adi_indexer::embed::EmbedError),
    #[error("embedding store I/O error: {0}")]
    Io(#[from] std::io::Error),
}
