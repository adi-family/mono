//! The on-disk project manifest ([`Manifest`], serialized as `config.toml`) and the
//! id-attached view of a loaded project ([`Project`]).

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

/// A project's metadata manifest — the `config.toml` at the root of its directory. It holds
/// descriptive information only; a project's *runtime* config (services, proxy hosts, ports)
/// stays in the project's own `.adi/hive.yaml`, owned by adi-hive.
///
/// Unknown fields are ignored so the manifest can gain fields without breaking older stores.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Manifest {
    /// A human-facing display name. Defaults to the project id when created without one.
    pub name: String,
    /// An optional one-line description.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// The id of the project this one nests under (a sub-project), or `None` for a top-level
    /// project. Pure metadata over a flat store: every project keeps its own directory under
    /// `projects/` whatever its parent — only listings/UI nest by these links.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<String>,
    /// When the project was registered, as Unix epoch seconds.
    #[serde(default)]
    pub created_at: u64,
    /// When the project was archived (soft-deleted), as Unix epoch seconds; `None` while active.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub archived_at: Option<u64>,
}

/// A registered project: its id (the directory name under `projects/`) plus its loaded
/// [`Manifest`]. The id is not stored in the file — it *is* the directory. `Serialize` so
/// the CLI can emit it as JSON; it is built from disk, never deserialized, so no `Deserialize`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Project {
    /// The project id — its directory name under `~/.adi/mono/projects/`.
    pub id: String,
    /// The parsed `config.toml` manifest.
    pub manifest: Manifest,
}

impl Project {
    /// Whether the project is archived (its manifest carries an `archived_at`).
    #[must_use]
    pub fn is_archived(&self) -> bool {
        self.manifest.archived_at.is_some()
    }

    /// The display name, falling back to the id when the manifest's name is blank.
    #[must_use]
    pub fn display_name(&self) -> &str {
        if self.manifest.name.trim().is_empty() {
            &self.id
        } else {
            &self.manifest.name
        }
    }
}

/// Validate a project id: a single, filesystem-safe path segment. This is a security
/// boundary — ids arrive from the CLI and the HTTP API and are joined onto the store path,
/// so anything with a separator or `.`/`..` must be rejected to prevent path traversal.
pub(crate) fn validate_id(id: &str) -> Result<()> {
    let ok = !id.is_empty()
        && id != "."
        && id != ".."
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'));
    if ok {
        Ok(())
    } else {
        Err(Error::InvalidId(id.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_ids_are_single_safe_segments() {
        for id in ["demo", "my-app", "app_2", "a.b", "A1"] {
            assert!(validate_id(id).is_ok(), "{id} should be valid");
        }
    }

    #[test]
    fn invalid_ids_are_rejected() {
        for id in [
            "",
            ".",
            "..",
            "a/b",
            "a\\b",
            "with space",
            "sneaky/../x",
            "tab\t",
        ] {
            assert!(
                matches!(validate_id(id), Err(Error::InvalidId(_))),
                "{id:?} should be rejected"
            );
        }
    }

    #[test]
    fn display_name_falls_back_to_id_when_blank() {
        let p = Project {
            id: "demo".to_string(),
            manifest: Manifest {
                name: "   ".to_string(),
                ..Manifest::default()
            },
        };
        assert_eq!(p.display_name(), "demo");
        assert!(!p.is_archived());
    }
}
