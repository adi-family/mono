//! The marketplace manifest: one JSON document, a list of bundles.
//!
//! An entry names **a git repository and one commit in it**. That pair is the whole contract:
//! what installs is a clone standing at that commit, so a publisher who pushes something else
//! tomorrow changes nothing about what the operator already read, and moving onto a newer commit
//! is an act somebody takes on purpose. A manifest that named a branch — or an artifact behind a
//! URL — would be a listing whose meaning changes under the reader.
//!
//! v1 shipped exactly one kind of item, an app, which is a dashboard. Every marketplace item is
//! now **a bundle** — a named collection of platform elements from one repository — and an app is
//! the special case of a bundle whose only element is a dashboard, which is exactly what v1
//! shipped, byte for byte (`docs/marketplace-bundles.md`). The wire shape barely moves: the array
//! is renamed `bundles` (an old manifest's `apps` key is read as an alias for the same array, so
//! nothing published for v1 needs to change), and one field is added, a preview of what a bundle
//! carries.
//!
//! Unknown fields are ignored, so a v2 manifest an older machine reads still lists its bundles
//! rather than failing whole.
//!
//! ```json
//! {
//!   "name": "ADI starter apps",
//!   "bundles": [
//!     {
//!       "slug": "crm-suite",
//!       "name": "CRM suite",
//!       "description": "A follow-up agent, its import tool, and the dashboard that watches both.",
//!       "repo": "https://github.com/adi-family/crm-suite.git",
//!       "commit": "9f2c1d4e5a6b7c8d9e0f1a2b3c4d5e6f70819a2b",
//!       "elements": [
//!         { "kind": "agent", "name": "sales-bot", "description": "Drafts the follow-up." },
//!         { "kind": "tool", "name": "csv-import", "description": "Loads a contacts export." },
//!         { "kind": "dashboard", "name": "crm", "description": "The list itself." }
//!       ]
//!     }
//!   ]
//! }
//! ```

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::kind::Kind;

/// The length of a full git object name, which is the only length a pin may be: a short sha is
/// ambiguous by construction, and ambiguity is the thing pinning exists to remove.
const SHA_LEN: usize = 40;

/// One marketplace's manifest, as hosted at its URL.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MarketplaceManifest {
    /// The publisher's own name for the marketplace. Display text only — the identity on this
    /// machine is the name the source was added under, so two operators can alias one URL.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// The bundles. An empty list is a valid marketplace (a starter repo with nothing in it yet);
    /// what is never valid is an entry that fails [`BundleEntry::validate`].
    ///
    /// `#[serde(alias = "apps")]` is the whole of v1's migration: an old manifest published under
    /// the retired key still parses, unchanged, into the same field.
    #[serde(default, alias = "apps")]
    pub bundles: Vec<BundleEntry>,
}

