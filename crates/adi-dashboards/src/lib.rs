//! adi-dashboards — the on-disk contract of a dashboard directory.
//!
//! A dashboard is a bun-served frontend/backend pair under `~/.adi/mono/dashboards/<id>/`, and
//! everything about one that two subsystems have to agree on lives here: its `config.toml`
//! [`manifest`], the `.adi/hive.yaml` the supervisor and the front door both read — and the
//! one hostname its two services claim ([`hive`]) — and the [`bundle`] a dashboard becomes when
//! it travels: between machines over the mesh, or from a marketplace onto this one.
//!
//! The crate is the extraction of those rules from `adi-webapp-api`'s handlers, done when the
//! marketplace needed the same landing path the panel's import uses. Two implementations of
//! "put a dashboard on disk" would be two chances to get the path jail or the one-origin hive
//! file wrong, and the jail is the one that matters: a bundle is somebody else's directory.
//!
//! It owns *shapes and rules*, not policy. Deciding when a dashboard is migrated, listed, or
//! archived stays with the caller; this crate answers what a valid dashboard directory is and
//! what may be written into one.

pub mod bundle;
pub mod hive;
pub mod manifest;

pub use bundle::{
    BundleError, BundleFile, CollectError, DashboardBundle, DecodedFiles, KEPT_ON_IMPORT,
    MAX_BUNDLE_BYTES, MAX_BUNDLE_FILES, NEVER_BUNDLED_DIRS, NEVER_BUNDLED_ROOT_FILES,
    clear_imported, collect_files, decode_bundle, valid_id, write_import,
};
pub use hive::{
    API_PATH, HIVE_ARCHIVED, HIVE_LIVE, HOST_ZONE, HiveFile, dashboard_host, declared_host,
    hive_yaml, is_one_origin, parse_hive, preferred_host,
};
pub use manifest::{Manifest, read_manifest, write_manifest};
