//! adi-update — the auto-update engine for the adi platform.
//!
//! One update covers everything: the published artifact holds *every* binary — the whole
//! notarized `ADI.app` bundle on macOS, the node's tarball of binaries on Linux and
//! Windows — so a binary added in a future release ships in a single swap with no updater
//! change. The engine:
//!
//! 1. fetches a small JSON [`Manifest`] (version, per-platform url + sha256) from a
//!    configurable URL ([`Settings`], `~/.adi/mono/update/config.toml`);
//! 2. compares the published version with what is installed (the bundle's `Info.plist`,
//!    or the `VERSION` file beside the node's binaries);
//! 3. downloads this host's artifact and verifies its sha256 — plus, on macOS, the
//!    bundle's code signature and Team ID;
//! 4. runs the downloaded CLI to confirm it starts and reports the promised version;
//! 5. swaps the live install, keeping the previous one as a backup to roll back to.
//!
//! Restarting services onto the new binaries — and rolling back when they don't come up —
//! is the caller's job (`adi-core`), since that's where the supervisor knowledge lives.
//! Everything shells out to tools the OS already ships (`curl`, `tar`, and on macOS
//! `hdiutil`, `codesign`, `plutil`) — no network dependency. The one exception is the
//! artifact's sha256, hashed in-process: there is no checksum tool all three platforms
//! have, and the macOS-only `shasum` this used to call is absent on every stock Linux.

mod engine;
mod manifest;
mod payload;
mod settings;
mod shell;
mod state;
mod version;

pub use engine::{Check, DEFAULT_TEAM_ID, Engine, Error, Installed, default_app_path, sha256};
pub use manifest::{Artifact, Manifest, host_platform};
pub use settings::{DEFAULT_MANIFEST_URL, Settings};
pub use state::State;
pub use version::Version;

/// The version compiled into this build.
///
/// The git tag is the release's source of truth (`scripts/version.sh`), and the packaging
/// scripts export it as `ADI_VERSION` before invoking cargo — so a released binary reports
/// the tag it was cut from, while a plain `cargo build` falls back to the workspace version.
/// `build.rs` makes cargo notice when that variable changes.
pub const BUILT_VERSION: &str = match option_env!("ADI_VERSION") {
    Some(v) if !v.is_empty() => v,
    _ => env!("CARGO_PKG_VERSION"),
};
