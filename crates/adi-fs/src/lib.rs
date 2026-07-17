//! adi-fs — an **isolated, base-directory-jailed filesystem**.
//!
//! A [`Jail`] is rooted at one base directory. Every operation takes a *relative* path and is
//! confined to that base: the relative path is normalized and any component that would climb
//! out — a `..`, an absolute path, or a Windows drive/UNC prefix — is rejected, and (as
//! defense in depth) the resolved path is canonicalized so a symlink can't smuggle access
//! outside either. In short: **there is no going backward** past the base.
//!
//! It's a small, dependency-free primitive: browse a directory ([`Jail::list`]), read a file
//! ([`Jail::read`] / [`Jail::read_to_string`]), and write one atomically ([`Jail::write`]).
//! The control panel uses it to let a project edit the files under its own directory (its
//! `.adi/hive.yaml` and anything beside it) without ever reaching the rest of the disk.
//!
//! ```
//! # let tmp = std::env::temp_dir().join(format!("adi-fs-doctest-{}", std::process::id()));
//! # std::fs::create_dir_all(tmp.join(".adi")).unwrap();
//! # std::fs::write(tmp.join(".adi/hive.yaml"), "version: \"1\"\n").unwrap();
//! use adi_fs::{Jail, Error};
//!
//! let jail = Jail::new(&tmp);
//!
//! // Browse and read within the base.
//! let entries = jail.list("")?;
//! assert!(entries.iter().any(|e| e.name == ".adi" && e.is_dir));
//! let text = jail.read_to_string(".adi/hive.yaml")?;
//! assert!(text.contains("version"));
//!
//! // Edit a file atomically.
//! jail.write(".adi/hive.yaml", b"version: \"2\"\n")?;
//! assert_eq!(jail.read_to_string(".adi/hive.yaml")?, "version: \"2\"\n");
//!
//! // Climbing out is refused.
//! assert!(matches!(jail.read("../secret"), Err(Error::Escape(_))));
//! # std::fs::remove_dir_all(&tmp).ok();
//! # Ok::<(), adi_fs::Error>(())
//! ```

mod error;
mod fsutil;

use std::path::{Component, Path, PathBuf};
use std::time::UNIX_EPOCH;

pub use error::{Error, Result};

/// An isolated filesystem confined to one base directory. Cheap to clone; holds only the
/// base path. Construct one per root you want to expose (e.g. a project's directory).
#[derive(Debug, Clone)]
pub struct Jail {
    base: PathBuf,
}

/// One entry in a directory [listing](Jail::list): its name plus lightweight stats. `is_dir`
/// and `size`/`modified` follow symlinks (they describe the target); `is_symlink` records
/// whether the entry itself is a link.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// The entry's file name (a single path segment, always valid UTF-8 — non-UTF-8 names,
    /// which can't be addressed safely, are skipped).
    pub name: String,
    /// Whether the entry is (or points at) a directory.
    pub is_dir: bool,
    /// Whether the entry itself is a symbolic link.
    pub is_symlink: bool,
    /// The file size in bytes (0 for directories and broken links).
    pub size: u64,
    /// Last-modified time as Unix epoch seconds, when the platform reports it.
    pub modified: Option<u64>,
}

/// Lightweight metadata for a single path ([`Jail::metadata`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Metadata {
    /// Whether the path is (or points at) a directory.
    pub is_dir: bool,
    /// Whether the path is (or points at) a regular file.
    pub is_file: bool,
    /// Whether the path itself is a symbolic link.
    pub is_symlink: bool,
    /// The file size in bytes.
    pub size: u64,
    /// Last-modified time as Unix epoch seconds, when the platform reports it.
    pub modified: Option<u64>,
}

impl Jail {
    /// Create a jail rooted at `base`. The base need not exist yet (operations will report
    /// [`Error::NotFound`] until it does); it becomes the boundary nothing may resolve past.
    #[must_use]
    pub fn new(base: impl Into<PathBuf>) -> Self {
        Self { base: base.into() }
    }

    /// The base directory this jail is confined to.
    #[must_use]
    pub fn base(&self) -> &Path {
        &self.base
    }

