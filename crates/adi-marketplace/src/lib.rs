//! adi-marketplace — apps from a manifest you host anywhere, installed as git clones.
//!
//! A marketplace is one JSON manifest at an HTTPS URL the operator chose (GitHub raw, a gist, any
//! host); the store keeps an *array* of them, each URL one source. No platform, no hosting, no
//! accounts — the same shape plugins ship in, which is evidence the shape is sufficient rather
//! than merely cheap. The full reasoning is the operator's decision of 2026-08-31
//! (`business/decisions/2026-08-31-marketplaces-are-a-manifest-in-a-git-repo.md`); the schema and
//! every deliberate limit is written out in `docs/marketplace.md`.
//!
//! **An app is a git repository at a pinned commit, and installing one is cloning it.** Three
//! properties follow from that, and all three are the point:
//!
//! * **The pin is what you get.** A manifest names a full 40-hex commit; an install stands at
//!   that commit or fails. A publisher who pushes something else after the operator read the
//!   listing changes nothing about what installs, and moving onto a newer pin is
//!   [`install::update`] — an act somebody takes, with the diff already public.
//! * **The operator names their copy.** The slug is the published identity; the directory, the id
//!   and the hostname come from the name the person typed, renameable afterwards like any
//!   dashboard's. Installing the same app twice is ordinary.
//! * **It stays a clone.** `.git` is kept and the pin sits on a branch that tracks `origin`, so
//!   the app can be read, edited, committed to and pulled — the update path is git's, not a
//!   format of ours.
//!
//! **Installed is not started.** An arriving app lands in the dashboard store's own inert state:
//! files on disk, `archived_at` stamped, and its hive file parked under the name the supervisor's
//! glob does not match — so nothing executes until somebody starts it ([`install::start`], or
//! Restore on the Dashboards page). That is the property the store already gives a tool
//! ("creating a tool gives it to nobody"), kept for a payload whose backend is somebody else's
//! TypeScript.
//!
//! The store layout:
//!
//! ```text
//! marketplace/sources.toml        # [[marketplaces]] — name + url, one per source
//! marketplace/cache/<name>.json   # the last fetch: when, from where, any error, the manifest
//! marketplace/staging/<id>/       # a clone being assembled, outside the supervisor's glob
//! ```
//!
//! Sync degrades rather than fails: a URL that cannot be fetched leaves the stale cache standing
//! with a warning recorded beside it, because a marketplace that is unreachable is a listing
//! problem, not a data-loss problem.

mod cache;
mod error;
mod fetch;
pub mod git;
pub mod install;
mod manifest;
pub mod sources;
pub mod sync;

pub use cache::{SourceState, source_states};
pub use error::{Error, Result};
pub use install::{AppInstall, CachedApp, InstallRecord, Installed, Started, Updated};
pub use manifest::{AppEntry, MarketplaceManifest};

use adi_config::Config;

/// The store module everything marketplace lives under.
const MODULE: &str = "marketplace";

/// The handle: the store's [`Config`], nothing more. Cheap to build; every operation opens what
/// it needs from disk, so two handles in one process cannot disagree about state the way a cached
/// listing would.
#[derive(Debug, Clone)]
pub struct Marketplace {
    config: Config,
}

impl Marketplace {
    /// Open the standard store's marketplace module.
    #[must_use]
    pub fn open() -> Self {
        Self {
            config: Config::open(),
        }
    }

    /// A marketplace rooted at an arbitrary store — tests, alternate installs.
    #[must_use]
    pub fn with_config(config: Config) -> Self {
        Self { config }
    }

    /// The store this marketplace lives in.
    #[must_use]
    pub fn config(&self) -> &Config {
        &self.config
    }

    /// The dashboards root apps install into — `~/.adi/mono/dashboards/`.
    #[must_use]
    pub fn dashboards_dir(&self) -> std::path::PathBuf {
        self.config.module("dashboards").dir().to_path_buf()
    }
}

impl Default for Marketplace {
    fn default() -> Self {
        Self::open()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A store of this test's own, under the system temp dir — never the operator's live one.
    pub(crate) fn scratch(tag: &str) -> Marketplace {
        let root = std::env::temp_dir().join(format!(
            "adi-marketplace-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id(),
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("scratch root");
        Marketplace::with_config(Config::with_root(root))
    }

    #[test]
    fn the_dashboards_root_is_the_shared_module_dir() {
        let market = scratch("layout");
        assert_eq!(
            market.dashboards_dir(),
            market.config().module("dashboards").dir()
        );
        let _ = std::fs::remove_dir_all(market.config().root());
    }
}
