//! Sync: fetch every configured source's manifest, cache it, and say what happened.
//!
//! The contract that matters: **a failing URL degrades to the stale cache with a warning, not an
//! error.** A marketplace that is unreachable is a listing problem, not data loss, so sync keeps
//! the last good manifest standing and records why the newest attempt failed beside it — the
//! panel shows the stale copy with the reason, and the next successful fetch clears it. The one
//! case that is still an error is a first fetch that fails with nothing to fall back on: there is
//! no honest way to report that as success.

use crate::Marketplace;
use crate::cache::{self, Envelope};
use crate::error::Result;
use crate::fetch;
use crate::manifest;
use crate::sources::{self, Source};
use adi_config::Config;

/// One source's sync outcome.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncResult {
    /// The source this happened to.
    pub source: Source,
    /// What happened.
    pub status: SyncStatus,
}

/// What one sync of one source did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SyncStatus {
    /// Fetched, validated, cached. Carries how many apps the manifest lists.
    Synced { apps: usize },
    /// The fetch failed and the stale cache stands in for it. Carries why, and how many apps the
    /// stale copy still lists.
    Stale {
        error: String,
        apps: usize,
        /// When the standing copy was fetched, if it ever was.
        fetched_at: Option<u64>,
    },
    /// The fetch failed and there was no cache to fall back on.
    Failed { error: String },
}

impl SyncResult {
    /// Whether this source's listing can be shown at all after the sync.
    #[must_use]
    pub fn has_listing(&self) -> bool {
        !matches!(self.status, SyncStatus::Failed { .. })
    }

    /// When the standing copy was fetched, if this is a stale outcome; the test-side window into
    /// [`SyncStatus::Stale`].
    #[must_use]
    pub fn stale_since(&self) -> Option<u64> {
        match &self.status {
            SyncStatus::Stale { fetched_at, .. } => *fetched_at,
            _ => None,
        }
    }

    /// One line for a terminal: what happened to this source, and why if it went wrong.
    #[must_use]
    pub fn summary(&self) -> String {
        let name = &self.source.name;
        match &self.status {
            SyncStatus::Synced { apps } => format!("{name}: {apps} app(s), cached"),
            SyncStatus::Stale {
                error, fetched_at, ..
            } => match fetched_at {
                Some(at) => format!("{name}: fetch failed ({error}) — keeping the copy from {at}"),
                None => format!("{name}: fetch failed ({error}) — keeping the stale copy"),
            },
            SyncStatus::Failed { error } => format!("{name}: fetch failed ({error}) — no cache"),
        }
    }
}

/// Sync every source over HTTPS.
///
/// # Errors
/// [`crate::Error`] never for a fetch failure alone — those become [`SyncStatus::Stale`] /
/// [`SyncStatus::Failed`] per source. The call answers `Err` only when the sources themselves
/// cannot be read.
pub fn sync(market: &Marketplace) -> Result<Vec<SyncResult>> {
    sync_with(market, fetch::get)
}

/// Sync every source through a caller-supplied fetch — the seam the tests drive, so the
/// degrade-to-stale contract is pinned without a network.
///
/// # Errors
/// [`crate::Error`] only when the sources cannot be read.
pub fn sync_with(
    market: &Marketplace,
    fetch: impl Fn(&str) -> std::result::Result<Vec<u8>, String>,
) -> Result<Vec<SyncResult>> {
    let config = market.config();
    Ok(sources::list(config)?
        .into_iter()
        .map(|source| sync_one(config, &source, &fetch))
        .collect())
}

/// Sync one source: fetch, validate, cache — or record why not, keeping what stands.
fn sync_one(
    config: &Config,
    source: &Source,
    fetch: &impl Fn(&str) -> std::result::Result<Vec<u8>, String>,
) -> SyncResult {
    let status = match fetch(&source.url)
        .and_then(|bytes| manifest::parse(&bytes).map_err(|e| e.to_string()))
    {
        Ok(fresh) => {
            let apps = fresh.apps.len();
            let cached = cache::write(
                config,
                &source.name,
                &Envelope {
                    url: source.url.clone(),
                    fetched_at: Some(adi_config::now_unix()),
                    error: None,
                    manifest: Some(fresh),
                },
            );
            // A fetch that cannot be cached has not synced anything the listing will ever see.
            uncached_on_write_failure(cached, SyncStatus::Synced { apps })
        }
        Err(error) => {
            // Keep the standing copy, but record this attempt's failure beside it — a listing
            // rendered from the cache must be able to say "this is from Tuesday, and the fetch
            // since has failed", not just look current.
            let standing = cache::read(config, &source.name);
            let (manifest, fetched_at) = match &standing {
                Some(envelope) if envelope.url == source.url => {
                    (envelope.manifest.clone(), envelope.fetched_at)
                }
                _ => (None, None),
            };
            match manifest {
                Some(manifest) => {
                    let apps = manifest.apps.len();
                    let cached = cache::write(
                        config,
                        &source.name,
                        &Envelope {
                            url: source.url.clone(),
                            fetched_at,
                            error: Some(error.clone()),
                            manifest: Some(manifest),
                        },
                    );
                    // The standing copy still serves the listing even when this attempt's note
                    // could not be recorded beside it.
                    uncached_on_write_failure(
                        cached,
                        SyncStatus::Stale {
                            error,
                            apps,
                            fetched_at,
                        },
                    )
                }
                None => SyncStatus::Failed { error },
            }
        }
    };
    SyncResult {
        source: source.clone(),
        status,
    }
}

