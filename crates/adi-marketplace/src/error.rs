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
    /// The slug an entry carries is not one safe path segment.
    #[error("app slug {0:?} is not a single safe path segment: {rule}", rule = adi_config::NAME_RULE)]
    BadSlug(String),
    /// The repository an entry names is not one this build will clone.
    #[error(
        "{0:?} is not a repository this installs from — an app's repo must be an https:// url \
         (or a file:// path while it is being developed)"
    )]
    BadRepo(String),
    /// The commit an entry pins is not a full git object name.
    #[error(
        "{0} pins {1:?}, which is not a commit — a manifest pins a full 40-character commit, \
         never a branch or a tag, because the pin is what makes an install repeatable"
    )]
    BadCommit(String, String),
    /// The source has never been synced, so there is no cache to install from.
    #[error("no cached manifest for {0} — run `adi-mono marketplace sync` first")]
    NotSynced(String),
    /// The manifest carries no such app.
    #[error("{0} carries no app named {1} — it carries: {2}")]
    UnknownApp(String, String, String),
    /// An install was asked for with nothing to call the copy.
    #[error("give the app a name — it is what you will see it under, and you can rename it later")]
    EmptyName,
    /// Git itself refused, or is not installed. Carries git's own last line.
    #[error("{0}")]
    Git(String),
    /// The cloned repository is not laid out as a dashboard, so nothing here could run it.
    #[error(
        "{0} does not look like an ADI app: {1}. An app repository is a dashboard — \
         `frontend/index.ts` and `backend/index.ts` at its root (guides/dashboards.md)"
    )]
    NotAnApp(String, String),
    /// Nothing on this machine by that id was installed from a marketplace.
    #[error("no installed app called {0} — `adi-mono marketplace apps` lists what is here")]
    NotInstalled(String),
    /// An update was asked for on a copy with uncommitted work in it.
    #[error(
        "{0} has uncommitted changes — commit or stash them first, or force the update to reset \
         onto the pin and lose them"
    )]
    Dirty(String),
    /// Nothing could be fetched.
    #[error("{0}")]
    Fetch(String),
}

/// The outcome alias every fallible operation answers with.
pub type Result<T> = std::result::Result<T, Error>;