/// One bundle in a marketplace: the listing text, and the repository and commit it installs from.
///
/// Every field here is v1's own, meaning exactly what it always did — an app is still the special
/// case of a bundle whose repository carries nothing but a dashboard. [`BundleEntry::elements`]
/// is the one addition.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BundleEntry {
    /// The published identity: the second half of `<marketplace>/<slug>`, which is what an
    /// install is addressed by. **Not** the directory it lands as — that is minted from the name
    /// the operator chooses. One safe path segment.
    pub slug: String,
    /// The human name, offered as the default when the operator names their copy.
    pub name: String,
    /// One line on what the app is for.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// The app's mark: an `https://` URL, or a `data:image/…` URI for a publisher who would
    /// rather the listing fetched nothing from anywhere. Display only — it is drawn beside the
    /// entry and nothing is decided by it. Absent is ordinary, and reads as a placeholder glyph
    /// rather than a hole.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    /// What the app is about, in the publisher's own words — a word or two each, drawn as tags.
    ///
    /// Free text on purpose. A controlled vocabulary would need somebody to keep it and somebody
    /// to petition for a new term, which is the platform this marketplace deliberately does not
    /// have: a manifest is a file its publisher owns.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub keywords: Vec<String>,
    /// The long form, in Markdown: what the app does, what it needs, what it does not do.
    ///
    /// Carried in the manifest rather than fetched from the repository, for the reason the icon
    /// may be a `data:` URI: the manifest has already been fetched, so this costs no further
    /// request and reads with the network gone. The panel renders a small subset
    /// ([`adi_ui::Markdown`], through Leptos views rather than `innerHTML`), so a document cannot
    /// inject markup however it is written.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub readme: Option<String>,
    /// Pictures and clips of the app in use, in the order they should be shown.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub gallery: Vec<Media>,
    /// The app's version, as its publisher wrote it. Display text: the commit is the identity of
    /// what installs, and this is the label a person recognizes it by.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// The repository to clone. `https://` — or a `file://` path, which is how an app is
    /// developed against a local marketplace before it is published anywhere.
    pub repo: String,
    /// The commit to stand at: a full 40-hex object name, never a branch or a tag. This is the
    /// pin, and it is why an install is repeatable.
    pub commit: String,
    /// The branch that commit sits on, when it is not the repository's default. It decides what
    /// a later `git pull` in the installed copy follows, and nothing else.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    /// A **preview** of what installing this bundle offers, for a listing page that has not
    /// cloned anything yet — exactly why `readme` and `gallery` are carried in the manifest
    /// rather than fetched from the repository. It is advisory, not authoritative: what actually
    /// installs is read off the pinned commit's own tree at install time
    /// (`crate::layout::read_layout`), and a manifest that oversells or undersells what is there
    /// is a per-element surprise at install, not a validation failure here
    /// (`docs/marketplace-bundles.md` decision #2). Absent is legal — an old-shape publisher, or
    /// one who has not written a preview yet — and installs exactly as the tree reads, without one.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub elements: Vec<Element>,
}

/// One element a bundle's manifest previews.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Element {
    /// One of `agent` | `tool` | `dashboard` | `llm` | `embedding` | `service` | `trigger` |
    /// `project` — the manifest's own, singular spelling (`crate::kind::Kind::wire`). A word this
    /// build does not recognise is not a validation failure: [`Element::kind`] reads back `None`,
    /// and [`BundleEntry::elements`] leaves it out of the preview.
    pub kind: String,
    /// The published name — the file or directory stem this element carries in the repository,
    /// and the third address coordinate (`crate::address::Address`).
    pub name: String,
    /// One line, for the listing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

impl Element {
    /// The kind this build understands `kind` as, or `None` for a word not among the eight.
    #[must_use]
    pub fn kind(&self) -> Option<Kind> {
        Kind::from_wire(self.kind.trim())
    }

    /// The published name, trimmed.
    #[must_use]
    pub fn name(&self) -> &str {
        self.name.trim()
    }

    /// The line under it, if the entry publishes one.
    #[must_use]
    pub fn description(&self) -> Option<&str> {
        self.description
            .as_deref()
            .map(str::trim)
            .filter(|d| !d.is_empty())
    }

    /// Validate one element's shape: `kind` is required text, `name` is one safe path segment —
    /// checked because `name` doubles as an address coordinate, never checked against a
    /// repository, per decision #2.
    fn validate(&self, bundle_slug: &str) -> Result<()> {
        if self.kind.trim().is_empty() {
            return Err(Error::BadElementKind(bundle_slug.to_string()));
        }
        let name = self.name.trim();
        if name.is_empty() || name != self.name || !adi_config::valid_name(name) {
            return Err(Error::BadElementName(bundle_slug.to_string(), self.name.clone()));
        }
        Ok(())
    }
}