/// Swap an outcome for a failure naming the cache write that did not happen.
fn uncached_on_write_failure(written: crate::Result<()>, outcome: SyncStatus) -> SyncStatus {
    match written {
        Ok(()) => outcome,
        Err(e) => SyncStatus::Failed {
            error: format!("could not cache the fetch: {e}"),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A manifest small enough to read, with the two fields an install keys on.
    const MANIFEST: &str = r#"{"name":"ADI starter apps","apps":[
        {"slug":"crm","name":"CRM","version":"0.1.0",
         "artifact":"https://example/crm.bundle.json"},
        {"slug":"nosh","name":"Nosh",
         "artifact":"https://example/nosh.bundle.json"}
    ]}"#;

    #[test]
    fn a_good_fetch_is_cached_and_counted() {
        let market = crate::tests::scratch("good");
        sources::add(market.config(), "adi", "https://example/m.json").expect("add");

        let results = sync_with(&market, |_| Ok(MANIFEST.as_bytes().to_vec())).expect("sync");
        assert_eq!(results.len(), 1);
        assert_eq!(
            results[0].status,
            SyncStatus::Synced { apps: 2 },
            "{}",
            results[0].summary()
        );

        // The cache holds the manifest, and a listing can be built from it.
        let states = cache::source_states(market.config());
        assert_eq!(states.len(), 1);
        assert!(states[0].synced_at.is_some(), "{states:?}");
        assert_eq!(states[0].error, None);
        assert_eq!(states[0].name, "adi");
        let _ = std::fs::remove_dir_all(market.config().root());
    }

    #[test]
    fn a_failing_url_degrades_to_the_stale_cache_not_an_error() {
        let market = crate::tests::scratch("stale");
        sources::add(market.config(), "adi", "https://example/m.json").expect("add");

        // First fetch works.
        let results = sync_with(&market, |_| Ok(MANIFEST.as_bytes().to_vec())).expect("sync");
        assert_eq!(results[0].status, SyncStatus::Synced { apps: 2 });

        let results = sync_with(&market, |_| Err("dns went away".to_string())).expect("sync");
        match &results[0].status {
            SyncStatus::Stale {
                error,
                apps,
                fetched_at,
            } => {
                assert_eq!(error, "dns went away");
                assert_eq!(*apps, 2, "the stale copy still lists its apps");
                assert!(
                    fetched_at.is_some(),
                    "the standing copy's timestamp is carried"
                );
            }
            other => panic!("expected a stale outcome, got {other:?}"),
        }
        assert!(results[0].has_listing());

        // And the state the panel renders says both things at once: stale, and why.
        let states = cache::source_states(market.config());
        assert_eq!(
            states[0].error.as_deref(),
            Some("dns went away"),
            "{states:?}"
        );
        assert!(states[0].synced_at.is_some());

        // The next good fetch clears the error.
        let results = sync_with(&market, |_| Ok(MANIFEST.as_bytes().to_vec())).expect("sync");
        assert_eq!(results[0].status, SyncStatus::Synced { apps: 2 });
        assert_eq!(cache::source_states(market.config())[0].error, None);
        let _ = std::fs::remove_dir_all(market.config().root());
    }

    #[test]
    fn a_first_fetch_that_fails_has_nothing_to_degrade_to() {
        let market = crate::tests::scratch("failed");
        sources::add(market.config(), "adi", "https://example/m.json").expect("add");

        let results = sync_with(&market, |_| Err("connection refused".to_string())).expect("sync");
        assert_eq!(
            results[0].status,
            SyncStatus::Failed {
                error: "connection refused".to_string()
            }
        );
        assert!(!results[0].has_listing());
        let _ = std::fs::remove_dir_all(market.config().root());
    }

    #[test]
    fn a_manifest_that_validates_badly_counts_as_a_failed_fetch() {
        let market = crate::tests::scratch("bad-manifest");
        sources::add(market.config(), "adi", "https://example/m.json").expect("add");

        let broken = br#"{"apps":[{"slug":"../evil","name":"X","artifact":"https://e/x.json"}]}"#;
        let results = sync_with(&market, |_| Ok(broken.to_vec())).expect("sync");
        assert!(matches!(results[0].status, SyncStatus::Failed { .. }));
        let _ = std::fs::remove_dir_all(market.config().root());
    }
}
