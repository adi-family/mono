//! A dashboard's `config.toml` — the metadata its directory carries, independent of anything
//! the hive file says about running it.

use std::path::Path;

use serde::Deserialize;

/// The metadata file each dashboard directory carries.
///
/// Deliberately loose: every field is optional and a missing or malformed file degrades to the
/// default rather than failing the caller, because a dashboard is a directory anybody can copy
/// in and the listing that visits all of them must survive half-written ones.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct Manifest {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    /// The project this dashboard is filed under (its id), or `None` when unfiled.
    #[serde(default)]
    pub project: Option<String>,
    /// When the dashboard was archived (Unix seconds), or `None` while it is live.
    ///
    /// "Archived" is also how an app arrives from a marketplace: landed with its hive file
    /// parked out of the supervisor's glob, so installing starts nothing. See the marketplace
    /// crate, which stamps this on arrival.
    #[serde(default)]
    pub archived_at: Option<u64>,
    /// The node this dashboard was moved to, when it was. Written beside
    /// [`archived_at`](Self::archived_at) rather than instead of it: the local remains are
    /// archived like any other archived dashboard, and this only says *why*.
    #[serde(default)]
    pub moved_to: Option<String>,
}

/// Read a dashboard directory's `config.toml` manifest, degrading a missing or malformed file to
/// the default (all fields absent) rather than failing.
#[must_use]
pub fn read_manifest(dir: &Path) -> Manifest {
    std::fs::read_to_string(dir.join("config.toml"))
        .ok()
        .and_then(|raw| toml::from_str::<Manifest>(&raw).ok())
        .unwrap_or_default()
}

/// Write a dashboard's `config.toml`, emitting only the fields that are present so a rewrite never
/// invents a blank `name`/`description` the manifest didn't already carry.
///
/// # Errors
/// [`std::io::Error`] on any write failure.
pub fn write_manifest(dir: &Path, manifest: &Manifest) -> std::io::Result<()> {
    let mut out = String::new();
    for (key, value) in [
        ("name", manifest.name.as_deref().map(toml_string)),
        (
            "description",
            manifest.description.as_deref().map(toml_string),
        ),
        ("project", manifest.project.as_deref().map(toml_string)),
        ("archived_at", manifest.archived_at.map(|ts| ts.to_string())),
        ("moved_to", manifest.moved_to.as_deref().map(toml_string)),
    ] {
        if let Some(value) = value {
            out.push_str(key);
            out.push_str(" = ");
            out.push_str(&value);
            out.push('\n');
        }
    }
    std::fs::write(dir.join("config.toml"), out)
}

/// Quote a value as a TOML basic string, escaping what that grammar requires.
fn toml_string(value: &str) -> String {
    let escaped = value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n");
    format!("\"{escaped}\"")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> std::path::PathBuf {
        let root = std::env::temp_dir().join(format!(
            "adi-dashboards-manifest-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id(),
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("scratch root");
        root
    }

    #[test]
    fn a_written_manifest_round_trips_and_omits_absent_fields() {
        let dir = scratch("roundtrip");
        write_manifest(
            &dir,
            &Manifest {
                name: Some("Nosh".to_string()),
                description: Some("a \"quoted\" thing\non two lines".to_string()),
                ..Manifest::default()
            },
        )
        .expect("write");

        let raw = std::fs::read_to_string(dir.join("config.toml")).expect("read");
        assert!(!raw.contains("project"), "absent fields stay absent: {raw}");
        assert!(!raw.contains("archived_at"), "{raw}");

        let back = read_manifest(&dir);
        assert_eq!(back.name.as_deref(), Some("Nosh"));
        assert_eq!(
            back.description.as_deref(),
            Some("a \"quoted\" thing\non two lines")
        );
        assert_eq!(back.project, None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn every_present_field_is_written() {
        let dir = scratch("all-fields");
        write_manifest(
            &dir,
            &Manifest {
                name: Some("Nosh".to_string()),
                description: Some("what it is for".to_string()),
                project: Some("demo".to_string()),
                archived_at: Some(1_786_839_320),
                moved_to: Some("laptop-b".to_string()),
            },
        )
        .expect("write");

        let raw = std::fs::read_to_string(dir.join("config.toml")).expect("read");
        assert!(raw.contains("name = \"Nosh\"\n"), "{raw}");
        assert!(raw.contains("archived_at = 1786839320\n"), "{raw}");
        assert!(raw.contains("moved_to = \"laptop-b\"\n"), "{raw}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_missing_or_broken_manifest_degrades_to_empty() {
        let dir = scratch("degrade");
        assert_eq!(read_manifest(&dir), Manifest::default());

        std::fs::write(dir.join("config.toml"), "name = [oh no\n").expect("broken");
        assert_eq!(read_manifest(&dir), Manifest::default());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