/// One picture or clip in an entry's gallery.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Media {
    /// Where it is: an `https://` URL, or a `data:image/…` / `data:video/…` URI.
    pub url: String,
    /// Which it is. Absent is ordinary — [`Media::kind`] reads it off the URL, which is right
    /// for every file with an extension and is why publishing it is optional. Say it for a URL
    /// that ends in nothing recognisable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<MediaKind>,
    /// One line under it, in the publisher's words.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub caption: Option<String>,
    /// The still a clip shows before it is played. Ignored on a picture — and worth publishing
    /// on a clip, since a video with no poster is a black rectangle until somebody presses play.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub poster: Option<String>,
}

/// What a gallery entry is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MediaKind {
    Image,
    Video,
}

/// The URL endings that mean a clip. Everything else — including a URL with no extension at all
/// — is a picture unless the entry says otherwise, because that is the commoner case by far.
const VIDEO_EXTENSIONS: [&str; 5] = [".mp4", ".webm", ".ogv", ".mov", ".m4v"];

impl Media {
    /// Where it is, trimmed.
    #[must_use]
    pub fn url(&self) -> &str {
        self.url.trim()
    }

    /// Whether to draw a picture or a player: what the entry declared, else what the URL says.
    #[must_use]
    pub fn kind(&self) -> MediaKind {
        if let Some(kind) = self.kind {
            return kind;
        }
        let url = self.url().to_ascii_lowercase();
        // Only the path decides: a query string can carry anything, and `?v=1.mp4` is not a clip.
        let path = url.split(['?', '#']).next().unwrap_or_default();
        if path.starts_with("data:video/") || VIDEO_EXTENSIONS.iter().any(|ext| path.ends_with(ext))
        {
            MediaKind::Video
        } else {
            MediaKind::Image
        }
    }

    /// The still to show before a clip is played, if the entry publishes one.
    #[must_use]
    pub fn poster(&self) -> Option<&str> {
        self.poster
            .as_deref()
            .map(str::trim)
            .filter(|poster| !poster.is_empty())
    }

    /// The line under it, if the entry publishes one.
    #[must_use]
    pub fn caption(&self) -> Option<&str> {
        self.caption
            .as_deref()
            .map(str::trim)
            .filter(|caption| !caption.is_empty())
    }
}

impl BundleEntry {
    /// Validate one entry: the slug it is addressed by, the repository it clones from, the commit
    /// it pins, and the shape of every element it previews.
    ///
    /// # Errors
    /// [`Error::BadSlug`] when the slug is not one safe path segment, [`Error::BadRepo`] for a
    /// repository URL this build will not clone, [`Error::BadCommit`] for anything but a full
    /// 40-hex object name, [`Error::BadIcon`] for an icon a listing would have to fetch over
    /// plaintext, [`Error::BadElementKind`] / [`Error::BadElementName`] for a preview element with
    /// no kind or an unsafe name. Never for a *recognised* element's kind being wrong — that is
    /// checked against the repository at install time, not here (decision #2).
    pub fn validate(&self) -> Result<()> {
        if !adi_config::valid_name(self.slug.trim()) || self.slug.trim() != self.slug {
            return Err(Error::BadSlug(self.slug.clone()));
        }
        if !valid_repo(&self.repo) {
            return Err(Error::BadRepo(self.repo.clone()));
        }
        if !valid_commit(&self.commit) {
            return Err(Error::BadCommit(self.slug.clone(), self.commit.clone()));
        }
        if let Some(icon) = self.icon()
            && !valid_icon(icon)
        {
            return Err(Error::BadIcon(self.slug.clone(), icon.to_string()));
        }
        // Through the accessor, so an entry left blank in a publisher's template is not an item
        // rather than a refusal — the rule the icon follows.
        for media in self.gallery() {
            // The poster is held to the same rule as the thing it stands in for: it is drawn on
            // the same page, from the same kind of URL.
            for url in [Some(media.url()), media.poster()].into_iter().flatten() {
                if !valid_media(url) {
                    return Err(Error::BadMedia(self.slug.clone(), url.to_string()));
                }
            }
        }
        // Every published element, recognised or not — the shape check applies regardless of
        // whether this build understands the kind, because `name` is an address coordinate either
        // way.
        for element in &self.elements {
            element.validate(&self.slug)?;
        }
        Ok(())
    }

