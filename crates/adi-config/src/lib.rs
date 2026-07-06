//! adi-config — the configurator for the adi platform.
//!
//! A [`Config`] store is one directory — the "mono" dir (`~/.adi/mono` by default,
//! honoring `$ADI_DIR`). Each subsystem opens its own [`Module`] — a settings
//! *directory* under it — and within it either:
//!
//! * manages a typed TOML [`ConfigFile`] ([`Module::file`]) — load / default /
//!   create / save, all atomic; or
//! * stores [raw files](Module::write_raw) when it owns its own on-disk format
//!   (JSON, YAML, a log, a socket path, …) and only needs the store to decide *where*.
//!
//! Crates delegate here instead of each re-deriving `$HOME`/`ADI_DIR`/`mono` paths.
//!
//! ```
//! use adi_config::Config;
//! use serde::{Deserialize, Serialize};
//!
//! #[derive(Serialize, Deserialize, Default)]
//! struct HiveSettings { front_door: u16 }
//!
//! # let tmp = std::env::temp_dir().join(format!("adi-config-doctest-{}", std::process::id()));
//! # let store = Config::with_root(&tmp);
//! // In real code: let store = Config::open();
//! let hive = store.module("hive");
//!
//! // A typed TOML config, created from the default on first run.
//! let settings: HiveSettings = hive.file("settings.toml").load_or_create()?;
//!
//! // Or a raw file the module owns the format of.
//! hive.write_raw("hive.yaml", b"proxy: {}\n")?;
//! # let _ = settings.front_door;
//! # std::fs::remove_dir_all(&tmp).ok();
//! # Ok::<(), adi_config::Error>(())
//! ```

mod error;
mod file;
mod fsutil;
mod layout;
mod module;

use std::path::{Path, PathBuf};

pub use error::{Error, Result};
pub use file::ConfigFile;
pub use layout::{dir, dir_name};
pub use module::Module;

/// The settings store: one directory that hands out per-[module](Module) settings
/// directories. Cheap to clone; holds only the root path.
#[derive(Debug, Clone)]
pub struct Config {
    root: PathBuf,
}

impl Default for Config {
    fn default() -> Self {
        Self::open()
    }
}

impl Config {
    /// Open the standard store — the `~/.adi/mono` directory (honoring `$ADI_DIR`).
    #[must_use]
    pub fn open() -> Self {
        Self { root: layout::dir() }
    }

    /// Open a store rooted at an arbitrary directory — for tests or alternate installs.
    #[must_use]
    pub fn with_root(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// The store's root directory.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// A handle to `name`'s settings directory (`<root>/<name>`).
    #[must_use]
    pub fn module(&self, name: &str) -> Module {
        Module::new(self.root.join(name))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn module_dir_is_the_named_child_of_the_root() {
        let store = Config::with_root("/store");
        assert_eq!(store.root(), Path::new("/store"));
        assert_eq!(store.module("hive").dir(), Path::new("/store/hive"));
        assert_eq!(store.module("ports").dir(), Path::new("/store/ports"));
    }

    #[test]
    fn open_roots_the_store_under_the_mono_namespace() {
        assert!(Config::open().root().ends_with("mono"));
    }
}
