//! The on-disk tool manifest ([`Manifest`], serialized as `config.toml`) and the id-attached
//! view of a loaded tool ([`Tool`]), plus the runtime vocabulary and the id/bin-name rules.

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

/// A shell script, run as `sh <script> <args…>`.
pub const RUNTIME_SH: &str = "sh";
/// TypeScript, run as `bun run <script> <args…>`.
pub const RUNTIME_TS: &str = "ts";

/// Map a stored/incoming `runtime` onto one this build understands, defaulting to [`RUNTIME_SH`]
/// so an unknown value written by a newer build still runs as a shell script rather than failing.
#[must_use]
pub fn normalize_runtime(runtime: &str) -> &'static str {
    match runtime.trim() {
        RUNTIME_TS => RUNTIME_TS,
        _ => RUNTIME_SH,
    }
}

/// The file extension an *owned* tool's script gets for a runtime (`sh` → `sh`, `ts` → `ts`).
#[must_use]
pub fn runtime_ext(runtime: &str) -> &'static str {
    match normalize_runtime(runtime) {
        RUNTIME_TS => "ts",
        _ => "sh",
    }
}

/// Guess a runtime from a file's extension, defaulting to [`RUNTIME_SH`]. Used when a tool is
/// linked by path without an explicit runtime — a `.ts` file is TypeScript, everything else
/// (`.sh`, no extension, a shebang script) runs through `sh`.
#[must_use]
pub fn runtime_from_path(path: &str) -> &'static str {
    match path.rsplit('.').next() {
        Some(ext) if ext.eq_ignore_ascii_case("ts") => RUNTIME_TS,
        _ => RUNTIME_SH,
    }
}

/// Validate a runtime string, returning the normalized form. Unlike [`normalize_runtime`] this
/// *rejects* an unknown value so the CLI/API can tell the user rather than silently pick `sh`.
///
/// # Errors
/// [`Error::InvalidRuntime`] when `runtime` is neither `sh` nor `ts`.
pub fn validate_runtime(runtime: &str) -> Result<&'static str> {
    match runtime.trim() {
        RUNTIME_SH => Ok(RUNTIME_SH),
        RUNTIME_TS => Ok(RUNTIME_TS),
        other => Err(Error::InvalidRuntime(other.to_string())),
    }
}

/// A tool's metadata manifest — the `config.toml` at the root of its directory.
///
/// A tool is a small CLI an agent can run. It is either **owned** (its script lives in the store,
/// at `tools/<id>/script.<ext>`) or **linked** (`linked_path` points at an existing file on disk,
/// which the store never copies or deletes). Unknown fields are ignored so the manifest can gain
/// fields without breaking older stores.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Manifest {
    /// A human-facing display name. Also the basis for the tool's `.bin/<name>` shim. Defaults to
    /// the tool id when created without one.
    pub name: String,
    /// An optional one-line description.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// The language/interpreter of the tool's script (`sh` | `ts`).
    #[serde(default)]
    pub runtime: String,
    /// The absolute path a *linked* tool points at. `None` for an owned tool (whose script the
    /// store keeps under its own directory).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub linked_path: Option<String>,
    /// The project this tool is filed under (its id), or `None` for a global tool.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    /// Whether this is a built-in **system** tool (an adi-ecosystem CLI seeded by the platform).
    /// System tools are always present in every agent's `.bin`, and are protected from hard delete.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub system: bool,
    /// When the tool was registered, as Unix epoch seconds.
    #[serde(default)]
    pub created_at: u64,
    /// When the tool was archived (soft-deleted), as Unix epoch seconds; `None` while active.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub archived_at: Option<u64>,
}

/// A registered tool: its id (the directory name under `tools/`) plus its loaded [`Manifest`].
/// The id is not stored in the file — it *is* the directory. `Serialize` so the CLI can emit it
/// as JSON; it is built from disk, never deserialized, so no `Deserialize`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Tool {
    /// The tool id — its directory name under `~/.adi/mono/tools/` (a generated UUID).
    pub id: String,
    /// The parsed `config.toml` manifest.
    pub manifest: Manifest,
}

impl Tool {
    /// Whether the tool is archived (its manifest carries an `archived_at`).
    #[must_use]
    pub fn is_archived(&self) -> bool {
        self.manifest.archived_at.is_some()
    }

    /// Whether this tool links an existing file on disk (rather than owning a script in the store).
    #[must_use]
    pub fn is_linked(&self) -> bool {
        self.manifest.linked_path.is_some()
    }