    /// The elements a listing should draw: only the ones whose kind this build understands. An
    /// entry with none previews nothing, whether because it published none or because it published
    /// only kinds this build has never heard of — both are legal (decision #2).
    #[must_use]
    pub fn elements(&self) -> Vec<(Kind, &Element)> {
        self.elements
            .iter()
            .filter_map(|el| Some((el.kind()?, el)))
            .collect()
    }

    /// The long form this entry publishes, if it publishes one.
    #[must_use]
    pub fn readme(&self) -> Option<&str> {
        self.readme
            .as_deref()
            .map(str::trim)
            .filter(|readme| !readme.is_empty())
    }

    /// The gallery as a page shows it: the entries whose URL is not blank, in published order.
    #[must_use]
    pub fn gallery(&self) -> Vec<&Media> {
        self.gallery
            .iter()
            .filter(|media| !media.url().is_empty())
            .collect()
    }

    /// The icon this entry publishes, if it publishes one. Trimmed, and an empty string is no
    /// icon rather than one that fails to load.
    #[must_use]
    pub fn icon(&self) -> Option<&str> {
        self.icon
            .as_deref()
            .map(str::trim)
            .filter(|icon| !icon.is_empty())
    }

    /// The keywords as a listing shows them: trimmed, blanks dropped, and no term twice however
    /// its publisher cased it — in the order they were written, since that order is the
    /// publisher saying which one matters most.
    #[must_use]
    pub fn keywords(&self) -> Vec<String> {
        let mut seen: Vec<String> = Vec::new();
        let mut out = Vec::new();
        for keyword in &self.keywords {
            let keyword = keyword.trim();
            let folded = keyword.to_lowercase();
            if keyword.is_empty() || seen.contains(&folded) {
                continue;
            }
            seen.push(folded);
            out.push(keyword.to_string());
        }
        out
    }

    /// The branch the pin should sit on, when the entry names one.
    #[must_use]
    pub fn branch(&self) -> Option<&str> {
        self.branch
            .as_deref()
            .map(str::trim)
            .filter(|b| !b.is_empty())
    }

    /// The pinned commit, lowercased — the form every comparison against a working copy uses.
    #[must_use]
    pub fn pin(&self) -> String {
        self.commit.trim().to_ascii_lowercase()
    }
}

/// Whether a repository URL is one this build will clone.
///
/// `https://` because fetching is HTTPS, always — and `file://` because an app is developed
/// against a local repository long before it is published, and refusing that would mean the only
/// way to try the install path is to push first. Everything else is refused, `ssh://` and
/// `git@host:path` included: those reach for the operator's agent and their keys, which a URL out
/// of somebody else's manifest has no business doing.
fn valid_repo(repo: &str) -> bool {
    let repo = repo.trim();
    (repo.starts_with("https://") && repo.len() > "https://".len())
        || (repo.starts_with("file:///") && repo.len() > "file:///".len())
}

/// Whether an icon is one a listing will draw.
///
/// `https://` because a page that fetched an image over plaintext would be doing on the
/// operator's behalf the one thing the rest of this module refuses to do — and `data:image/`
/// because the manifest has already been fetched, so an icon carried inside it costs no request
/// at all and works from the cache with the network gone. Everything else is refused, `http://`
/// and a bare path included: there is nothing here for a relative path to be relative *to*.
fn valid_icon(icon: &str) -> bool {
    (icon.starts_with("https://") && icon.len() > "https://".len())
        || carried_inline(icon, "data:image/")
}

