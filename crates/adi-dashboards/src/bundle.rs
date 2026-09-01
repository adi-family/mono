//! A dashboard packed up to travel: the bundle DTO, the rules a bundle's files are confined by
//! on the way in, and the walk that packs a directory on the way out.
//!
//! One format, two roads — machine-to-machine over the mesh, and from a marketplace onto this
//! machine — because "everything a person or an agent authored, nothing a machine generated" is
//! the same packing list whichever road it takes.

use std::path::{Path, PathBuf};

use base64::Engine as _;
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// One file of a [`DashboardBundle`].
///
/// The bytes are base64 rather than text because a dashboard is a directory a human fills: an
/// icon, a font, a fixture `.db` are all ordinary things to find in one, and a transfer that
/// silently dropped whatever was not UTF-8 would be a transfer you cannot trust.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BundleFile {
    /// Path relative to the dashboard's own directory, always `/`-separated. Never absolute and
    /// never containing `..` — the receiving side re-checks both before it writes anything.
    pub path: String,
    /// The file's bytes, base64 (standard alphabet, padded).
    pub contents: String,
}

/// A dashboard packed up for another machine — the body of `POST /api/dashboards/import`, and
/// the artifact a marketplace entry points at.
///
/// What is **not** in here is the point. The manifest and `.adi/hive.yaml` are omitted and rebuilt
/// on the far side, because both name things that are true only where they were written: the hive
/// file carries an absolute `working_dir`, and its `proxy.host` may already belong to a different
/// dashboard over there. Everything a person or an agent authored travels verbatim.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DashboardBundle {
    /// The dashboard's id, carried across so a second transfer **updates** the copy on the node
    /// instead of leaving a duplicate behind. A preference for the far side's directory name —
    /// where it lands is decided by whoever is writing (a transfer honours it; a marketplace
    /// install installs under its own entry's slug).
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    /// The project it is filed under here. Honoured on the far side only if a project with that
    /// id exists there too; otherwise the copy arrives unfiled, which is what an id that means
    /// nothing on that machine should do.
    #[serde(default)]
    pub project: Option<String>,
    /// The hostname it answers on where it came from — a *preference*, not an instruction. The
    /// receiving machine keeps the label when it is free there and derives a fresh one when it is
    /// not, because two dashboards on one hostname is a routing coin-flip.
    #[serde(default)]
    pub host: Option<String>,
    pub files: Vec<BundleFile>,
}

/// The most a bundle may carry, in raw bytes. Generous for what a dashboard is — a handful of
/// `.ts` files and whatever assets go with them — and small enough that the whole thing fits in
/// one JSON body on both ends after base64 has added its third.
///
/// A cap rather than a stream because the alternative is worse: a transfer that half-arrives
/// leaves a dashboard on the node with some of its modules, which looks like it worked.
pub const MAX_BUNDLE_BYTES: u64 = 4 * 1024 * 1024;

/// The most files a bundle may carry. A dashboard someone pointed at a data directory is the
/// case this exists for — it fails with a sentence rather than a five-minute walk.
pub const MAX_BUNDLE_FILES: usize = 2000;

/// Directory names never packed, wherever they appear in the tree.
///
/// `.adi` because the hive file inside it is rebuilt on the far side (its `working_dir` is an
/// absolute local path, and its host may be taken over there); the other two because they are
/// caches of things already in the bundle, and shipping them is how a 20 KB dashboard becomes a
/// 200 MB one.
pub const NEVER_BUNDLED_DIRS: &[&str] = &[".adi", "node_modules", ".git"];

/// Files never packed from the dashboard's root: the manifest travels as the bundle's own fields,
/// so shipping it too would be two sources of truth for one name.
pub const NEVER_BUNDLED_ROOT_FILES: &[&str] = &["config.toml"];

/// What lives through an import that overwrites an existing dashboard.
///
/// `.adi` holds the hive file the receiving machine wrote for *its* paths — rewritten right
/// after, but never through a window in which the supervisor could read a missing one. Anything
/// installed under `node_modules` is a cache the bundle deliberately did not carry, and deleting
/// it would make every re-transfer an install.
pub const KEPT_ON_IMPORT: &[&str] = &[".adi", "node_modules"];

/// A bundle's files, decoded and resolved to absolute paths under the dashboard directory.
pub type DecodedFiles = Vec<(PathBuf, Vec<u8>)>;

