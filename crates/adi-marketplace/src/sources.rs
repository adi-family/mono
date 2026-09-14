//! The store's config of sources: `marketplace/sources.toml`, an array of `[[marketplaces]]`
//! tables — one `name` + `url` per manifest the operator pointed this machine at.
//!
//! An array, not one hardcoded repo, because the operator's ruling at launch was explicit:
//! sources are MULTIPLE, each URL one JSON manifest hosted anywhere. A name is local identity
//! (what `<marketplace>/<slug>` and the cache file are keyed by), and the manifest's own `name` —
//! if it carries one — is display text only, so two operators can add the same URL under
//! different names without either being wrong.
//!
//! **The official marketplace is there to begin with** ([`OFFICIAL_NAME`]). It shipped empty until
//! 2026-09-14 and the operator reversed that: the ADI Store is ours, the apps in it are ours, and
//! asking somebody to type a raw GitHub URL before they can see any of them was a chore standing
//! in front of the product. Nothing else about the shape changes — it is one entry in the same
//! array, and `marketplace remove store` removes it like any other.
//!
//! The mechanism is deliberately the file's own **absence**: no `sources.toml` means a machine
//! that has never configured sources, and that machine gets the official one. The moment anything
//! writes the file — an add, or a remove — the file is the whole truth, so a removal sticks
//! rather than coming back on the next read.

use serde::{Deserialize, Serialize};

use crate::MODULE;
use crate::error::{Error, Result};
use adi_config::Config;

/// The file the array lives in, within the marketplace module.
const SOURCES_FILE: &str = "sources.toml";

/// The name the official marketplace is added under, and therefore the first half of every
/// address into it: `store/crm-suite`. It is the word the ADI Store's own pages print.
pub const OFFICIAL_NAME: &str = "store";

/// Where the official manifest is published. One file in a public repository — the same shape
/// every other source has, so the official one is not a special case anywhere but here.
pub const OFFICIAL_URL: &str =
    "https://raw.githubusercontent.com/adi-family/marketplace/main/apps/marketplace.json";

/// The one source a machine has before anybody configures any.
#[must_use]
pub fn official() -> Source {
    Source {
        name: OFFICIAL_NAME.to_string(),
        url: OFFICIAL_URL.to_string(),
    }
}

/// One configured marketplace: a name this machine knows it by, and the URL of its manifest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Source {
    /// Local identity: the first half of `<marketplace>/<slug>`, and the cache file's name.
    /// One safe path segment, enforced by [`add`](Sources::add).
    pub name: String,
    /// Where the manifest is fetched from. `https://`, or a `file:///` absolute path for a
    /// manifest developed locally before it is published anywhere — see [`valid_url`].
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

/// Read every configured source, in the order they were added — starting from the official one on
/// a machine where nothing has been configured yet.
///
/// # Errors
/// [`Error::Config`] if the file cannot be read or does not parse.
pub fn list(config: &Config) -> Result<Vec<Source>> {
    // The file's absence is the signal, not an empty array inside it: `[]` on disk is somebody
    // having removed everything, and putting the official source back under them would make
    // `marketplace remove` a suggestion.
    if !configured(config) {
        return Ok(vec![official()]);
    }
    Ok(SourcesFile::load(config)?.marketplaces)
}

/// Whether anything has ever written the array — see [`list`] for why that is the question.
fn configured(config: &Config) -> bool {
    config
        .module(MODULE)
        .file::<SourcesFile>(SOURCES_FILE)
        .path()
        .exists()
}

/// Add a source: validate the name and the URL, refuse a duplicate name, persist.
///
/// # Errors
/// [`Error::InvalidName`] for a name that is not one safe path segment, [`Error::NotHttps`] for a
/// URL that is neither `https://` nor a `file:///` absolute path, [`Error::Duplicate`] when the
/// name is taken, plus [`Error::Config`] on a write failure.
pub fn add(config: &Config, name: &str, url: &str) -> Result<Source> {
    let name = name.trim();
    if !adi_config::valid_name(name) {
        return Err(Error::InvalidName(name.to_string()));
    }
    let url = url.trim();
    if !valid_url(url) {
        return Err(Error::NotHttps(url.to_string()));
    }
    // Through `list`, so the first `add` on a fresh machine writes the official source down
    // beside the new one instead of quietly replacing it with it.
    let mut marketplaces = list(config)?;
    if marketplaces.iter().any(|s| s.name == name) {
        return Err(Error::Duplicate(name.to_string()));
    }
    let source = Source {
        name: name.to_string(),
        url: url.to_string(),
    };
    marketplaces.push(source.clone());
    SourcesFile::save(config, &SourcesFile { marketplaces })?;
    Ok(source)
}

/// Whether a source URL is one this build will fetch.
///
/// `https://` because fetching is HTTPS, always — and `file:///` because a marketplace listing a
/// bundle in development has nowhere legitimate to be listed from yet: the same carve-out
/// `BundleEntry::repo` already gets (`manifest::valid_repo`), for the same reason, and it costs
/// the same thing — a manifest URL, like a repo URL, can now point at this machine's own
/// filesystem, so a source added over `file://` is only ever as trustworthy as the path itself.
/// `file://host/path` — a URI with an authority, not a local path — is refused: it is not the
/// shape a bundle's own `repo` accepts either, and there is no host component this build reads.
fn valid_url(url: &str) -> bool {
    (url.starts_with("https://") && url.len() > "https://".len())
        || (url.starts_with("file:///") && url.len() > "file:///".len())
}

