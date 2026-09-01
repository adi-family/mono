//! The store's config of sources: `marketplace/sources.toml`, an array of `[[marketplaces]]`
//! tables — one `name` + `url` per manifest the operator pointed this machine at.
//!
//! An array, not one hardcoded repo, because the operator's ruling at launch was explicit:
//! sources are MULTIPLE, each URL one JSON manifest hosted anywhere. The array ships empty; a
//! name is local identity (what `<marketplace>/<slug>` and the cache file are keyed by), and the
//! manifest's own `name` — if it carries one — is display text only, so two operators can add the
//! same URL under different names without either being wrong.

use serde::{Deserialize, Serialize};

use crate::MODULE;
use crate::error::{Error, Result};
use adi_config::Config;

/// The file the array lives in, within the marketplace module.
const SOURCES_FILE: &str = "sources.toml";

/// One configured marketplace: a name this machine knows it by, and the HTTPS URL of its manifest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Source {
    /// Local identity: the first half of `<marketplace>/<slug>`, and the cache file's name.
    /// One safe path segment, enforced by [`add`](Sources::add).
    pub name: String,
    /// Where the manifest is fetched from. Always `https://`.
    pub url: String,
}

/// The on-disk shape: a top-level `marketplaces` array, so the file reads as
/// `[[marketplaces]]` tables and an empty store is an empty array rather than a missing file.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
struct SourcesFile {
    #[serde(default, rename = "marketplaces")]
    marketplaces: Vec<Source>,
}

impl SourcesFile {
    /// Load the store's sources; a store nothing was added to yet reads as empty.
    fn load(config: &Config) -> Result<Self> {
        config
            .module(MODULE)
            .file::<Self>(SOURCES_FILE)
            .load_or_default()
            .map_err(Into::into)
    }

    fn save(config: &Config, sources: &Self) -> Result<()> {
        config
            .module(MODULE)
            .file::<Self>(SOURCES_FILE)
            .save(sources)
            .map_err(Into::into)
    }
}

/// Read every configured source, in the order they were added.
///
/// # Errors
/// [`Error::Config`] if the file cannot be read or does not parse.
pub fn list(config: &Config) -> Result<Vec<Source>> {
    Ok(SourcesFile::load(config)?.marketplaces)
}

/// Add a source: validate the name and the URL, refuse a duplicate name, persist.
///
/// # Errors
/// [`Error::InvalidName`] for a name that is not one safe path segment, [`Error::NotHttps`] for a
/// URL that does not spell `https://`, [`Error::Duplicate`] when the name is taken, plus
/// [`Error::Config`] on a write failure.
pub fn add(config: &Config, name: &str, url: &str) -> Result<Source> {
    let name = name.trim();
    if !adi_config::valid_name(name) {
        return Err(Error::InvalidName(name.to_string()));
    }
    let url = url.trim();
    if !url.starts_with("https://") || url.len() <= "https://".len() {
        return Err(Error::NotHttps(url.to_string()));
    }
    let mut file = SourcesFile::load(config)?;
    if file.marketplaces.iter().any(|s| s.name == name) {
        return Err(Error::Duplicate(name.to_string()));
    }
    let source = Source {
        name: name.to_string(),
        url: url.to_string(),
    };
    file.marketplaces.push(source.clone());
    SourcesFile::save(config, &file)?;
    Ok(source)
}

/// Remove the source named `name` and its cache with it, answering whether anything was there.
///
/// # Errors
/// [`Error::Config`] on a read or write failure.
pub fn remove(config: &Config, name: &str) -> Result<bool> {
    let mut file = SourcesFile::load(config)?;
    let before = file.marketplaces.len();
    file.marketplaces.retain(|s| s.name != name.trim());
    if file.marketplaces.len() == before {
        return Ok(false);
    }
    SourcesFile::save(config, &file)?;
    // The cache outliving its source is a listing that renders from nowhere; gone is gone.
    let _ = config
        .module(MODULE)
        .remove_raw(&crate::cache::cache_file_name(name.trim()));
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(tag: &str) -> Config {
        let root = std::env::temp_dir().join(format!(
            "adi-marketplace-sources-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id(),
        ));
        let _ = std::fs::remove_dir_all(&root);
        Config::with_root(root)
    }

    const URL: &str = "https://raw.githubusercontent.com/adi-family/apps/main/marketplace.json";

    #[test]
    fn the_store_starts_empty_and_writes_the_documented_shape() {
        let cfg = config("shape");
        assert!(list(&cfg).expect("list").is_empty());

        add(&cfg, "adi", URL).expect("add");
        let raw =
            std::fs::read_to_string(cfg.module(MODULE).file::<SourcesFile>(SOURCES_FILE).path())
                .expect("read");
        assert!(raw.contains("[[marketplaces]]"), "{raw}");
        assert!(raw.contains("name = \"adi\""), "{raw}");
        assert!(raw.contains(&format!("url = \"{URL}\"")), "{raw}");
        let _ = std::fs::remove_dir_all(cfg.root());
    }

    #[test]
    fn add_validates_the_name_the_url_and_duplicates() {
        let cfg = config("validate");
        add(&cfg, "adi", URL).expect("add");

        for bad in ["", ".", "..", "a/b", "with space"] {
            assert!(
                matches!(add(&cfg, bad, URL), Err(Error::InvalidName(_))),
                "{bad:?}"
            );
        }
        for bad in [
            "http://insecure.example/manifest.json",
            "ftp://example/manifest.json",
            "https://", // scheme and nothing else
        ] {
            assert!(
                matches!(add(&cfg, "other", bad), Err(Error::NotHttps(_))),
                "{bad}"
            );
        }
        assert!(matches!(
            add(&cfg, "adi", "https://other.example/m.json"),
            Err(Error::Duplicate(_))
        ));
        // The same URL under a second name is a second view of one manifest, not a conflict.
        add(&cfg, "apps", URL).expect("same url, other name");
        let _ = std::fs::remove_dir_all(cfg.root());
    }

    #[test]
    fn remove_drops_the_source_and_its_cache_and_reports_a_miss() {
        let cfg = config("remove");
        add(&cfg, "adi", URL).expect("add");
        // A cache file for the source, as a sync would have left it.
        cfg.module(MODULE)
            .write_raw(
                &crate::cache::cache_file_name("adi"),
                b"{\"url\":\"x\",\"manifest\":null}",
            )
            .expect("cache");

        assert!(remove(&cfg, "adi").expect("remove"));
        assert!(list(&cfg).expect("list").is_empty());
        assert!(
            !cfg.module(MODULE)
                .raw_path(&crate::cache::cache_file_name("adi"))
                .exists()
        );
        assert!(!remove(&cfg, "adi").expect("remove again"));
        let _ = std::fs::remove_dir_all(cfg.root());
    }
}
