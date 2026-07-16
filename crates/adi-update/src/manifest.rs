//! The published release manifest — the small JSON file the updater polls. Written by
//! `apps/macos/publish.sh` next to the DMG; unknown fields are ignored so the format
//! can grow without breaking older clients.

use serde::{Deserialize, Serialize};

/// `manifest.json` as published alongside each release.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Manifest {
    /// The released version (`0.2.0`); compared numerically against the installed app.
    pub version: String,
    /// The full app bundle as a notarized DMG — the one artifact that updates everything.
    pub dmg: Artifact,
    /// RFC 3339 publication timestamp, informational.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pub_date: Option<String>,
    /// Human release notes, informational.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
}

/// One downloadable artifact: where it lives and how to verify the bytes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Artifact {
    pub url: String,
    /// Hex sha256 of the artifact, checked before the DMG is ever mounted.
    pub sha256: String,
    /// Size in bytes, informational.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,
}

impl Manifest {
    /// Parse a manifest from JSON bytes.
    ///
    /// # Errors
    /// The `serde_json` error when the payload isn't a valid manifest.
    pub fn from_json(bytes: &[u8]) -> Result<Self, serde_json::Error> {
        serde_json::from_slice(bytes)
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
        assert_eq!(m.dmg.url, "https://example.com/ADI.dmg");
        assert_eq!(m.dmg.sha256, "abc123");
        assert_eq!(m.dmg.size, Some(123));
    }

    #[test]
    fn optional_fields_may_be_absent() {
        let m =
            Manifest::from_json(br#"{ "version": "0.1.1", "dmg": { "url": "u", "sha256": "s" } }"#)
                .expect("parse");
        assert_eq!(m.pub_date, None);
        assert_eq!(m.notes, None);
        assert_eq!(m.dmg.size, None);
    }
}