    /// Resolve a jailed relative path to an absolute path under [`base`](Self::base), rejecting
    /// anything that would climb out. This is purely lexical (it does not touch disk) and does
    /// not require the path to exist — so it's usable for a not-yet-created write target.
    ///
    /// The empty string (and `.`) resolve to the base itself.
    ///
    /// # Errors
    /// [`Error::Escape`] if `rel` is absolute, has a drive/UNC prefix, or contains a `..`
    /// component.
    pub fn resolve(&self, rel: &str) -> Result<PathBuf> {
        let mut normalized = PathBuf::new();
        for component in Path::new(rel).components() {
            match component {
                Component::Normal(segment) => normalized.push(segment),
                // `.` and redundant separators are harmless; drop them.
                Component::CurDir => {}
                // `..`, a leading `/`, or a `C:\`/UNC prefix all try to leave the base.
                Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                    return Err(Error::Escape(rel.to_string()));
                }
            }
        }
        if normalized.as_os_str().is_empty() {
            Ok(self.base.clone())
        } else {
            Ok(self.base.join(normalized))
        }
    }

    /// List the directory at `rel` (the base itself when `rel` is empty). Entries are sorted
    /// directories-first, then case-insensitively by name. Non-UTF-8 names are skipped.
    ///
    /// # Errors
    /// [`Error::Escape`] if `rel` escapes the base, [`Error::NotFound`] if the directory is
    /// missing, or [`Error::Io`] on any other read failure.
    pub fn list(&self, rel: &str) -> Result<Vec<Entry>> {
        let dir = self.resolve(rel)?;
        self.guard(rel, &dir)?;
        let reader = std::fs::read_dir(&dir).map_err(|e| Error::io(rel, e))?;

        let mut entries = Vec::new();
        for item in reader {
            let item = item.map_err(|e| Error::io(rel, e))?;
            // A non-UTF-8 name can't be re-addressed over the API, so skip it rather than
            // surface something the caller could never open.
            let Ok(name) = item.file_name().into_string() else {
                continue;
            };
            let path = item.path();
            let is_symlink = std::fs::symlink_metadata(&path)
                .map(|m| m.file_type().is_symlink())
                .unwrap_or(false);
            // Follow the link for the target's kind/size; a broken link degrades to "file, 0".
            let (is_dir, size, modified) = match std::fs::metadata(&path) {
                Ok(meta) => (meta.is_dir(), meta.len(), mtime(&meta)),
                Err(_) => (false, 0, None),
            };
            entries.push(Entry {
                name,
                is_dir,
                is_symlink,
                size,
                modified,
            });
        }
        entries.sort_by(|a, b| {
            b.is_dir
                .cmp(&a.is_dir)
                .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
        });
        Ok(entries)
    }

    /// Read the raw bytes of the file at `rel`.
    ///
    /// # Errors
    /// [`Error::Escape`] if `rel` escapes the base, [`Error::NotFound`] if it's missing,
    /// [`Error::NotAFile`] if it's a directory, or [`Error::Io`] on any other read failure.
    pub fn read(&self, rel: &str) -> Result<Vec<u8>> {
        let path = self.resolve(rel)?;
        self.guard(rel, &path)?;
        let meta = std::fs::metadata(&path).map_err(|e| Error::io(rel, e))?;
        if meta.is_dir() {
            return Err(Error::NotAFile(rel.to_string()));
        }
        std::fs::read(&path).map_err(|e| Error::io(rel, e))
    }

    /// Read the file at `rel` as UTF-8 text.
    ///
    /// # Errors
    /// As [`read`](Self::read), plus [`Error::NotText`] when the bytes aren't valid UTF-8.
    pub fn read_to_string(&self, rel: &str) -> Result<String> {
        let bytes = self.read(rel)?;
        String::from_utf8(bytes).map_err(|_| Error::NotText(rel.to_string()))
    }

    /// Atomically write `bytes` to the file at `rel`, creating any missing parent directories
    /// **within the jail**. A reader never observes a half-written file (write-temp-then-rename).
    ///
    /// # Errors
    /// [`Error::Escape`] if `rel` escapes the base, [`Error::NotAFile`] if `rel` is an existing
    /// directory, or [`Error::Io`] on a write failure.
    pub fn write(&self, rel: &str, bytes: &[u8]) -> Result<()> {
        let path = self.resolve(rel)?;
        // Never clobber a directory with a file write.
        if std::fs::symlink_metadata(&path).is_ok_and(|m| m.is_dir()) {
            return Err(Error::NotAFile(rel.to_string()));
        }
        self.guard(rel, &path)?;
        fsutil::atomic_write(&path, bytes).map_err(|e| Error::io(rel, e))
    }

    /// Stat the path at `rel`.
    ///
    /// # Errors
    /// [`Error::Escape`] if `rel` escapes the base, [`Error::NotFound`] if it's missing, or
    /// [`Error::Io`] on any other failure.
    pub fn metadata(&self, rel: &str) -> Result<Metadata> {
        let path = self.resolve(rel)?;
        self.guard(rel, &path)?;
        let meta = std::fs::metadata(&path).map_err(|e| Error::io(rel, e))?;
        let is_symlink = std::fs::symlink_metadata(&path)
            .map(|m| m.file_type().is_symlink())
            .unwrap_or(false);
        Ok(Metadata {
            is_dir: meta.is_dir(),
            is_file: meta.is_file(),
            is_symlink,
            size: meta.len(),
            modified: mtime(&meta),
        })
    }

    /// Whether an in-bounds path exists at `rel`. An escaping path is simply `false` (it can
    /// never exist inside the jail), never an error.
    #[must_use]
    pub fn exists(&self, rel: &str) -> bool {
        self.resolve(rel).is_ok_and(|p| p.exists())
    }

    /// Defense-in-depth symlink check: confirm `path` (already lexically in-bounds via
    /// [`resolve`](Self::resolve)) still lives under the canonicalized base once symlinks are
    /// followed. A path that doesn't exist yet can't be canonicalized, so we walk up to its
    /// nearest existing ancestor and verify that instead — a symlinked parent can't smuggle a
    /// write outside the base.
    fn guard(&self, rel: &str, path: &Path) -> Result<()> {
        let base = self.base.canonicalize().map_err(|e| Error::io(rel, e))?;
        let mut probe = path;
        loop {
            match probe.canonicalize() {
                Ok(resolved) => {
                    return if resolved.starts_with(&base) {
                        Ok(())
                    } else {
                        Err(Error::Escape(rel.to_string()))
                    };
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => match probe.parent() {
                    // Keep walking up toward the (existing) base.
                    Some(parent) => probe = parent,
                    // No parent left and still not found: lexically it was in-bounds, so allow.
                    None => return Ok(()),
                },
                Err(e) => return Err(Error::io(rel, e)),
            }
        }
    }
}

