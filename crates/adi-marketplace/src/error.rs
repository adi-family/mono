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
    /// A source URL has to be `https://`, or a `file:///` absolute path for a manifest developed
    /// locally before it is published — the same carve-out a bundle's own `repo` gets, and for the
    /// same reason (`sources::valid_url`).
    #[error(
        "a marketplace url must be https:// (or a file:// path while it is being developed) — \
         got {0:?}"
    )]
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
    #[error("bundle slug {0:?} is not a single safe path segment: {rule}", rule = adi_config::NAME_RULE)]
    BadSlug(String),
    /// The repository an entry names is not one this build will clone.
    #[error(
        "{0:?} is not a repository this installs from — a bundle's repo must be an https:// url \
         (or a file:// path while it is being developed)"
    )]
    BadRepo(String),
    /// The commit an entry pins is not a full git object name.
    #[error(
        "{0} pins {1:?}, which is not a commit — a manifest pins a full 40-character commit, \
         never a branch or a tag, because the pin is what makes an install repeatable"
    )]
    BadCommit(String, String),
    /// The icon an entry publishes is not one a listing will draw.
    #[error(
        "{0} carries an icon this will not draw: {1:?} — a bundle's icon must be an https:// url, \
         or a data:image/… uri to carry it in the manifest itself and fetch nothing"
    )]
    BadIcon(String, String),
    /// A gallery entry names something a page will not draw.
    #[error(
        "{0} carries a gallery item this will not draw: {1:?} — a picture or clip must be an \
         https:// url, or a data:image/… or data:video/… uri to carry it in the manifest itself"
    )]
    BadMedia(String, String),
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
    /// An element in a bundle's preview carries no kind — required whenever `elements` is
    /// published at all.
    #[error(
        "{0} publishes an element with no kind — one of agent, tool, dashboard, llm, embedding, \
         service, trigger, project is required"
    )]
    BadElementKind(String),
    /// An element's name is not one safe path segment — it doubles as the third address
    /// coordinate (`<marketplace>/<slug>/<kind>/<name>`), so it is held to the same rule a slug is.
    #[error(
        "{0} publishes an element named {1:?}, which is not a single safe path segment: {rule}",
        rule = adi_config::NAME_RULE
    )]
    BadElementName(String, String),
    /// An address named more than a bundle, and what followed the slug was not one element of it.
    #[error(
        "{0:?} does not name one element of a bundle — use <marketplace>/<slug>/<kind>/<name> \
         (agents, tools, dashboards, llm, embeddings, services, triggers) or \
         <marketplace>/<slug>/project for the scaffold"
    )]
    BadAddress(String),
    /// A bundle repository carries Rust — source or a compiled binary — under one of its kind
    /// directories. Refused per "No Rust in an item": the store has never grown the capability to
    /// compile or run anything untrusted, and a bundle is not where that starts
    /// (`docs/marketplace-bundles.md`).
    #[error(
        "{0} carries {1} at {2} — an item may never carry Rust source or a compiled binary \
         (docs/marketplace-bundles.md)"
    )]
    CarriesRust(String, String, String),
    /// An address named one element of a bundle, but the tree at the pinned commit does not carry
    /// it — the manifest's preview promised something the repository does not, or the operator
    /// simply mistyped the coordinate (`docs/marketplace-bundles.md` decision #2: the preview is
    /// advisory, checked against the real tree only here).
    #[error("{0:?} names no element the repository actually carries")]
    UnknownElement(String),
    /// A bundle whose tree, once read, carries nothing installable: no kind directory, and not
    /// the legacy dashboard-at-the-root shape either.
    #[error("{0} carries nothing installable — no kind directory, and no dashboard at its root")]
    EmptyBundle(String),
    /// Nothing has been installed from `<marketplace>/<slug>` yet, so there is no ledger to
    /// update, grow, or remove an element from. Never fired for the legacy single-dashboard
    /// bundle on purpose (decision #6): it writes no ledger at all, and keeps updating through
    /// `install::update` by its own dashboard id instead.
    #[error("nothing installed from {0}/{1} — install it first")]
    BundleNotInstalled(String, String),
    /// An address named one element of a bundle, but nothing under that coordinate is in the
    /// ledger — never installed, or already removed.
    #[error("{0:?} names no element this machine has installed from that bundle")]
    ElementNotInstalled(String),
    /// A per-kind store refused an uninstall, an update, or a start for a reason of its own —
    /// carried verbatim rather than translated, because every store already phrases its own
    /// refusals for the person reading them.
    #[error("{0}")]
    Store(String),
    /// An embedding backend was asked to be uninstalled while a consumer still resolves through
    /// it — the same refusal `/api/embeddings/backends/delete` gives, because a consumer whose
    /// assignment names nothing cannot resolve at all.
    #[error("{0}")]
    EmbeddingInUse(String),
}

/// The outcome alias every fallible operation answers with.
pub type Result<T> = std::result::Result<T, Error>;
