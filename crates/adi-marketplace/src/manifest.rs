//! The marketplace manifest: one JSON document, a list of apps.
//!
//! An entry names **a git repository and one commit in it**. That pair is the whole contract:
//! what installs is a clone standing at that commit, so a publisher who pushes something else
//! tomorrow changes nothing about what the operator already read, and moving onto a newer commit
//! is an act somebody takes on purpose. A manifest that named a branch — or an artifact behind a
//! URL — would be a listing whose meaning changes under the reader.
//!
//! Unknown fields are ignored, so a v2 manifest an older machine reads still lists its apps
//! rather than failing whole.
//!
//! ```json
//! {
//!   "name": "ADI starter apps",
//!   "apps": [
//!     {
//!       "slug": "crm",
//!       "name": "CRM",
//!       "description": "Who has gone quiet, and what was last said to them.",
//!       "version": "0.1.0",
//!       "repo": "https://github.com/adi-family/crm.git",
//!       "commit": "9f2c1d4e5a6b7c8d9e0f1a2b3c4d5e6f70819a2b",
//!       "branch": "main"
//!     }
//!   ]
//! }
//! ```

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

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
    /// The apps. An empty list is a valid marketplace (a starter repo with nothing in it yet);
    /// what is never valid is an entry that fails [`AppEntry::validate`].
    #[serde(default)]
    pub apps: Vec<AppEntry>,
}

/// One app in a marketplace: the listing text, and the repository and commit it installs from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppEntry {
    /// The published identity: the second half of `<marketplace>/<slug>`, which is what an
    /// install is addressed by. **Not** the directory it lands as — that is minted from the name
    /// the operator chooses. One safe path segment.
    pub slug: String,
    /// The human name, offered as the default when the operator names their copy.
    pub name: String,
    /// One line on what the app is for.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
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
}

impl AppEntry {
    /// Validate one entry: the slug it is addressed by, the repository it clones from, and the
    /// commit it pins.
    ///
    /// # Errors
    /// [`Error::BadSlug`] when the slug is not one safe path segment, [`Error::BadRepo`] for a
    /// repository URL this build will not clone, [`Error::BadCommit`] for anything but a full
    /// 40-hex object name.
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
        Ok(())
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
    /// Whatever [`AppEntry::validate`] returns for the first entry that does not belong.
    pub fn validate(&self) -> Result<()> {
        for app in &self.apps {
            app.validate()?;
        }
        Ok(())
    }

    /// The entry with this slug, if the manifest carries it.
    #[must_use]
    pub fn app(&self, slug: &str) -> Option<&AppEntry> {
        self.apps.iter().find(|app| app.slug == slug)
    }

    /// Every slug, comma-joined — the sentence an unknown-slug error carries.
    #[must_use]
    pub fn slugs(&self) -> String {
        self.apps
            .iter()
            .map(|app| app.slug.as_str())
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
        assert_eq!(parsed.apps.len(), 1);
        assert_eq!(parsed.apps[0].version, None);
        assert_eq!(parsed.apps[0].branch(), None, "the default branch, then");
        assert_eq!(parsed.apps[0].pin(), SHA);
        assert_eq!(parsed.app("crm").map(|a| a.name.as_str()), Some("CRM"));
        assert_eq!(parsed.app("nope"), None);
    }

    #[test]
    fn an_empty_apps_list_is_a_valid_marketplace() {
        let parsed = parse(b"{\"apps\":[]}").expect("parses");
        assert_eq!(parsed.apps.len(), 0);
        assert_eq!(parsed.slugs(), "");
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
        assert_eq!(parsed.apps[0].branch(), Some("dev"));
        assert_eq!(parsed.apps[0].pin(), SHA, "a pin compares lowercased");
    }
}