/// Why a bundle was refused whole. Every variant is a property of the bundle itself — nothing
/// here names a write failure, which stays [`std::io::Error`] with the caller.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum BundleError {
    /// A file entry carries no path at all.
    #[error("the bundle carries a file with no path")]
    NoPath,
    /// The bundle claims a generated directory it may never carry.
    #[error("a bundle may not carry {path:?} — that directory is generated here")]
    GeneratedDir { path: String },
    /// The path does not resolve inside the dashboard directory.
    #[error("refusing {path:?}: {cause}")]
    Escaped { path: String, cause: String },
    /// The path names the dashboard directory itself.
    #[error("{path:?} names the dashboard itself")]
    NamesItself { path: String },
    /// The contents are not the base64 the format promises.
    #[error("{path:?} is not valid base64: {cause}")]
    NotBase64 { path: String, cause: String },
    /// The decoded contents pass [`MAX_BUNDLE_BYTES`].
    #[error("the bundle is too large")]
    TooLarge,
    /// More files than [`MAX_BUNDLE_FILES`] — refused before a byte is decoded.
    #[error("the bundle carries too many files")]
    TooManyFiles,
}

/// Decode every file's bytes and resolve its path, refusing the whole bundle on the first thing
/// that does not belong.
///
/// The path check is [`adi_fs::Jail`]'s, not one written here: it is the same lexical rule the
/// store browser is confined by, and a second implementation is a second chance to get `..` wrong.
/// On top of it, the generated directories are refused outright — a bundle claiming to carry
/// `.adi/hive.yaml` is a bundle trying to choose this machine's routing.
///
/// # Errors
/// [`BundleError`] for the first file that does not belong — a bad path, a generated directory,
/// contents that are not the base64 the format promises, or a size past [`MAX_BUNDLE_BYTES`].
/// Nothing is written on refusal.
pub fn decode_bundle(dir: &Path, files: &[BundleFile]) -> Result<DecodedFiles, BundleError> {
    let jail = adi_fs::Jail::new(dir);
    let mut decoded = Vec::with_capacity(files.len());
    let mut total = 0_u64;
    for file in files {
        let path = file.path.trim();
        if path.is_empty() {
            return Err(BundleError::NoPath);
        }
        if path
            .split(['/', '\\'])
            .any(|segment| NEVER_BUNDLED_DIRS.contains(&segment))
        {
            return Err(BundleError::GeneratedDir {
                path: path.to_string(),
            });
        }
        let resolved = jail.resolve(path).map_err(|e| BundleError::Escaped {
            path: path.to_string(),
            cause: e.to_string(),
        })?;
        if resolved == dir {
            return Err(BundleError::NamesItself {
                path: path.to_string(),
            });
        }
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(file.contents.as_bytes())
            .map_err(|e| BundleError::NotBase64 {
                path: path.to_string(),
                cause: e.to_string(),
            })?;
        total += bytes.len() as u64;
        if total > MAX_BUNDLE_BYTES {
            return Err(BundleError::TooLarge);
        }
        decoded.push((resolved, bytes));
    }
    Ok(decoded)
}

/// Mirror `decoded` into the dashboard directory: drop what an earlier version left behind, then
/// write what this one carries.
///
/// # Errors
/// [`std::io::Error`] on any failure.
pub fn write_import(dir: &Path, decoded: &DecodedFiles) -> std::io::Result<()> {
    clear_imported(dir)?;
    std::fs::create_dir_all(dir.join(".adi"))?;
    for (path, bytes) in decoded {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, bytes)?;
    }
    Ok(())
}

/// Empty a dashboard directory of everything an import replaces, keeping [`KEPT_ON_IMPORT`]. A
/// directory that does not exist yet is simply nothing to clear.
///
/// # Errors
/// [`std::io::Error`] on any failure.
pub fn clear_imported(dir: &Path) -> std::io::Result<()> {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e),
    };
    for entry in entries.flatten() {
        let Ok(name) = entry.file_name().into_string() else {
            continue;
        };
        if KEPT_ON_IMPORT.contains(&name.as_str()) {
            continue;
        }
        let path = entry.path();
        // `symlink_metadata`, so a symlinked directory is unlinked rather than walked into.
        let meta = std::fs::symlink_metadata(&path)?;
        if meta.is_dir() {
            std::fs::remove_dir_all(&path)?;
        } else {
            std::fs::remove_file(&path)?;
        }
    }
    Ok(())
}