/// Remove the source named `name` and its cache with it, answering whether anything was there.
///
/// # Errors
/// [`Error::Config`] on a read or write failure.
pub fn remove(config: &Config, name: &str) -> Result<bool> {
    // Also through `list`: removing the official source on a machine that has never written the
    // file has to *write* one, or the next read would hand it straight back.
    let mut marketplaces = list(config)?;
    let before = marketplaces.len();
    marketplaces.retain(|s| s.name != name.trim());
    if marketplaces.len() == before {
        return Ok(false);
    }
    SourcesFile::save(config, &SourcesFile { marketplaces })?;
    // The cache outliving its source is a listing that renders from nowhere; gone is gone.
    let _ = config
        .module(MODULE)
        .remove_raw(&crate::cache::cache_path(name.trim()));
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

    const URL: &str = "https://raw.githubusercontent.com/adi-family/marketplace/main/apps/marketplace.json";

    /// A machine nobody has configured has the official marketplace and nothing else — that is
    /// the whole of "it is added by default".
    #[test]
    fn a_fresh_store_has_the_official_marketplace() {
        let cfg = config("official");
        assert_eq!(list(&cfg).expect("list"), vec![official()]);
        assert_eq!(official().name, "store", "the word every address into it starts with");
        assert!(!configured(&cfg), "and nothing has been written to say so");
        let _ = std::fs::remove_dir_all(cfg.root());
    }

    /// The first `add` writes the default down beside the new one. Taking `list`'s answer as the
    /// starting point is what does it; starting from the file would drop the official source on
    /// the floor the first time anybody added a second marketplace.
    #[test]
    fn adding_a_second_marketplace_keeps_the_official_one() {
        let cfg = config("keeps");
        add(&cfg, "other", "https://other.example/m.json").expect("add");
        let names: Vec<String> = list(&cfg).expect("list").into_iter().map(|s| s.name).collect();
        assert_eq!(names, vec!["store".to_string(), "other".to_string()]);
        let _ = std::fs::remove_dir_all(cfg.root());
    }

    /// And removing it sticks: the removal writes the file, and a written file is the whole truth.
    #[test]
    fn removing_the_official_marketplace_is_final() {
        let cfg = config("final");
        assert!(remove(&cfg, "store").expect("remove"));
        assert!(list(&cfg).expect("list").is_empty(), "it does not come back on the next read");
        assert!(!remove(&cfg, "store").expect("remove again"), "and it is not there to remove twice");
        let _ = std::fs::remove_dir_all(cfg.root());
    }

    /// Removing something that was never there writes nothing, so it does not silently take the
    /// default away with it.
    #[test]
    fn removing_a_stranger_leaves_the_default_alone() {
        let cfg = config("stranger");
        assert!(!remove(&cfg, "nope").expect("remove"));
        assert_eq!(list(&cfg).expect("list"), vec![official()]);
        let _ = std::fs::remove_dir_all(cfg.root());
    }

    #[test]
    fn the_store_writes_the_documented_shape() {
        let cfg = config("shape");
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
            "https://",          // scheme and nothing else
            "file://",           // scheme and nothing else
            "file:///",          // no path past the third slash
            "file://host/etc/x", // an authority, not a local path
        ] {
            assert!(
                matches!(add(&cfg, "other", bad), Err(Error::NotHttps(_))),
                "{bad}"
            );
        }
        // A local manifest, mid-development — the same carve-out `repo` gets.
        add(&cfg, "local", "file:///tmp/sample/marketplace.json").expect("file:// admitted");
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
        // A cache file exactly where a sync leaves one — `cache/<name>.json`, not `<name>.json`.
        // Seeded through the same helper the reader uses, because a test that spells the path
        // itself is a test that can agree with a bug: this one passed for months while `remove`
        // deleted a file nothing ever wrote, leaving the real cache standing.
        crate::cache::write(
            &cfg,
            "adi",
            &crate::cache::Envelope {
                url: URL.to_string(),
                fetched_at: Some(1),
                error: None,
                manifest: None,
            },
        )
        .expect("cache");
        assert!(
            cfg.module(MODULE)
                .raw_path(&crate::cache::cache_path("adi"))
                .exists(),
            "precondition: the cache is on disk"
        );

        assert!(remove(&cfg, "adi").expect("remove"));
        assert_eq!(
            list(&cfg).expect("list"),
            vec![official()],
            "the one that was added is gone; the default it was added beside is not"
        );
        assert!(
            !cfg.module(MODULE)
                .raw_path(&crate::cache::cache_path("adi"))
                .exists(),
            "a removed marketplace leaves no listing behind"
        );
        assert!(!remove(&cfg, "adi").expect("remove again"));
        let _ = std::fs::remove_dir_all(cfg.root());
    }
}
