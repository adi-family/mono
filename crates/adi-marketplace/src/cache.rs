//! The cache: one envelope per source under `marketplace/cache/`, holding the last successful
//! fetch — when it happened, from where, what went wrong on the attempt after it, and the
//! manifest itself.
//!
//! An envelope rather than the bare manifest because "when is this from" and "is it stale" are
//! questions the listing has to answer without the network. The manifest inside stays byte-shape
//! identical to what the source served, so nothing downstream parses a wrapper by accident.

use serde::{Deserialize, Serialize};

use crate::error::Result;
use crate::manifest::MarketplaceManifest;
use crate::sources;
use adi_config::Config;

/// The cache directory within the marketplace module.
const CACHE_DIR: &str = "cache";

/// The file one source's envelope lives in.
#[must_use]
pub(crate) fn cache_file_name(name: &str) -> String {
    format!("{name}.json")
}

/// One source's last fetch, as the cache keeps it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Envelope {
    /// The URL this was fetched from — recorded so a renamed source's cache cannot be mistaken
    /// for a fresher fetch of its new URL.
    pub url: String,
    /// Unix seconds of the last *successful* fetch. `None` before there has ever been one.
    pub fetched_at: Option<u64>,
    /// Why the most recent attempt (if any) failed, while the manifest above still stands.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// The manifest itself. `None` when no fetch has ever succeeded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub manifest: Option<MarketplaceManifest>,
}

/// Read one source's envelope; a source never synced (or removed) reads as `None`.
#[must_use]
pub(crate) fn read(config: &Config, name: &str) -> Option<Envelope> {
    let bytes = config
        .module(crate::MODULE)
        .read_raw(&format!("{CACHE_DIR}/{}", cache_file_name(name)))
        .ok()??;
    serde_json::from_slice(&bytes).ok()
}

/// Write one source's envelope atomically.
///
/// # Errors
/// [`crate::Error::Config`] on a write failure.
pub(crate) fn write(config: &Config, name: &str, envelope: &Envelope) -> Result<()> {
    let bytes = serde_json::to_vec_pretty(envelope)
        .map_err(|e| crate::Error::Fetch(format!("encoding the cache: {e}")))?;
    config
        .module(crate::MODULE)
        .write_raw(&format!("{CACHE_DIR}/{}", cache_file_name(name)), &bytes)?;
    Ok(())
}

/// What the panel needs to say about a source: where it points, and whether what it shows is
/// fresh or a stale copy standing in for an unreachable fetch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceState {
    /// The local name the source was added under.
    pub name: String,
    /// The manifest URL.
    pub url: String,
    /// Unix seconds of the last successful fetch, or `None` if it never succeeded.
    pub synced_at: Option<u64>,
    /// Why the last attempt failed, when it did and a stale copy is being shown.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Every configured source with its cache's health — the state a listing is rendered from.
/// Sources with no cache yet still appear, saying so.
#[must_use]
pub fn source_states(config: &Config) -> Vec<SourceState> {
    sources::list(config)
        .unwrap_or_default()
        .into_iter()
        .map(|source| {
            let envelope = read(config, &source.name);
            SourceState {
                name: source.name.clone(),
                url: source.url.clone(),
                // An envelope recorded for another URL is not a fetch of this one — a renamed
                // source starts stale rather than inheriting a timestamp it did not earn.
                synced_at: envelope
                    .as_ref()
                    .filter(|e| e.url == source.url)
                    .and_then(|e| e.fetched_at),
                error: envelope
                    .as_ref()
                    .filter(|e| e.url == source.url)
                    .and_then(|e| e.error.clone()),
            }
        })
        .collect()
}
