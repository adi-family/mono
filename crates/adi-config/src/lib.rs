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

/// Whether `name` is a single, filesystem-safe path segment — the rule every store applies
/// before joining `<name>.toml` onto a [`Module`] directory. This is a security boundary: names
/// arrive from CLIs, HTTP APIs, and webhook URL paths. A name must be non-empty, must not be
/// `.` or `..`, and may contain only ASCII alphanumerics, `.`, `-`, or `_`. Callers map a
/// `false` onto their own `InvalidName` error.
#[must_use]
pub fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name != "."
        && name != ".."
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'))
}

/// Check `name` with [`valid_name`], mapping a rejection through `err` into the caller's own error
/// type. Every store owns an `InvalidName(String)` variant but a distinct `Error` enum, so pass
/// the variant itself as the constructor: `validate_name(name, Error::InvalidName)`.
///
/// # Errors
/// Returns `err(name)` when [`valid_name`] rejects `name`.
pub fn validate_name<E>(name: &str, err: impl FnOnce(String) -> E) -> std::result::Result<(), E> {
    if valid_name(name) {
        Ok(())
    } else {
        Err(err(name.to_string()))
    }
}

/// Seconds since the Unix epoch, saturating to 0 if the clock is somehow before it. Stores stamp
/// this onto their manifests (`created_at` / `updated_at`).
#[must_use]
pub fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
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

    #[test]
    fn valid_name_accepts_safe_segments_and_rejects_traversal() {
        for name in ["athz-solver", "planner", "agent_2", "a.b"] {
            assert!(valid_name(name), "{name} should be valid");
        }
        for name in ["", ".", "..", "a/b", "a\\b", "with space"] {
            assert!(!valid_name(name), "{name:?} should be rejected");
        }
    }

    #[test]
    fn now_unix_is_after_the_epoch() {
        assert!(now_unix() > 0);
    }

    #[test]
    fn validate_name_maps_rejections_through_the_constructor() {
        assert_eq!(validate_name("ok", |n| n), Ok(()));
        assert_eq!(validate_name("a/b", |n| n), Err("a/b".to_string()));
    }
}
