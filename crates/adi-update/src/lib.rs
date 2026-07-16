//! adi-update — the auto-update engine for the adi platform.
//!
//! One update covers everything: the published artifact is the whole notarized
//! `ADI.app` bundle (as the release DMG), so every bundled binary — and any binary
//! added in a future release — ships in a single swap. The engine:
//!
//! 1. fetches a small JSON [`Manifest`] (version, DMG url, sha256) from a
//!    configurable URL ([`Settings`], `~/.adi/mono/update/config.toml`);
//! 2. compares the published version with the installed app's `Info.plist`;
//! 3. downloads the DMG, verifies its sha256 **and** its code signature / Team ID;
//! 4. atomically swaps `/Applications/ADI.app` (previous install kept as a backup).
//!
//! Restarting services onto the new binaries is the caller's job (`adi-core`), since
//! that's where the launchd knowledge lives. Everything shells out to the macOS
//! toolchain (`curl`, `shasum`, `hdiutil`, `codesign`, `plutil`) — no network or
//! crypto dependencies.

mod engine;
mod manifest;
mod settings;
mod shell;
mod state;
mod version;

pub use engine::{Check, DEFAULT_APP_PATH, DEFAULT_TEAM_ID, Engine, Error, Installed};
pub use manifest::{Artifact, Manifest};
pub use settings::{DEFAULT_MANIFEST_URL, Settings};
pub use state::State;
pub use version::Version;

/// The version compiled into this build — the workspace version every binary shares.
pub const BUILT_VERSION: &str = env!("CARGO_PKG_VERSION");
