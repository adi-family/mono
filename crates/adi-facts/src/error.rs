//! What can go wrong in the fact base.
//!
//! Most of these exist because the prototype failed the same way *silently*. `--keep` naming
//! neither side of a pair was read as "the base side won" and threw the incoming fact away
//! without a word; committing while a pair was open would have been a base holding an
//! undecided contradiction. Each is now a named error that says what the two valid answers are.

use adi_knowledge::Error as KnowledgeError;

/// Errors this crate returns.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// A base id that does not parse, or names a scope nothing defines.
    #[error("cannot parse fact base id: {0}")]
    BadBaseId(String),

    /// A base that has not been created. `facts add` creates one; nothing else does.
    #[error("no such fact base: {0}")]
    NoSuchBase(String),

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

    /// A transaction id nothing is staged under.
    #[error("no such transaction: {0}")]
    NoSuchTransaction(String),

    /// A pair number that is not in this transaction.
    #[error("no such pair p{pair} in {tx}")]
    NoSuchPair {
        /// The transaction.
        tx: String,
        /// The pair number asked for.
        pair: i64,
    },

    /// A fact id that is not in the base.
    #[error("no such fact: {0}")]
    NoSuchFact(String),

    /// A `--from` that is neither a fact id nor `#N` for a fact in this batch.
    #[error("--from {0} is neither a fact id nor #N for a fact staged in this batch")]
    BadSource(String),

    /// A `--from #N` naming a staged fact that a verdict has since thrown away.
    ///
    /// Refused rather than skipped: an edge that is quietly not written is a derived node that
    /// never goes stale, which is the one outcome this design exists to prevent — arriving as
    /// silence rather than as an error.
    #[error(
        "--from {0} names a fact this transaction dropped, so there is nothing to derive from. \
         Re-run `tx show` and pick a source that is still staged."
    )]
    SourceDropped(String),

    /// `merge` arrived without the sentence that replaces both sides.
    #[error(
        "merge needs --fact: the one sentence that says what both say.\n  left:  {left}\n  right: {right}"
    )]
    MergeNeedsFact {
        /// The incoming fact.
        left: String,
        /// The fact it was paired with.
        right: String,
    },

    /// `supersede` arrived without a winner.
    #[error("supersede needs --keep: the winner, {left} or {right}")]
    SupersedeNeedsKeep {
        /// How the incoming fact is named.
        left: String,
        /// How the other side is named.
        right: String,
    },

    /// `--keep` named something that is not a side of this pair.
    ///
    /// This was the third silent bug: an unrecognised id was read as "the base side won", so a
    /// typo discarded the incoming fact and said nothing.
    #[error("--keep {keep} is neither side of p{pair}. It must be {left} or {right}.")]
    KeepIsNotASide {
        /// What the caller passed.
        keep: String,
        /// The pair number.
        pair: i64,
        /// The valid answer naming the incoming fact.
        left: String,
        /// The valid answer naming the other side.
        right: String,
    },

    /// A commit was asked for while pairs were still open. Names them.
    #[error("cannot commit: {count} pair(s) still open — {pairs}")]
    StillOpen {
        /// How many.
        count: usize,
        /// Their labels, strongest first.
        pairs: String,
    },

    /// A transaction that has already been committed or aborted.
    #[error("{tx} is {state}; nothing further can be done to it")]
    TransactionClosed {
        /// The transaction.
        tx: String,
        /// What state it is in.
        state: String,
    },

    /// The embedder could not be built, or could not embed.
    #[error("embedding failed: {0}")]
    Embed(String),

    /// The classifier or extractor could not be reached, or gave nothing back.
    #[error("the model could not be reached: {0}")]
    Judge(String),

    /// SQLite failed.
    #[error("fact base: {0}")]
    Backend(String),

    /// The config store could not read or write a base manifest.
    #[error(transparent)]
    Config(#[from] adi_config::Error),

    /// A filesystem failure outside the config store.
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

impl From<rusqlite::Error> for Error {
    fn from(e: rusqlite::Error) -> Self {
        Self::Backend(e.to_string())
    }
}

/// Knowledge's scope layer is reused whole, so its errors have to land somewhere.
///
/// Only the three a [`BaseId`](adi_knowledge::BaseId) or a [`Reader`](adi_knowledge::Reader) can
/// actually produce are mapped; the rest are about *notes*, which this crate never touches, and
/// arriving here would mean the reuse had gone wrong rather than that a user had.
impl From<KnowledgeError> for Error {
    fn from(e: KnowledgeError) -> Self {
        match e {
            KnowledgeError::InvalidName(name) | KnowledgeError::BadBaseId(name) => {
                Self::BadBaseId(name)
            }
            KnowledgeError::NoSuchBase(id) => Self::NoSuchBase(id),
            KnowledgeError::Denied { reader, verb, base } => Self::Denied { reader, verb, base },
            KnowledgeError::Embed(why) => Self::Embed(why),
            KnowledgeError::Config(e) => Self::Config(e),
            KnowledgeError::Io(e) => Self::Io(e),
            other => Self::Backend(other.to_string()),
        }
    }
}

/// The result type every fallible call in this crate returns.
pub type Result<T> = std::result::Result<T, Error>;
