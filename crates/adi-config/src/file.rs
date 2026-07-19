//! A [`ConfigFile<T>`] is a typed TOML settings file within a module directory:
//! load it (optionally defaulting or creating it), and save it back atomically.

use std::marker::PhantomData;
use std::path::{Path, PathBuf};

use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::error::{Error, Result};
use crate::fsutil::atomic_write;

/// A handle to one TOML config file. The type parameter `T` is the config shape it
/// (de)serializes; the handle is cheap and holds no file descriptor.
#[derive(Debug, Clone)]
pub struct ConfigFile<T> {
    path: PathBuf,
    // `fn() -> T` so the handle is `Send + Sync` regardless of `T`.
    _marker: PhantomData<fn() -> T>,
}

impl<T> ConfigFile<T> {
    pub(crate) fn new(path: PathBuf) -> Self {
        Self {
            path,
            _marker: PhantomData,
        }
    }

    /// The file's path on disk.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// True if the file currently exists.
    #[must_use]
    pub fn exists(&self) -> bool {
        self.path.exists()
    }

    fn parse(&self, raw: &str) -> Result<T>
    where
        T: DeserializeOwned,
    {
        toml::from_str(raw).map_err(|source| Error::Parse {
            path: self.path.clone(),
            source,
        })
    }
}

impl<T: DeserializeOwned> ConfigFile<T> {
    /// Load and parse the file. A missing file is an error — use
    /// [`load_or_default`](Self::load_or_default) or
    /// [`load_or_create`](Self::load_or_create) to tolerate absence.
    ///
    /// # Errors
    /// [`Error::Io`] if the file cannot be read; [`Error::Parse`] on invalid TOML.
    pub fn load(&self) -> Result<T> {
        let raw = std::fs::read_to_string(&self.path)?;
        self.parse(&raw)
    }
}

impl<T: DeserializeOwned + Default> ConfigFile<T> {
    /// Load the file, or return `T::default()` if it does not exist. Unlike
    /// [`load_or_create`](Self::load_or_create) this does not write anything.
    ///
    /// # Errors
    /// [`Error::Io`] on a read failure other than not-found; [`Error::Parse`] on invalid TOML.
    pub fn load_or_default(&self) -> Result<T> {
        match std::fs::read_to_string(&self.path) {
            Ok(raw) => self.parse(&raw),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(T::default()),
            Err(e) => Err(e.into()),
        }
    }
}

impl<T: Serialize> ConfigFile<T> {
    /// Serialize `value` to pretty TOML and write it atomically, creating parents.
    ///
    /// # Errors
    /// [`Error::Encode`] if the value cannot be serialized; [`Error::Io`] on write failure.
    pub fn save(&self, value: &T) -> Result<()> {
        let toml = toml::to_string_pretty(value).map_err(|source| Error::Encode {
            path: self.path.clone(),
            source,
        })?;
        atomic_write(&self.path, toml.as_bytes())?;
        Ok(())
    }
}

impl<T: Serialize + DeserializeOwned + Default> ConfigFile<T> {
    /// Load the file, first materializing it from `T::default()` when absent so the
    /// on-disk config exists for the user to edit afterwards.
    ///
    /// # Errors
    /// Any [`Error`] from the underlying save or load.
    pub fn load_or_create(&self) -> Result<T> {
        if self.exists() {
            return self.load();
        }
        let value = T::default();
        self.save(&value)?;
        Ok(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;

    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    struct Settings {
        name: String,
        port: u16,
    }

    impl Default for Settings {
        fn default() -> Self {
            Self {
                name: "default".to_string(),
                port: 8080,
            }
        }
    }

    fn scratch(tag: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "adi-config-file-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id(),
        ))
    }

    #[test]
    fn save_then_load_round_trips() {
        let dir = scratch("roundtrip");
        let _ = std::fs::remove_dir_all(&dir);
        let file: ConfigFile<Settings> = ConfigFile::new(dir.join("settings.toml"));

        let value = Settings {
            name: "hive".to_string(),
            port: 8009,
        };
        file.save(&value).expect("save");
        assert!(file.exists());
        assert_eq!(file.load().expect("load"), value);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_missing_is_an_error_but_default_is_not() {
        let dir = scratch("missing");
        let _ = std::fs::remove_dir_all(&dir);
        let file: ConfigFile<Settings> = ConfigFile::new(dir.join("settings.toml"));

        assert!(matches!(file.load(), Err(Error::Io(_))));
        assert_eq!(
            file.load_or_default().expect("default"),
            Settings::default()
        );
        assert!(!file.exists(), "load_or_default must not write");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_or_create_materializes_the_default_then_reads_it_back() {
        let dir = scratch("create");
        let _ = std::fs::remove_dir_all(&dir);
        let file: ConfigFile<Settings> = ConfigFile::new(dir.join("settings.toml"));

        let created = file.load_or_create().expect("create");
        assert_eq!(created, Settings::default());
        assert!(file.exists(), "load_or_create must persist the default");

        assert_eq!(file.load_or_create().expect("reload"), Settings::default());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn invalid_toml_surfaces_as_a_parse_error() {
        let dir = scratch("corrupt");
        let file: ConfigFile<Settings> = ConfigFile::new(dir.join("settings.toml"));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(file.path(), b"name = \nport = ").unwrap();
        assert!(matches!(file.load(), Err(Error::Parse { .. })));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
