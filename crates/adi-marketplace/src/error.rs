//! The marketplace's own failures, phrased for the person who just hit one.

use adi_config::Error as ConfigError;

/// Everything the marketplace can refuse to do.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The store itself could not be read or written.
    #[error(transparent)]
    Config(#[from] ConfigError),
    /// A file under the store could not be read or written.
    #[error(transparent)]
    Io(#[from] std::io::Error),
    /// A source's name has to be one safe path segment — it names the cache file.
    #[error("invalid marketplace name {0:?}: {rule}", rule = adi_config::NAME_RULE)]
    InvalidName(String),
    /// A source URL has to be `https://` — this is the one place a manifest is fetched from, and
    /// the operator's ruling is that fetching is HTTPS, always.
    #[error("a marketplace url must start with https:// — got {0:?}")]
    NotHttps(String),
    /// A source by that name is already configured.
    #[error(
        "a marketplace named {0} is already configured — remove it first, or pick another name"
    )]
    Duplicate(String),
    /// No source by that name.
    #[error("no marketplace named {0} — add one with `adi-mono marketplace add`")]
    UnknownSource(String),
    /// A spec has to name both halves: which marketplace, which app.
    #[error("{0:?} names no app — install takes <marketplace>/<app-slug>, e.g. adi/crm")]
    BadSpec(String),
    /// The slug an entry carries is not installable as a directory name.
    #[error("app slug {0:?} is not a single safe path segment: {rule}", rule = adi_config::NAME_RULE)]
    BadSlug(String),
    /// The source has never been synced, so there is no cache to install from.
    #[error("no cached manifest for {0} — run `adi-mono marketplace sync` first")]
    NotSynced(String),
    /// The manifest carries no such app.
    #[error("{0} carries no app named {1} — it carries: {2}")]
    UnknownApp(String, String, String),
    /// Something already answers to that slug in the dashboard store. Refused rather than
    /// numbered or overwritten: a silent `crm-2` is a surprise, and a silent replace is worse.
    #[error(
        "a dashboard named {0} is already there ({1}) — reinstall over it with `--force`, which \
         replaces its files"
    )]
    Collision(String, String),
    /// The artifact at the entry's URL is not a bundle this build will land.
    #[error("the artifact for {0} is not a valid dashboard bundle: {1}")]
    BadArtifact(String, String),
    /// Nothing could be fetched.
    #[error("{0}")]
    Fetch(String),
    /// Nothing by that slug is installed, so there is nothing to start.
    #[error(
        "no dashboard named {0} is installed — `adi-mono marketplace install <marketplace>/{0}` first"
    )]
    NotInstalled(String),
}

/// The outcome alias every fallible operation answers with.
pub type Result<T> = std::result::Result<T, Error>;