    /// Whether this is a built-in system tool (seeded by the platform).
    #[must_use]
    pub fn is_system(&self) -> bool {
        self.manifest.system
    }

    /// The normalized runtime (`sh` | `ts`).
    #[must_use]
    pub fn runtime(&self) -> &'static str {
        normalize_runtime(&self.manifest.runtime)
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

    /// The `.bin` shim file name agents invoke this tool by — a filesystem-safe slug of its name,
    /// falling back to the short id when the name has no usable characters.
    #[must_use]
    pub fn bin_name(&self) -> String {
        let slug = bin_slug(self.display_name());
        if slug.is_empty() {
            short_id(&self.id)
        } else {
            slug
        }
    }
}

/// Validate a tool id: a single, filesystem-safe path segment (same rule as a store name; see
/// [`adi_config::validate_name`]). This is a security boundary — ids arrive from the CLI and the
/// HTTP API and are joined onto the store path, so path traversal must be rejected.
pub(crate) fn validate_id(id: &str) -> Result<()> {
    adi_config::validate_name(id, Error::InvalidId)
}

/// The leading segment of a uuid — enough to recognize a tool without its full id.
#[must_use]
pub(crate) fn short_id(id: &str) -> String {
    id.split('-').next().unwrap_or(id).to_string()
}

/// Fold a display name into a filesystem-safe `.bin` shim name: lowercased, whitespace and any
/// character that isn't `[a-z0-9._-]` collapsed to a single `-`, with leading/trailing `-`
/// trimmed. Two tools whose names slug to the same value collide in `.bin`; the store resolves
/// that deterministically when it regenerates the directory.
#[must_use]
pub(crate) fn bin_slug(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    let mut last_dash = false;
    for ch in name.trim().chars() {
        let c = ch.to_ascii_lowercase();
        if c.is_ascii_alphanumeric() || matches!(c, '.' | '_') {
            out.push(c);
            last_dash = false;
        } else if !last_dash {
            out.push('-');
            last_dash = true;
        }
    }
    out.trim_matches('-').to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_ids_are_single_safe_segments() {
        for id in ["demo", "my-tool", "tool_2", "a.b", "A1"] {
            assert!(validate_id(id).is_ok(), "{id} should be valid");
        }
    }

    #[test]
    fn invalid_ids_are_rejected() {
        for id in ["", ".", "..", "a/b", "a\\b", "with space", "sneaky/../x"] {
            assert!(
                matches!(validate_id(id), Err(Error::InvalidId(_))),
                "{id:?} should be rejected"
            );
        }
    }

    #[test]
    fn runtimes_normalize_and_validate() {
        assert_eq!(normalize_runtime("ts"), RUNTIME_TS);
        assert_eq!(normalize_runtime("sh"), RUNTIME_SH);
        assert_eq!(normalize_runtime("perl"), RUNTIME_SH);
        assert_eq!(runtime_ext("ts"), "ts");
        assert_eq!(runtime_ext("sh"), "sh");
        assert_eq!(runtime_from_path("build.ts"), RUNTIME_TS);
        assert_eq!(runtime_from_path("deploy.sh"), RUNTIME_SH);
        assert_eq!(runtime_from_path("mytool"), RUNTIME_SH);
        assert_eq!(validate_runtime("ts").expect("ts"), RUNTIME_TS);
        assert!(matches!(
            validate_runtime("perl"),
            Err(Error::InvalidRuntime(_))
        ));
    }

    #[test]
    fn bin_slug_folds_to_a_safe_name() {
        assert_eq!(bin_slug("Deploy Prod"), "deploy-prod");
        assert_eq!(bin_slug("  weird!!name  "), "weird-name");
        assert_eq!(bin_slug("keep.dots_and-dashes"), "keep.dots_and-dashes");
        assert_eq!(bin_slug("!!!"), "");
    }

    #[test]
    fn bin_name_falls_back_to_short_id_when_name_is_unusable() {
        let tool = Tool {
            id: "abcd1234-5678-90ab-cdef-1234567890ab".to_string(),
            manifest: Manifest {
                name: "!!!".to_string(),
                ..Manifest::default()
            },
        };
        assert_eq!(tool.bin_name(), "abcd1234");
        assert!(!tool.is_archived());
        assert!(!tool.is_linked());
        assert_eq!(tool.runtime(), RUNTIME_SH);
    }
}