/// An id from a bundle, accepted only as one ordinary path segment — it becomes a directory name
/// under the dashboards root, and the far side chose it.
#[must_use]
pub fn valid_id(raw: &str) -> Option<String> {
    let id = raw.trim();
    let usable = !id.is_empty()
        && id.len() <= 128
        && id != "."
        && id != ".."
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'));
    usable.then(|| id.to_string())
}

/// Walk one directory of a dashboard, appending its files to `files`. `rel` is the path so far,
/// relative to the dashboard root, which is what the bundle records.
///
/// Recursive rather than iterative because the depth is a dashboard's own source tree; the two
/// caps are what bound the work, not the shape of the walk.
///
/// # Errors
/// [`BundleError::TooLarge`] (as [`BundleError`] carries the count and size so far) once either
/// cap is past; [`std::io::Error`] on a read failure.
pub fn collect_files(
    dir: &Path,
    rel: &mut PathBuf,
    files: &mut Vec<BundleFile>,
    total: &mut u64,
) -> Result<(), CollectError> {
    let here = dir.join(&*rel);
    let entries = match std::fs::read_dir(&here) {
        Ok(entries) => entries,
        Err(e) => return Err(CollectError::Io(e)),
    };
    // Sorted, so a bundle of an unchanged dashboard is byte-identical between runs and a diff of
    // two transfers is about the dashboard rather than about directory order.
    let mut names: Vec<String> = entries
        .flatten()
        .filter_map(|e| e.file_name().into_string().ok())
        .collect();
    names.sort();

    for name in names {
        if NEVER_BUNDLED_DIRS.contains(&name.as_str())
            || (rel.as_os_str().is_empty() && NEVER_BUNDLED_ROOT_FILES.contains(&name.as_str()))
        {
            continue;
        }
        let path = here.join(&name);
        // Not `metadata`: that follows the link, and a link out of the dashboard would then be
        // read and shipped as though it lived here.
        let Ok(meta) = std::fs::symlink_metadata(&path) else {
            continue;
        };
        if meta.file_type().is_symlink() {
            continue;
        }
        rel.push(&name);
        let walked = if meta.is_dir() {
            collect_files(dir, rel, files, total)
        } else {
            pack_file(&path, rel, meta.len(), files, total)
        };
        rel.pop();
        walked?;
    }
    Ok(())
}

/// The failure modes of packing a directory into a bundle.
#[derive(Debug, Error)]
pub enum CollectError {
    /// A read of the dashboard's own files failed.
    #[error(transparent)]
    Io(#[from] std::io::Error),
    /// The directory is past [`MAX_BUNDLE_FILES`] / [`MAX_BUNDLE_BYTES`].
    #[error(
        "this dashboard is too large to transfer ({} files, {} bytes so far; the limits are \
         {MAX_BUNDLE_FILES} files and {MAX_BUNDLE_BYTES} bytes) — move the bulk of it out of the \
         dashboard directory, or copy it across by hand",
        files,
        bytes
    )]
    TooLarge { files: usize, bytes: u64 },
}

/// Add one file to the bundle, refusing once either cap is past.
fn pack_file(
    path: &Path,
    rel: &Path,
    size: u64,
    files: &mut Vec<BundleFile>,
    total: &mut u64,
) -> Result<(), CollectError> {
    *total += size;
    if *total > MAX_BUNDLE_BYTES || files.len() >= MAX_BUNDLE_FILES {
        return Err(CollectError::TooLarge {
            files: files.len() + 1,
            bytes: *total,
        });
    }
    let bytes = std::fs::read(path).map_err(CollectError::Io)?;
    files.push(BundleFile {
        path: slash_path(rel),
        contents: base64::engine::general_purpose::STANDARD.encode(bytes),
    });
    Ok(())
}

