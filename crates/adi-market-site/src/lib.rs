//! adi-market-site — the public marketplace: plain HTML, generated from a manifest.
//!
//! The control panel already draws this listing, and draws it well (`adi-webapp`'s Marketplace
//! screen). But it is a wasm application served from somebody's own machine, so **nothing outside
//! can read it**: not a person who has never installed adi, not a link in a message, not a search
//! engine. An app store whose shelf is invisible until you have the thing it sells is not a shelf.
//!
//! So this crate takes the same manifest the installer reads and writes it out as files:
//!
//! ```text
//! index.html                  the shelf — everything published, grouped by marketplace
//! <marketplace>/<slug>/       one page per item, at the address it installs by
//! get/                        how to get adi, for the reader who has not got it
//! sitemap.xml  robots.txt     so it can be crawled
//! site.css  site.js  fonts/   the design system, once, for the whole site
//! ```
//!
//! Three decisions shape all of it:
//!
//! * **Static, and complete without script.** Every page is complete in its first response: a
//!   crawler that runs nothing still reads the name, the description, the contents, the commands
//!   and the structured data. The one script ([`get`]) decides only which of two answers already
//!   in the markup is in front. The output is a directory anyone can host — a static bucket, a
//!   git-hosted page, the front door — with no runtime to keep alive.
//! * **One schema.** The types are `adi-marketplace`'s own, so the page cannot describe a bundle
//!   the installer would refuse, and a field added to the manifest is a field this site can show
//!   rather than a second struct to remember.
//! * **Everything is somebody else's text.** A manifest is published by whoever publishes it, and
//!   this crate renders it into markup. [`html::escape`] is the whole defence, and the readme goes
//!   through [`markdown`] rather than near an `innerHTML`.
//!
//! Generating a site needs no store and no network: the input is a manifest file, which is what a
//! publisher has in the repository the manifest lives in.

use std::fs;
use std::io;
use std::path::Path;

use adi_marketplace::{BundleEntry, MarketplaceManifest};

pub mod assets;
mod entry;
mod get;
mod html;
mod icons;
mod item;
pub mod links;
mod markdown;
pub mod seo;
pub mod serve;
mod shelf;
mod shell;

/// The site as a whole — the two things that are not in any manifest.
#[derive(Debug, Clone)]
pub struct Site {
    /// What this marketplace is called on its own pages, in `og:site_name` and in the breadcrumb.
    pub name: String,
    /// Where the site will be published, with no trailing slash — the base every canonical URL,
    /// the sitemap and the structured data is built from. Empty builds a site with no absolute
    /// addresses in it, which renders and browses correctly but should not be published: a page
    /// with no canonical URL is one a search engine is free to guess at.
    pub base_url: String,
}

impl Site {
    /// An absolute URL for a path that starts with `/`, or nothing when the site has no base.
    #[must_use]
    pub fn url(&self, path: &str) -> String {
        if self.base_url.is_empty() {
            return String::new();
        }
        format!("{}{path}", self.base_url)
    }

    /// One item's absolute URL. The path is its install address — see [`entry::address`].
    #[must_use]
    pub fn item_url(&self, source: &str, entry: &BundleEntry) -> String {
        self.url(&format!("/{}/", entry::address(source, entry)))
    }
}

/// One marketplace the site publishes.
#[derive(Debug, Clone)]
pub struct Source {
    /// The name this marketplace is addressed by — the first half of `<marketplace>/<slug>`, the
    /// directory its items live in on the site, and the name an operator would add it under.
    pub name: String,
    /// Where the manifest itself is published, when the site is being built by somebody who knows.
    /// It is what makes `marketplace add` printable, so a reader with adi already can follow the
    /// listing they are looking at. A site built from a local file for review has none.
    pub url: Option<String>,
    /// The manifest, parsed and validated.
    pub manifest: MarketplaceManifest,
}

impl Source {
    /// What to call this marketplace on the page: the publisher's own name for it, else the name
    /// it is addressed by.
    #[must_use]
    pub fn title(&self) -> &str {
        self.manifest
            .name
            .as_deref()
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .unwrap_or(&self.name)
    }

    /// Read and validate a manifest from disk.
    ///
    /// Parsed by the installer's own reader, so a manifest that would be refused on a machine
    /// cannot be published as a page here — including the rule that refuses an icon or a
    /// screenshot the page would have to fetch over plaintext, and the message a manifest still
    /// in the retired artifact shape gets told.
    ///
    /// # Errors
    /// If the file cannot be read, is not JSON, or carries an entry the installer would refuse.
    pub fn read(name: &str, path: &Path, url: Option<String>) -> anyhow::Result<Self> {
        let bytes =
            fs::read(path).map_err(|e| anyhow::anyhow!("reading {}: {e}", path.display()))?;
        let manifest = adi_marketplace::parse_manifest(&bytes)
            .map_err(|e| anyhow::anyhow!("{}: {e}", path.display()))?;
        Ok(Self {
            name: name.to_string(),
            url,
            manifest,
        })
    }
}

/// One file of the generated site, at a path relative to the output directory.
#[derive(Debug, Clone)]
pub struct File {
    pub path: String,
    pub bytes: Vec<u8>,
}

impl File {
    /// A text file.
    #[must_use]
    pub fn text(path: &str, body: String) -> Self {
        Self {
            path: path.to_string(),
            bytes: body.into_bytes(),
        }
    }
}

