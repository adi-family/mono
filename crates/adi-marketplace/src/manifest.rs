//! The marketplace manifest: one JSON document, a list of apps.
//!
//! Kept deliberately minimal — slug, name, description, version, and where the artifact comes
//! from — because shipping one app is how you find out what the manifest is missing before three
//! more kinds are built on it. Unknown fields are ignored, so a v2 manifest an older machine
//! reads still lists its apps rather than failing whole.
//!
//! The artifact is a [`DashboardBundle`](adi_dashboards::DashboardBundle) JSON: the same shape
//! the panel's export writes and its import lands. One format for "a dashboard travels", whether
//! between machines or from a marketplace.
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
//!       "artifact": "https://…/crm.bundle.json"
//!     }
//!   ]
//! }
//! ```

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

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

/// One app in a marketplace: the listing text, and where the artifact comes from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppEntry {
    /// The installable identity: the second half of `<marketplace>/<slug>`, and the directory the
    /// app lands as under `dashboards/`. One safe path segment.
    pub slug: String,
    /// The human name, shown in listings and carried onto the installed dashboard.
    pub name: String,
    /// One line on what the app is for.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// The app's version, as its publisher wrote it. Shown, never enforced: v1 has no pinning, no
    /// downgrade refusal, no comparison beyond string equality.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// Where the artifact comes from: the HTTPS URL of a `DashboardBundle` JSON.
    pub artifact: String,
}

impl MarketplaceManifest {
    /// Validate every entry, so a manifest is either wholly installable or refused with the entry
    /// that is not. Strict rather than lenient on purpose: a marketplace that published a broken
    /// entry is a listing the operator should see fail, not one that quietly hides part of it.
    ///
    /// # Errors
    /// [`Error::BadSlug`] for the first entry whose slug is not one safe path segment;
    /// [`Error::NotHttps`] for the first whose artifact is not an `https://` URL.
    pub fn validate(&self) -> Result<()> {
        for app in &self.apps {
            if !adi_config::valid_name(app.slug.trim()) || app.slug.trim() != app.slug {
                return Err(Error::BadSlug(app.slug.clone()));
            }
            if !app.artifact.trim().starts_with("https://") {
                return Err(Error::NotHttps(app.artifact.clone()));
            }
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
/// [`Error::Fetch`] when the bytes are not JSON; whatever [`MarketplaceManifest::validate`]
/// returns for the first entry that does not belong.
pub fn parse(bytes: &[u8]) -> Result<MarketplaceManifest> {
    let manifest: MarketplaceManifest = serde_json::from_slice(bytes)
        .map_err(|e| Error::Fetch(format!("not a valid manifest: {e}")))?;
    manifest.validate()?;
    Ok(manifest)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_manifest_parses_with_optional_fields_absent() {
        let parsed = parse(
            br#"{"apps":[{"slug":"crm","name":"CRM",
                        "artifact":"https://example/crm.bundle.json"}]}"#,
        )
        .expect("parses");
        assert_eq!(parsed.name, None);
        assert_eq!(parsed.apps.len(), 1);
        assert_eq!(parsed.apps[0].version, None);
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
        // A slug that is not one path segment — the directory it would land as could climb.
        let err = parse(
            br#"{"apps":[{"slug":"../evil","name":"X",
                        "artifact":"https://example/x.json"}]}"#,
        )
        .expect_err("refused");
        assert!(matches!(err, Error::BadSlug(_)), "{err}");

        // An artifact that is not https.
        let err =
            parse(br#"{"apps":[{"slug":"crm","name":"CRM","artifact":"http://example/c.json"}]}"#)
                .expect_err("refused");
        assert!(matches!(err, Error::NotHttps(_)), "{err}");

        // Not JSON at all.
        assert!(matches!(parse(b"<html>"), Err(Error::Fetch(_))));
    }
}