/// A relative path as the bundle spells it: `/`-separated on every platform, so a dashboard
/// packed on Windows unpacks on Linux and the reverse.
fn slash_path(rel: &Path) -> String {
    rel.components()
        .filter_map(|c| match c {
            std::path::Component::Normal(segment) => Some(segment.to_string_lossy()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("/")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "adi-dashboards-bundle-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id(),
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("scratch root");
        root
    }

    fn b64(bytes: &[u8]) -> String {
        base64::engine::general_purpose::STANDARD.encode(bytes)
    }

    fn entry(path: &str, contents: &str) -> BundleFile {
        BundleFile {
            path: path.to_string(),
            contents: b64(contents.as_bytes()),
        }
    }

    #[test]
    fn decode_refuses_every_path_that_does_not_belong() {
        let root = scratch("escape");
        for path in [
            "../../../../etc/passwd",
            "/etc/passwd",
            "frontend/../../escaped.ts",
            ".adi/hive.yaml",
            "node_modules/dep.js",
            "",
        ] {
            let err = decode_bundle(&root, &[entry(path, "pwned")]).expect_err(path);
            assert!(
                !matches!(err, BundleError::NotBase64 { .. }),
                "{path:?} was rejected as base64, not as a path: {err}"
            );
        }
        // Nothing was written while refusing.
        assert!(!root.join("frontend").exists());
        // The generated-dirs rule holds at any depth, not only the root.
        assert!(matches!(
            decode_bundle(&root, &[entry("a/b/.git/config", "x")]),
            Err(BundleError::GeneratedDir { .. })
        ));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn decode_refuses_what_is_not_base64_or_is_too_large() {
        let root = scratch("contents");
        let bad = BundleFile {
            path: "x.ts".to_string(),
            contents: "not base64 !!".to_string(),
        };
        assert!(matches!(
            decode_bundle(&root, &[bad]),
            Err(BundleError::NotBase64 { .. })
        ));

        let big = vec![b'a'; usize::try_from(MAX_BUNDLE_BYTES + 1).expect("usize holds 4 MiB")];
        assert_eq!(
            decode_bundle(&root, &[entry("y.ts", std::str::from_utf8(&big).unwrap())]),
            Err(BundleError::TooLarge)
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn write_import_mirrors_and_keeps_what_the_node_made_for_itself() {
        let root = scratch("mirror");
        // A previous version's file, the node's own hive file, and its dependency cache.
        std::fs::create_dir_all(root.join(".adi")).expect("adi");
        std::fs::create_dir_all(root.join("node_modules")).expect("nm");
        std::fs::write(root.join("stale.ts"), "old").expect("stale");
        std::fs::write(root.join(".adi").join("hive.yaml"), "ours").expect("hive");
        std::fs::write(root.join("node_modules").join("dep.js"), "cached").expect("dep");

        let decoded = vec![
            (root.join("frontend").join("index.ts"), b"new".to_vec()),
            (root.join("logo.png"), vec![0x89, b'P', 0x00, 0xff]),
        ];
        write_import(&root, &decoded).expect("mirror");

        assert_eq!(
            std::fs::read(root.join("frontend").join("index.ts")).expect("file"),
            b"new"
        );
        assert_eq!(
            std::fs::read(root.join("logo.png")).expect("asset"),
            [0x89, b'P', 0x00, 0xff],
            "bytes that are not UTF-8 survive verbatim"
        );
        assert!(!root.join("stale.ts").exists(), "a mirror, not a merge");
        assert_eq!(
            std::fs::read_to_string(root.join(".adi").join("hive.yaml")).expect("hive"),
            "ours",
            "the receiving machine's routing is never deleted out from under it"
        );
        assert!(root.join("node_modules").join("dep.js").exists());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn ids_are_one_ordinary_path_segment() {
        for id in ["crm", "nosh-2", "a.b", "A_1"] {
            assert_eq!(valid_id(id).as_deref(), Some(id));
        }
        for id in ["../elsewhere", "a/b", "", ".", "..", "a\\b"] {
            assert_eq!(valid_id(id), None, "{id:?}");
        }
    }

    #[test]
    fn collect_skips_generated_dirs_symlinks_and_root_manifest() {
        let root = scratch("collect");
        std::fs::create_dir_all(root.join("frontend").join("modules")).expect("dirs");
        std::fs::create_dir_all(root.join("node_modules").join("left-pad")).expect("cache");
        std::fs::write(root.join("config.toml"), "name = \"x\"").expect("manifest");
        std::fs::write(root.join("node_modules").join("left-pad").join("i.js"), "x").expect("dep");
        std::fs::write(
            root.join("frontend").join("modules").join("mine.ts"),
            "panel",
        )
        .expect("panel");
        #[cfg(unix)]
        std::os::unix::fs::symlink("/etc/hosts", root.join("hosts.link")).expect("symlink");

        let mut files = Vec::new();
        let mut rel = PathBuf::new();
        let mut total = 0;
        collect_files(&root, &mut rel, &mut files, &mut total).expect("collect");

        let paths: Vec<&str> = files.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(paths, ["frontend/modules/mine.ts"], "{paths:?}");
        assert_eq!(files[0].contents, b64(b"panel"));
        let _ = std::fs::remove_dir_all(&root);
    }
}