/// The whole site.
#[must_use]
pub fn render(site: &Site, sources: &[Source]) -> Vec<File> {
    let mut files = vec![
        File::text("index.html", shelf::render(site, sources)),
        File::text("get/index.html", get::render(site, sources)),
    ];
    for source in sources {
        for entry in &source.manifest.bundles {
            files.push(File::text(
                &format!("{}/index.html", entry::address(&source.name, entry)),
                item::render(site, source, entry),
            ));
        }
    }
    files.push(File::text("robots.txt", seo::robots(site)));
    if let Some(sitemap) = seo::sitemap(site, sources) {
        files.push(File::text("sitemap.xml", sitemap));
    }
    files.extend(assets::files());
    files
}

/// Write a rendered site into `dir`, creating what it needs.
///
/// Files are written over rather than the directory cleared: the output may be a checkout with a
/// `CNAME`, a `.nojekyll` or a `.git` in it, and a generator that deleted those would be a
/// generator nobody points at their publishing directory twice.
///
/// # Errors
/// Whatever the filesystem says.
pub fn write(dir: &Path, files: &[File]) -> io::Result<()> {
    for file in files {
        let path = dir.join(&file.path);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(path, &file.bytes)?;
    }
    Ok(())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use adi_marketplace::{Element, Media, MediaKind};

    /// A site with a base URL, so the absolute addresses every page owes a crawler are checkable.
    pub(crate) fn fixture_site() -> Site {
        Site {
            name: "ADI marketplace".to_string(),
            base_url: "https://market.example.com".to_string(),
        }
    }

    /// Two entries that between them exercise the page: one that publishes everything (elements of
    /// four kinds, a gallery with a clip in it, a readme that tries to inject markup), and one that
    /// publishes the bare minimum the schema requires.
    pub(crate) fn fixture_source() -> Source {
        let full = BundleEntry {
            slug: "crm-suite".to_string(),
            name: "CRM suite".to_string(),
            description: Some("Who has gone quiet, and what was last said to them.".to_string()),
            icon: Some("https://example.com/crm.png".to_string()),
            keywords: vec!["sales".to_string(), "contacts".to_string()],
            readme: Some(
                "## What it is\n\nA follow-up agent and <script>alert(1)</script> the board \
                 that watches it.\n\n- one\n- two\n"
                    .to_string(),
            ),
            gallery: vec![
                Media {
                    url: "https://example.com/list.png".to_string(),
                    kind: None,
                    caption: Some("The list, oldest silence first".to_string()),
                    poster: None,
                },
                Media {
                    url: "https://example.com/tour.mp4".to_string(),
                    kind: Some(MediaKind::Video),
                    caption: None,
                    poster: Some("https://example.com/tour.png".to_string()),
                },
            ],
            version: Some("0.2.0".to_string()),
            repo: "https://github.com/adi-family/crm-suite.git".to_string(),
            commit: "9f2c1d4e5a6b7c8d9e0f1a2b3c4d5e6f70819a2b".to_string(),
            branch: Some("main".to_string()),
            elements: [
                ("dashboard", "crm-board"),
                ("agent", "sales-bot"),
                ("llm", "gpt5"),
                ("tool", "csv-import"),
            ]
            .into_iter()
            .map(|(kind, name)| Element {
                kind: kind.to_string(),
                name: name.to_string(),
                description: None,
            })
            .collect(),
        };
        let plain = BundleEntry {
            slug: "plain".to_string(),
            name: "Plain bundle".to_string(),
            description: None,
            icon: None,
            keywords: Vec::new(),
            readme: None,
            gallery: Vec::new(),
            version: None,
            repo: "https://github.com/adi-family/plain.git".to_string(),
            commit: "9f2c1d4e5a6b7c8d9e0f1a2b3c4d5e6f70819a2b".to_string(),
            branch: None,
            elements: Vec::new(),
        };
        let manifest = MarketplaceManifest {
            name: Some("Local bundles".to_string()),
            bundles: vec![full, plain],
        };
        manifest.validate().expect("the fixture is a legal manifest");
        Source {
            name: "local".to_string(),
            url: Some("https://example.com/m.json".to_string()),
            manifest,
        }
    }

    #[test]
    fn a_site_is_a_shelf_a_page_per_entry_and_the_files_a_crawler_asks_for() {
        let files = render(&fixture_site(), &[fixture_source()]);
        let paths: Vec<&str> = files.iter().map(|f| f.path.as_str()).collect();
        assert!(paths.contains(&"index.html"));
        assert!(paths.contains(&"local/crm-suite/index.html"));
        assert!(paths.contains(&"local/plain/index.html"));
        assert!(paths.contains(&"sitemap.xml"));
        assert!(paths.contains(&"robots.txt"));
        assert!(paths.contains(&"site.css"));
    }

    /// A site with nowhere to be published yet still renders — and quietly leaves out the two
    /// files that would otherwise carry a made-up address.
    #[test]
    fn a_site_with_no_base_url_publishes_no_absolute_addresses() {
        let site = Site {
            name: "ADI marketplace".to_string(),
            base_url: String::new(),
        };
        let files = render(&site, &[fixture_source()]);
        assert!(!files.iter().any(|f| f.path == "sitemap.xml"));
        let shelf = files
            .iter()
            .find(|f| f.path == "index.html")
            .expect("the shelf");
        let html = String::from_utf8(shelf.bytes.clone()).expect("utf-8");
        assert!(!html.contains("rel=\"canonical\""), "{html}");
    }
}