/// A file's last-modified time as Unix epoch seconds, when the platform reports it.
fn mtime(meta: &std::fs::Metadata) -> Option<u64> {
    meta.modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fresh, isolated base directory with a small tree: `config.toml`, `.adi/hive.yaml`,
    /// and an empty `sub/` dir.
    fn scratch(tag: &str) -> (PathBuf, Jail) {
        let base = std::env::temp_dir().join(format!(
            "adi-fs-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id(),
        ));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(base.join(".adi")).unwrap();
        std::fs::create_dir_all(base.join("sub")).unwrap();
        std::fs::write(base.join("config.toml"), b"name = \"demo\"\n").unwrap();
        std::fs::write(base.join(".adi/hive.yaml"), b"version: \"1\"\n").unwrap();
        (base.clone(), Jail::new(base))
    }

    #[test]
    fn resolve_normalizes_in_bounds_paths() {
        let (base, jail) = scratch("resolve");
        assert_eq!(jail.resolve("").unwrap(), base);
        assert_eq!(jail.resolve(".").unwrap(), base);
        assert_eq!(jail.resolve("a/./b").unwrap(), base.join("a").join("b"));
        assert_eq!(jail.resolve("a//b").unwrap(), base.join("a").join("b"));
        let _ = std::fs::remove_dir_all(base);
    }

    #[test]
    fn resolve_rejects_climbing_out() {
        let (base, jail) = scratch("escape");
        for rel in ["..", "../x", "a/../../b", "/etc/passwd", "a/../.."] {
            assert!(
                matches!(jail.resolve(rel), Err(Error::Escape(_))),
                "{rel:?} should be rejected"
            );
        }
        assert!(matches!(
            jail.resolve("sub/../config.toml"),
            Err(Error::Escape(_))
        ));
        let _ = std::fs::remove_dir_all(base);
    }

    #[test]
    fn list_sorts_dirs_first_then_by_name() {
        let (base, jail) = scratch("list");
        let names: Vec<_> = jail.list("").unwrap().into_iter().map(|e| e.name).collect();
        assert_eq!(names, vec![".adi", "sub", "config.toml"]);

        let entries = jail.list("").unwrap();
        let hive_dir = entries.iter().find(|e| e.name == ".adi").unwrap();
        assert!(hive_dir.is_dir);
        let cfg = entries.iter().find(|e| e.name == "config.toml").unwrap();
        assert!(!cfg.is_dir);
        assert_eq!(cfg.size, b"name = \"demo\"\n".len() as u64);
        let _ = std::fs::remove_dir_all(base);
    }

    #[test]
    fn read_and_write_round_trip() {
        let (base, jail) = scratch("rw");
        assert_eq!(
            jail.read_to_string(".adi/hive.yaml").unwrap(),
            "version: \"1\"\n"
        );
        jail.write(".adi/hive.yaml", b"version: \"2\"\n").unwrap();
        assert_eq!(
            jail.read_to_string(".adi/hive.yaml").unwrap(),
            "version: \"2\"\n"
        );
        let _ = std::fs::remove_dir_all(base);
    }

    #[test]
    fn write_creates_missing_parents_inside_the_jail() {
        let (base, jail) = scratch("mkparents");
        jail.write("new/deep/file.txt", b"hi").unwrap();
        assert_eq!(jail.read_to_string("new/deep/file.txt").unwrap(), "hi");
        assert!(base.join("new/deep/file.txt").is_file());
        let _ = std::fs::remove_dir_all(base);
    }

    #[test]
    fn escaping_reads_and_writes_never_touch_disk() {
        let (base, jail) = scratch("noescape");
        assert!(matches!(jail.read("../config.toml"), Err(Error::Escape(_))));
        assert!(matches!(
            jail.write("../evil.txt", b"x"),
            Err(Error::Escape(_))
        ));
        assert!(matches!(jail.list(".."), Err(Error::Escape(_))));
        assert!(!base.parent().unwrap().join("evil.txt").exists());
        let _ = std::fs::remove_dir_all(base);
    }

    #[test]
    fn read_reports_missing_and_directory_targets() {
        let (base, jail) = scratch("errors");
        assert!(matches!(jail.read("nope.txt"), Err(Error::NotFound(_))));
        assert!(matches!(jail.read(".adi"), Err(Error::NotAFile(_))));
        let _ = std::fs::remove_dir_all(base);
    }

    #[test]
    fn read_to_string_rejects_non_utf8() {
        let (base, jail) = scratch("binary");
        std::fs::write(base.join("blob.bin"), [0xff, 0xfe, 0x00]).unwrap();
        assert!(matches!(
            jail.read_to_string("blob.bin"),
            Err(Error::NotText(_))
        ));
        assert_eq!(jail.read("blob.bin").unwrap(), vec![0xff, 0xfe, 0x00]);
        let _ = std::fs::remove_dir_all(base);
    }

    #[test]
    #[cfg(unix)]
    fn symlink_escaping_the_base_is_refused() {
        let (base, jail) = scratch("symlink");
        let outside = base.parent().unwrap().join(format!(
            "adi-fs-outside-{}-{:?}",
            std::process::id(),
            std::thread::current().id(),
        ));
        std::fs::write(&outside, b"secret").unwrap();
        std::os::unix::fs::symlink(&outside, base.join("link.txt")).unwrap();
        assert!(matches!(jail.read("link.txt"), Err(Error::Escape(_))));
        let _ = std::fs::remove_file(outside);
        let _ = std::fs::remove_dir_all(base);
    }

    #[test]
    fn metadata_and_exists() {
        let (base, jail) = scratch("meta");
        let meta = jail.metadata("config.toml").unwrap();
        assert!(meta.is_file && !meta.is_dir);
        assert!(jail.exists(".adi/hive.yaml"));
        assert!(!jail.exists("missing"));
        assert!(!jail.exists("../config.toml"));
        let _ = std::fs::remove_dir_all(base);
    }
}
