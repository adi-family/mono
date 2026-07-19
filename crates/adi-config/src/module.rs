//! A [`Module`] is one subsystem's settings directory under the store root
//! (e.g. `~/.adi/mono/hive`). Within it a module manages typed TOML config files
//! ([`Module::file`]) and raw file storage ([`Module::write_raw`] & friends).

use std::path::{Path, PathBuf};

use crate::error::Result;
use crate::file::ConfigFile;
use crate::fsutil::atomic_write;

/// A handle to a module's settings directory. Cheap to clone; creates nothing on
/// disk until a write happens (or [`ensure_dir`](Self::ensure_dir) is called).
#[derive(Debug, Clone)]
pub struct Module {
    dir: PathBuf,
}

impl Module {
    pub(crate) fn new(dir: PathBuf) -> Self {
        Self { dir }
    }

    /// This module's settings directory.
    #[must_use]
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Create the module directory (and any missing parents), returning it.
    ///
    /// # Errors
    /// [`Error::Io`](crate::Error::Io) if the directory cannot be created.
    pub fn ensure_dir(&self) -> Result<&Path> {
        std::fs::create_dir_all(&self.dir)?;
        Ok(&self.dir)
    }

    /// A typed TOML config file named `name` within this module directory.
    #[must_use]
    pub fn file<T>(&self, name: &str) -> ConfigFile<T> {
        ConfigFile::new(self.dir.join(name))
    }

    /// The typed `<name>.toml` manifest for a named entry — the file a registry keeps one of per
    /// agent / trigger. A convenience over [`file`](Self::file) so the `<name>.`[`MANIFEST_EXT`]
    /// naming convention lives here rather than at each registry's call site.
    ///
    /// [`MANIFEST_EXT`]: crate::MANIFEST_EXT
    #[must_use]
    pub fn manifest_file<T>(&self, name: &str) -> ConfigFile<T> {
        self.file(&format!("{name}.{}", crate::MANIFEST_EXT))
    }

    /// Remove a named entry's `<name>.toml` manifest, returning whether it existed. The delete
    /// half of [`manifest_file`](Self::manifest_file): a convenience over
    /// [`remove_raw`](Self::remove_raw) that keeps the `<name>.toml` convention in one place.
    ///
    /// # Errors
    /// [`Error::Io`](crate::Error::Io) on any removal failure other than not-found.
    pub fn remove_manifest(&self, name: &str) -> Result<bool> {
        self.remove_raw(&format!("{name}.{}", crate::MANIFEST_EXT))
    }

    /// Where a raw file named `name` lives (does not touch disk). Use this when the
    /// module owns its own file format (JSON, YAML, a log, a socket path, …) and only
    /// needs the store to decide *where* it goes.
    #[must_use]
    pub fn raw_path(&self, name: &str) -> PathBuf {
        self.dir.join(name)
    }

    /// Read a raw file's bytes; a missing file is `Ok(None)`.
    ///
    /// # Errors
    /// [`Error::Io`](crate::Error::Io) on any read failure other than not-found.
    pub fn read_raw(&self, name: &str) -> Result<Option<Vec<u8>>> {
        match std::fs::read(self.raw_path(name)) {
            Ok(bytes) => Ok(Some(bytes)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// Atomically write `bytes` to a raw file named `name`, creating the module dir.
    ///
    /// # Errors
    /// [`Error::Io`](crate::Error::Io) if the directory or file cannot be written.
    pub fn write_raw(&self, name: &str, bytes: &[u8]) -> Result<()> {
        atomic_write(&self.raw_path(name), bytes)?;
        Ok(())
    }

    /// Remove a raw file; a missing file is `Ok(false)`.
    ///
    /// # Errors
    /// [`Error::Io`](crate::Error::Io) on any removal failure other than not-found.
    pub fn remove_raw(&self, name: &str) -> Result<bool> {
        match std::fs::remove_file(self.raw_path(name)) {
            Ok(()) => Ok(true),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(e) => Err(e.into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "adi-config-module-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id(),
        ))
    }

    #[test]
    fn file_and_raw_paths_sit_inside_the_module_dir() {
        let module = Module::new(PathBuf::from("/store/hive"));
        assert_eq!(module.dir(), Path::new("/store/hive"));
        assert_eq!(
            module.file::<()>("hive.toml").path(),
            Path::new("/store/hive/hive.toml")
        );
        assert_eq!(
            module.raw_path("hive.yaml"),
            Path::new("/store/hive/hive.yaml")
        );
    }

    #[test]
    fn raw_store_round_trips_and_reports_absence() {
        let dir = scratch("raw");
        let _ = std::fs::remove_dir_all(&dir);
        let module = Module::new(dir.clone());

        assert_eq!(module.read_raw("blob.bin").expect("read missing"), None);
        assert!(!module.remove_raw("blob.bin").expect("remove missing"));

        module.write_raw("blob.bin", b"hello").expect("write");
        assert_eq!(
            module.read_raw("blob.bin").expect("read"),
            Some(b"hello".to_vec())
        );
        assert!(module.remove_raw("blob.bin").expect("remove"));
        assert_eq!(module.read_raw("blob.bin").expect("read again"), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn ensure_dir_creates_the_directory() {
        let dir = scratch("ensure");
        let _ = std::fs::remove_dir_all(&dir);
        let module = Module::new(dir.join("nested"));
        assert!(!module.dir().exists());
        module.ensure_dir().expect("ensure");
        assert!(module.dir().is_dir());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
