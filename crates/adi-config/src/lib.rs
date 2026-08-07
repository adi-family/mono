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
mod launch;
mod layout;
mod module;

use std::path::{Path, PathBuf};

pub use error::{Error, Result};
pub use file::{ConfigFile, Timestamped};
pub use launch::{augmented_path, launch_env};
pub use layout::{dir, dir_name, home};
pub use module::Module;

/// The extension every store gives its per-entry manifests: one `<name>.toml` file per named
/// entry (an agent, a trigger, …). Centralized here so registry crates share the convention
/// via [`Module::manifest_file`] / [`Module::remove_manifest`] instead of each re-`format!`-ing
/// it at every call site.
pub const MANIFEST_EXT: &str = "toml";

/// Where the mesh gateway listens, and therefore where the front door sends the whole `n.adi`
/// zone (see `docs/fleet.md` §3).
///
/// It lives here because **two crates that cannot see each other have to agree on it**:
/// `adi-core` writes the front door's `proxy.mesh_gateway` into the generated hive config, and
/// `adi-mesh` binds the listener. Neither depends on the other, and the two ends never
/// handshake — the front door reads a file written once, the gateway binds at start-up — so a
/// number typed twice would fail silently, as a `502` with nothing listening. Both crates
/// already depend on this one, so a single constant makes the compiler keep them in step.
///
/// A fixed port rather than a ports-manager lease for the same reason: a lease that moved
/// between allocations would leave the front door pointing at nothing. `10080` is outside the
/// allocator's `8000..=9999` range (so no project can ever be handed it), clear of `15353` and
/// the `adi daemon` band around it, and in the same fixed `10xxx` band as the resolver's
/// `10053`.
pub const MESH_GATEWAY_PORT: u16 = 10080;

/// Where the *generated* front-door config lives: module dir and file name.
///
/// Two crates have to agree on this for the same reason as [`MESH_GATEWAY_PORT`]: `adi-core`
/// writes this file (it is the front door a node actually runs), and `adi-mesh`'s gateway reads
/// it to answer "which local port serves this service label?". A machine may also carry a
/// hand-managed `hive/hive.yaml` — the richer one that imports projects and dashboards — and
/// where both exist that one wins. But a freshly installed node has *only* this file, so a
/// gateway that knew only the hand-managed path would resolve every label to nothing and refuse
/// every request with "no such service".
pub const FRONTDOOR_MODULE: &str = "dns";

/// The generated front-door config's file name within [`FRONTDOOR_MODULE`].
pub const FRONTDOOR_CONFIG_FILE: &str = "hive-frontdoor.yaml";

/// The mesh gateway's loopback address — [`MESH_GATEWAY_PORT`] on `127.0.0.1`.
///
/// Loopback only, and deliberately: the gateway is reached through the front door, never from
/// off-machine. Binding it anywhere else would hand every service of every paired node to the
/// local network, with no password in front of it until the far side asks for one.
#[must_use]
pub fn mesh_gateway_addr() -> std::net::SocketAddr {
    std::net::SocketAddr::from((std::net::Ipv4Addr::LOCALHOST, MESH_GATEWAY_PORT))
}

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
        Self {
            root: layout::dir(),
        }
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

    /// The shared database directory: `<root>/db`.
    #[must_use]
    pub fn db_dir(&self) -> PathBuf {
        self.root.join(DB_MODULE)
    }

    /// Where a scope's SQLite database lives — `db/global.db` for `None`, else
    /// `db/projects/<project>.db`. Returns `None` for a project id that isn't a safe path segment.
    ///
    /// The *format* belongs to `adi-db`; the store only decides **where**, which is why the rule
    /// lives here: the crates that merely point a run at its database (tools, agents, triggers)
    /// need this path without taking on a SQLite dependency to get it.
    #[must_use]
    pub fn db_path(&self, project: Option<&str>) -> Option<PathBuf> {
        match project {
            None => Some(self.db_dir().join(DB_GLOBAL_FILE)),
            Some(id) if valid_name(id) => Some(
                self.db_dir()
                    .join(DB_PROJECTS_DIR)
                    .join(format!("{id}.{DB_EXT}")),
            ),
            Some(_) => None,
        }
    }

    /// Where every project lives — `projects/`, whether or not anything is registered there yet.
    ///
    /// The root rather than one project: a run that works across several of them (comparing two
    /// checkouts, driving a service from the repo that calls it) has one directory to be told about
    /// instead of a long path per project, and the module name stays spelled in one place.
    #[must_use]
    pub fn projects_dir(&self) -> PathBuf {
        self.module(PROJECTS_MODULE).dir().to_path_buf()
    }

    /// Where a project's directory lives — `projects/<project>`, or `None` when the id is not a
    /// safe path segment or nothing is registered under it.
    ///
    /// Same division of labour as [`Self::db_path`]: the *contents* of a project belong to
    /// `adi-projects`, but the crates that only need to point a run at its project directory
    /// (tools, agents, triggers) get the path here rather than taking on that dependency. The
    /// existence check is deliberate — a manifest may name a project that was since removed, and a
    /// caller resolving a launch directory needs a `None` it can fall back from, not a path that
    /// makes the spawn fail.
    #[must_use]
    pub fn project_dir(&self, project: &str) -> Option<PathBuf> {
        if !valid_name(project) {
            return None;
        }
        let dir = self.module(PROJECTS_MODULE).dir().join(project);
        dir.is_dir().then_some(dir)
    }

    /// The database environment a run is launched with: `ADI_DB` (this scope's file) and
    /// `ADI_DB_DIR` (the store's `db/`). Exporting these is what lets an agent's shell, its `ts`
    /// tools, and the `@adi/db` Bun client all land on the same database without being configured.
    ///
    /// An unusable project id degrades to the global database rather than failing — this feeds
    /// process launches, where a bad id must not be the reason a run won't start.
    #[must_use]
    pub fn db_env(&self, project: Option<&str>) -> Vec<(String, String)> {
        let path = self
            .db_path(project)
            .unwrap_or_else(|| self.db_dir().join(DB_GLOBAL_FILE));
        vec![
            ("ADI_DB".to_string(), path.display().to_string()),
            ("ADI_DB_DIR".to_string(), self.db_dir().display().to_string()),
        ]
    }
}

/// The store module the shared databases live under, the global database's filename, the
/// subdirectory per-project databases live in, and the extension they all carry. See
/// [`Config::db_path`].
const DB_MODULE: &str = "db";
const DB_GLOBAL_FILE: &str = "global.db";
const DB_PROJECTS_DIR: &str = "projects";
const DB_EXT: &str = "db";

/// The store module registered projects live under — one directory per project id. See
/// [`Config::project_dir`].
const PROJECTS_MODULE: &str = "projects";

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
