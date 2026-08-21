//! The published release manifest — the small JSON file the updater polls. Written by
//! `.github/workflows/release.yml` (or `apps/macos/publish.sh` for a local cut) next to the
//! artifacts; unknown fields are ignored so the format can grow without breaking older clients.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// `manifest.json` as published alongside each release.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Manifest {
    /// The released version (`0.2.0`); compared numerically against the installed version.
    pub version: String,
    /// The macOS app bundle as a notarized DMG. It stays a top-level field because clients
    /// released before per-platform artifacts existed require it — dropping it would strand
    /// every Mac still running one of those builds. New publishers emit it *and* the
    /// `macos` entry in [`Self::artifacts`], which carry the same bytes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dmg: Option<Artifact>,
    /// Artifacts keyed by platform — `macos`, `linux-x86_64`, `windows-x86_64` — as produced
    /// by [`host_platform`]. A platform with no entry simply has no update published for it.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub artifacts: BTreeMap<String, Artifact>,
    /// RFC 3339 publication timestamp, informational.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pub_date: Option<String>,
    /// The release notes, in markdown: one section of the repo's `CHANGELOG.md`, lifted out
    /// by `scripts/changelog.sh` when the release is cut.
    ///
    /// Not decoration — this is what the control panel shows a person before they let an
    /// update restart their machine, so it is the release's own case for being taken. It may
    /// be absent (a hand-cut release, a manifest from another channel), and the panel then
    /// offers the update on the version number alone.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
}

/// One downloadable artifact: where it lives and how to verify the bytes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Artifact {
    pub url: String,
    /// Hex sha256 of the artifact, checked before anything is mounted, unpacked or run.
    pub sha256: String,
    /// Size in bytes, informational.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,
}

/// The key this host looks for in [`Manifest::artifacts`].
///
/// macOS is just `macos`: its artifact is a *universal* bundle covering both arches, so
/// splitting it by architecture would publish the same DMG twice. Everywhere else the arch
/// is part of the key, since those artifacts are per-triple.
#[must_use]
pub fn host_platform() -> String {
    match std::env::consts::OS {
        "macos" => "macos".to_string(),
        os => format!("{os}-{}", std::env::consts::ARCH),
    }
}

impl Manifest {
    /// Parse a manifest from JSON bytes.
    ///
    /// # Errors
    /// The `serde_json` error when the payload isn't a valid manifest.
    pub fn from_json(bytes: &[u8]) -> Result<Self, serde_json::Error> {
        serde_json::from_slice(bytes)
    }

    /// The artifact for `platform`, falling back to the legacy top-level `dmg` on macOS so a
    /// manifest published before per-platform artifacts still updates a Mac.
    #[must_use]
    pub fn artifact_for(&self, platform: &str) -> Option<&Artifact> {
        self.artifacts
            .get(platform)
            .or_else(|| (platform == "macos").then_some(self.dmg.as_ref()).flatten())
    }

    /// The artifact for the running host, if this release publishes one.
    #[must_use]
    pub fn artifact_for_host(&self) -> Option<&Artifact> {
        self.artifact_for(&host_platform())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_published_manifest() {
        let json = br#"{
            "version": "0.2.0",
            "pub_date": "2026-07-16T00:00:00Z",
            "notes": "adds triggers",
            "dmg": { "url": "https://example.com/ADI.dmg", "sha256": "abc123", "size": 123 },
            "some_future_field": true
        }"#;
        let m = Manifest::from_json(json).expect("parse");
        assert_eq!(m.version, "0.2.0");
        let dmg = m.dmg.as_ref().expect("dmg");
        assert_eq!(dmg.url, "https://example.com/ADI.dmg");
        assert_eq!(dmg.sha256, "abc123");
        assert_eq!(dmg.size, Some(123));
    }

    #[test]
    fn optional_fields_may_be_absent() {
        let m =
            Manifest::from_json(br#"{ "version": "0.1.1", "dmg": { "url": "u", "sha256": "s" } }"#)
                .expect("parse");
        assert_eq!(m.pub_date, None);
        assert_eq!(m.notes, None);
        assert_eq!(m.dmg.expect("dmg").size, None);
    }

    #[test]
    fn host_platform_names_macos_without_an_arch() {
        // The universal DMG covers both arches, so the key must not carry one.
        let key = host_platform();
        if std::env::consts::OS == "macos" {
            assert_eq!(key, "macos");
        } else {
            assert_eq!(key, format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH));
        }
    }

    #[test]
    fn per_platform_artifacts_are_selected_by_key() {
        let json = br#"{
            "version": "0.2.0",
            "dmg": { "url": "legacy.dmg", "sha256": "d" },
            "artifacts": {
                "macos":          { "url": "ADI.dmg",             "sha256": "a" },
                "linux-x86_64":   { "url": "adi-linux-x64.tar.gz","sha256": "b" },
                "windows-x86_64": { "url": "ADI-windows-x64.zip", "sha256": "c" }
            }
        }"#;
        let m = Manifest::from_json(json).expect("parse");
        assert_eq!(m.artifact_for("linux-x86_64").expect("linux").sha256, "b");
        assert_eq!(m.artifact_for("windows-x86_64").expect("win").sha256, "c");
        // The explicit entry wins over the legacy field when both are present.
        assert_eq!(m.artifact_for("macos").expect("macos").sha256, "a");
        assert!(m.artifact_for("freebsd-x86_64").is_none());
    }

    #[test]
    fn a_legacy_manifest_still_updates_macos_only() {
        let m = Manifest::from_json(br#"{ "version": "0.2.0", "dmg": { "url": "u", "sha256": "s" } }"#)
            .expect("parse");
        assert_eq!(m.artifact_for("macos").expect("macos").url, "u");
        // …and publishes nothing for a node, which must not mistake the DMG for its package.
        assert!(m.artifact_for("linux-x86_64").is_none());
    }

    #[test]
    fn a_manifest_may_drop_the_legacy_dmg_entirely() {
        let m = Manifest::from_json(
            br#"{ "version": "1.0.0", "artifacts": { "macos": { "url": "u", "sha256": "s" } } }"#,
        )
        .expect("parse");
        assert!(m.dmg.is_none());
        assert_eq!(m.artifact_for("macos").expect("macos").url, "u");
    }
}