/// Whether a gallery URL is one a page will draw — [`valid_icon`]'s rule, plus `data:video/` for
/// a clip small enough to be worth carrying in the manifest.
fn valid_media(url: &str) -> bool {
    valid_icon(url) || carried_inline(url, "data:video/")
}

/// Whether a URI carries the thing itself under this prefix, rather than being the bare prefix.
fn carried_inline(url: &str, prefix: &str) -> bool {
    url.starts_with(prefix) && url.len() > prefix.len()
}

/// Whether a pin is a full git object name — 40 hex characters, in either case.
fn valid_commit(commit: &str) -> bool {
    let commit = commit.trim();
    commit.len() == SHA_LEN && commit.chars().all(|c| c.is_ascii_hexdigit())
}

impl MarketplaceManifest {
    /// Validate every entry, so a manifest is either wholly installable or refused with the entry
    /// that is not. Strict rather than lenient on purpose: a marketplace that published a broken
    /// entry is a listing the operator should see fail, not one that quietly hides part of it.
    ///
    /// # Errors
    /// Whatever [`BundleEntry::validate`] returns for the first entry that does not belong.
    pub fn validate(&self) -> Result<()> {
        for bundle in &self.bundles {
            bundle.validate()?;
        }
        Ok(())
    }

    /// The entry with this slug, if the manifest carries it.
    #[must_use]
    pub fn bundle(&self, slug: &str) -> Option<&BundleEntry> {
        self.bundles.iter().find(|bundle| bundle.slug == slug)
    }

    /// Every slug, comma-joined — the sentence an unknown-slug error carries.
    #[must_use]
    pub fn slugs(&self) -> String {
        self.bundles
            .iter()
            .map(|bundle| bundle.slug.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    }
}

/// Parse a manifest from the bytes a fetch returned, validating it whole.
///
/// # Errors
/// [`Error::Fetch`] when the bytes are not JSON — including a manifest in the retired shape,
/// whose entries carry an `artifact` where a `repo` and `commit` belong; whatever
/// [`MarketplaceManifest::validate`] returns for the first entry that does not belong.
pub fn parse(bytes: &[u8]) -> Result<MarketplaceManifest> {
    let manifest: MarketplaceManifest = serde_json::from_slice(bytes).map_err(|e| {
        // A missing `repo` is almost always a manifest still publishing bundle artifacts, and
        // "missing field `repo`" alone sends its publisher looking in the wrong place.
        let hint = if e.to_string().contains("repo") || e.to_string().contains("commit") {
            " — an app is a git repository at a pinned commit now: give each entry a `repo` and a \
             full 40-hex `commit` (docs/marketplace.md)"
        } else {
            ""
        };
        Error::Fetch(format!("not a valid manifest: {e}{hint}"))
    })?;
    manifest.validate()?;
    Ok(manifest)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SHA: &str = "9f2c1d4e5a6b7c8d9e0f1a2b3c4d5e6f70819a2b";

    #[test]
    fn a_manifest_parses_with_optional_fields_absent() {
        let parsed = parse(
            format!(
                r#"{{"apps":[{{"slug":"crm","name":"CRM",
                   "repo":"https://github.com/adi-family/crm.git","commit":"{SHA}"}}]}}"#
            )
            .as_bytes(),
        )
        .expect("parses");
        assert_eq!(parsed.name, None);
        assert_eq!(parsed.bundles.len(), 1);
        assert_eq!(parsed.bundles[0].version, None);
        assert_eq!(parsed.bundles[0].icon(), None, "and no mark to draw beside it");
        assert!(parsed.bundles[0].keywords().is_empty());
        assert_eq!(parsed.bundles[0].branch(), None, "the default branch, then");
        assert_eq!(parsed.bundles[0].pin(), SHA);
        assert!(parsed.bundles[0].elements().is_empty(), "no preview published");
        assert_eq!(parsed.bundle("crm").map(|a| a.name.as_str()), Some("CRM"));
        assert_eq!(parsed.bundle("nope"), None);
    }

    #[test]
    fn an_empty_bundles_list_is_a_valid_marketplace() {
        let parsed = parse(b"{\"apps\":[]}").expect("parses");
        assert_eq!(parsed.bundles.len(), 0);
        assert_eq!(parsed.slugs(), "");
    }

    #[test]
    fn the_retired_apps_key_still_parses_as_the_same_field() {
        let parsed = parse(
            format!(
                r#"{{"apps":[{{"slug":"crm","name":"CRM",
                   "repo":"https://example/crm.git","commit":"{SHA}"}}]}}"#
            )
            .as_bytes(),
        )
        .expect("an old manifest keeps parsing");
        assert_eq!(parsed.bundles.len(), 1, "v1's own key aliases the new field");
        assert_eq!(parsed.bundle("crm").map(|b| b.name.as_str()), Some("CRM"));

        // The new key works exactly the same, side by side — nothing published for v1 has to move.
        let renamed = parse(
            format!(
                r#"{{"bundles":[{{"slug":"crm","name":"CRM",
                   "repo":"https://example/crm.git","commit":"{SHA}"}}]}}"#
            )
            .as_bytes(),
        )
        .expect("the new key parses too");
        assert_eq!(renamed.bundles, parsed.bundles);
    }

    #[test]
    fn an_entry_that_does_not_belong_refuses_the_whole_manifest() {
        let entry = |slug: &str, repo: &str, commit: &str| {
            parse(
                format!(
                    r#"{{"apps":[{{"slug":"{slug}","name":"X","repo":"{repo}","commit":"{commit}"}}]}}"#
                )
                .as_bytes(),
            )
        };

        // A slug that is not one path segment.
        assert!(matches!(
            entry("../evil", "https://example/x.git", SHA),
            Err(Error::BadSlug(_))
        ));
        // Repositories this build will not clone: plaintext, the protocols that reach for the
        // operator's ssh agent, and an argument that would read as a flag.
        for repo in [
            "http://example/x.git",
            "git://example/x.git",
            "ssh://git@example/x.git",
            "git@github.com:adi-family/crm.git",
            "--upload-pack=whatever",
            "https://",
        ] {
            assert!(
                matches!(entry("crm", repo, SHA), Err(Error::BadRepo(_))),
                "{repo}"
            );
        }
        // Anything but a full object name: a branch, a tag, a short sha, an empty pin.
        for commit in ["main", "v0.1.0", "9f2c1d4", "", &"z".repeat(40)] {
            assert!(
                matches!(
                    entry("crm", "https://example/x.git", commit),
                    Err(Error::BadCommit(_, _))
                ),
                "{commit}"
            );
        }

        // Not JSON at all.
        assert!(matches!(parse(b"<html>"), Err(Error::Fetch(_))));
    }

    #[test]
    fn a_manifest_in_the_retired_artifact_shape_says_what_replaced_it() {
        let err = parse(
            br#"{"apps":[{"slug":"crm","name":"CRM","artifact":"https://example/crm.bundle.json"}]}"#,
        )
        .expect_err("refused");
        assert!(err.to_string().contains("repo"), "{err}");
        assert!(err.to_string().contains("commit"), "{err}");
    }

    #[test]
    fn an_entry_carries_a_mark_and_what_it_is_about() {
        let parsed = parse(
            format!(
                r#"{{"apps":[{{"slug":"crm","name":"CRM",
                   "icon":"https://example/icon.png",
                   "keywords":["  Sales ","contacts","SALES","","follow-up"],
                   "repo":"https://example/crm.git","commit":"{SHA}"}}]}}"#
            )
            .as_bytes(),
        )
        .expect("parses");
        assert_eq!(parsed.bundles[0].icon(), Some("https://example/icon.png"));
        assert_eq!(
            parsed.bundles[0].keywords(),
            vec!["Sales", "contacts", "follow-up"],
            "trimmed, blanks dropped, no term twice, publisher's order kept"
        );

        // An icon carried in the manifest itself, which costs the listing no request at all.
        let inline = parse(
            format!(
                r#"{{"apps":[{{"slug":"crm","name":"CRM","icon":"data:image/svg+xml;base64,PHN2Zz48L3N2Zz4=",
                   "repo":"https://example/crm.git","commit":"{SHA}"}}]}}"#
            )
            .as_bytes(),
        )
        .expect("parses");
        assert!(
            inline.bundles[0]
                .icon()
                .is_some_and(|i| i.starts_with("data:image/"))
        );

        // An icon a listing would have to fetch over plaintext — or one there is nothing to
        // resolve — refuses the manifest the way a bad repo does, rather than drawing a hole.
        for icon in [
            "http://example/icon.png",
            "icon.png",
            "/assets/icon.png",
            "javascript:alert(1)",
            "data:image/",
        ] {
            let refused = parse(
                format!(
                    r#"{{"apps":[{{"slug":"crm","name":"CRM","icon":"{icon}",
                       "repo":"https://example/crm.git","commit":"{SHA}"}}]}}"#
                )
                .as_bytes(),
            );
            assert!(matches!(refused, Err(Error::BadIcon(_, _))), "{icon}");
        }
        // An empty icon is no icon, not a refusal: a publisher's template left unfilled.
        let blank = parse(
            format!(
                r#"{{"apps":[{{"slug":"crm","name":"CRM","icon":"   ",
                   "repo":"https://example/crm.git","commit":"{SHA}"}}]}}"#
            )
            .as_bytes(),
        )
        .expect("parses");
        assert_eq!(blank.bundles[0].icon(), None);
    }

    #[test]
    fn an_entry_carries_a_page_of_its_own_the_listing_has_no_room_for() {
        let parsed = parse(
            format!(
                // `r###`, not `r#`: the readme opens with a Markdown heading, so the literal
                // carries `"##` — which is where an `r#"…"#` or an `r##"…"##` would have ended.
                r###"{{"apps":[{{"slug":"crm","name":"CRM",
                   "readme":"## What it does\n\nOne list.",
                   "gallery":[
                     {{"url":"https://example/list.png","caption":" The list "}},
                     {{"url":"https://example/tour.mp4","poster":"https://example/tour.png"}},
                     {{"url":"https://example/watch?v=x","kind":"video"}},
                     {{"url":"https://example/still.png?v=1.mp4"}},
                     {{"url":"   "}}
                   ],
                   "repo":"https://example/crm.git","commit":"{SHA}"}}]}}"###
            )
            .as_bytes(),
        )
        .expect("parses");
        let app = &parsed.bundles[0];
        assert_eq!(app.readme(), Some("## What it does\n\nOne list."));

        let gallery = app.gallery();
        assert_eq!(gallery.len(), 4, "an item with no url is not an item");
        assert_eq!(gallery[0].kind(), MediaKind::Image);
        assert_eq!(gallery[0].caption(), Some("The list"));
        assert_eq!(gallery[0].poster(), None);
        assert_eq!(
            gallery[1].kind(),
            MediaKind::Video,
            "read off the extension"
        );
        assert_eq!(gallery[1].poster(), Some("https://example/tour.png"));
        assert_eq!(
            gallery[2].kind(),
            MediaKind::Video,
            "a url that ends in nothing says so itself"
        );
        assert_eq!(
            gallery[3].kind(),
            MediaKind::Image,
            "only the path decides — a query string can carry anything"
        );

        // A gallery is held to the icon's rule, and so is a poster: same page, same kind of URL.
        for item in [
            r#"{"url":"http://example/list.png"}"#,
            r#"{"url":"list.png"}"#,
            r#"{"url":"https://example/tour.mp4","poster":"http://example/tour.png"}"#,
        ] {
            let refused = parse(
                format!(
                    r#"{{"apps":[{{"slug":"crm","name":"CRM","gallery":[{item}],
                       "repo":"https://example/crm.git","commit":"{SHA}"}}]}}"#
                )
                .as_bytes(),
            );
            assert!(matches!(refused, Err(Error::BadMedia(_, _))), "{item}");
        }

        // A clip small enough to carry in the manifest is carried in the manifest.
        let inline = parse(
            format!(
                r#"{{"apps":[{{"slug":"crm","name":"CRM",
                   "gallery":[{{"url":"data:video/mp4;base64,AAAA"}}],
                   "repo":"https://example/crm.git","commit":"{SHA}"}}]}}"#
            )
            .as_bytes(),
        )
        .expect("parses");
        assert_eq!(inline.bundles[0].gallery()[0].kind(), MediaKind::Video);
    }

    #[test]
    fn a_local_repository_installs_so_an_app_can_be_developed_before_it_is_published() {
        let parsed = parse(
            format!(
                r#"{{"apps":[{{"slug":"crm","name":"CRM","branch":"dev",
                   "repo":"file:///Users/somebody/crm","commit":"{}"}}]}}"#,
                SHA.to_ascii_uppercase()
            )
            .as_bytes(),
        )
        .expect("parses");
        assert_eq!(parsed.bundles[0].branch(), Some("dev"));
        assert_eq!(parsed.bundles[0].pin(), SHA, "a pin compares lowercased");
    }

    #[test]
    fn a_bundle_previews_a_mixed_kind_elements_array() {
        let parsed = parse(
            format!(
                r#"{{"bundles":[{{"slug":"crm-suite","name":"CRM suite",
                   "repo":"https://example/crm-suite.git","commit":"{SHA}",
                   "elements":[
                     {{"kind":"agent","name":"sales-bot","description":"Drafts the follow-up."}},
                     {{"kind":"tool","name":"csv-import"}},
                     {{"kind":"dashboard","name":"crm"}},
                     {{"kind":"carrier-pigeon","name":"nope"}}
                   ]}}]}}"#
            )
            .as_bytes(),
        )
        .expect("parses");
        let entry = &parsed.bundles[0];
        // The raw field carries all four, including the one this build has never heard of.
        assert_eq!(entry.elements.len(), 4);
        // The preview leaves the unrecognised kind out rather than refusing the manifest.
        let seen = entry.elements();
        assert_eq!(seen.len(), 3, "{seen:?}");
        assert_eq!(seen[0].0, Kind::Agent);
        assert_eq!(seen[0].1.name(), "sales-bot");
        assert_eq!(seen[0].1.description(), Some("Drafts the follow-up."));
        assert_eq!(seen[1].1.description(), None, "no description published");
        assert_eq!(seen[2].0, Kind::Dashboard);
    }

    #[test]
    fn an_element_with_no_kind_or_an_unsafe_name_refuses_the_manifest() {
        let entry = |element: &str| {
            parse(
                format!(
                    r#"{{"bundles":[{{"slug":"crm-suite","name":"X",
                       "repo":"https://example/x.git","commit":"{SHA}",
                       "elements":[{element}]}}]}}"#
                )
                .as_bytes(),
            )
        };
        assert!(matches!(
            entry(r#"{"kind":"","name":"sales-bot"}"#),
            Err(Error::BadElementKind(_))
        ));
        assert!(matches!(
            entry(r#"{"kind":"agent","name":"../evil"}"#),
            Err(Error::BadElementName(_, _))
        ));
        assert!(matches!(
            entry(r#"{"kind":"agent","name":""}"#),
            Err(Error::BadElementName(_, _))
        ));
        // An unrecognised kind, by contrast, is not a validation failure at all — decision #2.
        entry(r#"{"kind":"carrier-pigeon","name":"nope"}"#).expect("not a refusal");
    }
}
