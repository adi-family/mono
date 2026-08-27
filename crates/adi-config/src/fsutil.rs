//! Small filesystem helpers shared by the config-file and raw-store code.
//!
//! # The store is private
//!
//! Everything under `~/.adi/mono` belongs to one person, and some of it is worth exactly as much
//! as the machine: `mesh/identity.key` is the node's long-term iroh secret, and
//! `mesh/invites.toml` plus `mesh/ticket` are together a *complete* invite — a live single-use
//! nonce and the address to dial it at, which is what the join handshake accepts from a key no
//! registry has ever seen. Until this module set a mode, all of it landed at whatever the umask
//! said, which is `0644`/`0755` on a stock macOS account — and macOS puts every local account in
//! `staff`, so a second account on the machine could read all of it. Backups, Time Machine and
//! any sync of the store carried the same bytes.
//!
//! So writes here are private by construction:
//!
//! * [`atomic_write`] creates its temp file `0600` **at creation**, with `OpenOptions::mode`, and
//!   only then renames. Not a `chmod` after the write and certainly not one after the rename:
//!   either would leave a window in which the finished bytes are readable, and the whole point of
//!   the temp-file dance is that no such window exists.
//! * [`harden_dir`] leaves a directory `0700`, and [`harden_existing`] repairs a file that was
//!   written before any of this — which is what stops it being fixed for new installs only. Every
//!   store already on disk has an `identity.key` at `0644` and nothing rewrites it.
//!
//! # Repairing what is already there, without walking the store
//!
//! [`harden_existing`] is called on the files this crate *reads*, not by a sweep of the tree. The
//! store is not only config: `projects/` and `dashboards/` on a working machine are 18 GB and
//! 50 GB of checked-out code, and a recursive `chmod` over that would be both slow and wrong — it
//! would strip the executable bit off every script it passed. Hardening what we touch reaches
//! exactly the files this crate owns, the first time anything opens them, and reaches nothing
//! else.
//!
//! That is also why the repair keeps the owner's bits and drops only group and other
//! (`mode & 0o700`): `0644` becomes `0600`, and a `0755` tool script becomes `0700` — still
//! executable, which it would not be if the repair forced `0600` the way a fresh write does.
//!
//! # What is deliberately not private
//!
//! Two files in the store cross a privilege boundary and are published on purpose:
//! `ports/registry.json` and `hive/status.json` are written by the **root** front door and read by
//! per-user tools, so they are `0644`. Neither is written through this module — each has its own
//! writer that sets the mode explicitly (`adi-ports-manager`'s `Registry::save`, `adi-osext`'s
//! `write_status_file`) — so there is nothing here to exempt, and no `write_public` sibling is
//! needed. If one is ever wanted, that is the shape: opt out by name, with the private path
//! staying the default.
//!
//! All of this is `#[cfg(unix)]`. Windows has no POSIX mode, and the store already sits under the
//! per-user profile directory.

use std::io;
use std::path::Path;

/// Write `bytes` to `path` atomically: create parents, write a per-pid temp file,
/// then rename it into place so a reader never observes a half-written file.
///
/// The result is `0600`, and it is `0600` from the moment the temp file exists — see this
/// module's header.
pub(crate) fn atomic_write(path: &Path, bytes: &[u8]) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        // The other path that brings store directories into existence, so it carries the
        // same guarantee as `Module::ensure_dir`: a write can be the very first thing that
        // creates the store, and it should not leave it indexable.
        crate::layout::ensure_root_not_indexed();
        std::fs::create_dir_all(parent)?;
        harden_dir(parent);
    }

    // Per-pid temp name keeps concurrent writers from clobbering each other's temp file.
    let file_name = path.file_name().map_or_else(
        || "config".to_string(),
        |n| n.to_string_lossy().into_owned(),
    );
    let tmp = path.with_file_name(format!("{file_name}.{}.tmp", std::process::id()));

    write_private(&tmp, bytes)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

/// Write `bytes` to `path`, owner-only from the instant the file exists.
///
/// The mode is passed to `open(2)`, not applied afterwards: a `chmod` after the write is a window,
/// however short, in which the bytes are readable by everyone. An existing file is truncated and
/// keeps whatever mode it already had, which is why [`atomic_write`] always writes a *fresh* temp
/// name rather than reusing one.
pub(crate) fn write_private(path: &Path, bytes: &[u8]) -> io::Result<()> {
    use std::io::Write as _;

    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    file.write_all(bytes)?;
    file.sync_all()
}

/// Leave a directory owner-only (`0700`). Best-effort: a directory somebody else owns — the root
/// front door writes into `hive/` — is not ours to change, and failing the caller's write over it
/// would be worse than leaving the mode alone.
pub(crate) fn harden_dir(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let Ok(meta) = std::fs::metadata(path) else {
            return;
        };
        let mode = meta.permissions().mode();
        if mode & 0o077 != 0 {
            let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode & 0o700));
        }
    }
    #[cfg(not(unix))]
    let _ = path;
}

/// Take group and other access away from a file that predates this module, keeping the owner's own
/// bits — so an executable stays executable (see this module's header).
///
/// Best-effort and silent, for the same reason as [`harden_dir`]: it runs on the read path, and a
/// file we may read but not chmod must still be readable.
pub(crate) fn harden_existing(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let Ok(meta) = std::fs::metadata(path) else {
            return;
        };
        if !meta.is_file() {
            return;
        }
        let mode = meta.permissions().mode();
        if mode & 0o077 != 0 {
            let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode & 0o700));
        }
    }
    #[cfg(not(unix))]
    let _ = path;
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt as _;

    fn scratch(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "adi-config-fsutil-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("scratch");
        dir
    }

    fn mode(path: &Path) -> u32 {
        std::fs::metadata(path)
            .expect("metadata")
            .permissions()
            .mode()
            & 0o7777
    }

    #[test]
    fn a_written_config_file_is_owner_only() {
        let dir = scratch("write");
        let path = dir.join("identity.key");
        atomic_write(&path, b"secret").expect("write");
        assert_eq!(mode(&path), 0o600);
        // …and so is the directory it was written into.
        assert_eq!(mode(&dir), 0o700);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_write_creates_missing_parents_owner_only() {
        let dir = scratch("parents");
        let path = dir.join("mesh/invites.toml");
        atomic_write(&path, b"[invites]\n").expect("write");
        assert_eq!(mode(&path), 0o600);
        assert_eq!(mode(&dir.join("mesh")), 0o700);
        std::fs::remove_dir_all(&dir).ok();
    }

    /// The mode is set on the temp file *before* the rename, so the finished bytes are never
    /// group-readable for even an instant. Asserted on the temp-file writer itself, since the
    /// temp file `atomic_write` makes is gone by the time it returns.
    #[test]
    fn the_temp_file_is_owner_only_from_the_moment_it_exists() {
        let dir = scratch("tmp");
        let tmp = dir.join("registry.json.1234.tmp");
        write_private(&tmp, b"{}").expect("write");
        assert_eq!(
            mode(&tmp),
            0o600,
            "0o{:o} — group could read the temp file",
            mode(&tmp)
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    /// The repair path: a file written before any of this, fixed the next time it is touched.
    #[test]
    fn an_old_world_readable_file_is_repaired_without_losing_its_owner_bits() {
        let dir = scratch("repair");
        let key = dir.join("identity.key");
        std::fs::write(&key, b"old").expect("write");
        std::fs::set_permissions(&key, std::fs::Permissions::from_mode(0o644)).expect("chmod");
        harden_existing(&key);
        assert_eq!(mode(&key), 0o600);

        // A tool script must stay executable — dropping to 0600 would break every agent's shim.
        let script = dir.join("tool.sh");
        std::fs::write(&script, b"#!/bin/sh\n").expect("write");
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).expect("chmod");
        harden_existing(&script);
        assert_eq!(mode(&script), 0o700, "the owner's execute bit must survive");

        // Already private: left exactly as it is.
        harden_existing(&key);
        assert_eq!(mode(&key), 0o600);
        std::fs::remove_dir_all(&dir).ok();
    }

    /// A directory is repaired the same way, and a missing path is not an error.
    #[test]
    fn hardening_tolerates_what_it_cannot_change() {
        let dir = scratch("tolerate");
        let sub = dir.join("mesh");
        std::fs::create_dir_all(&sub).expect("mkdir");
        std::fs::set_permissions(&sub, std::fs::Permissions::from_mode(0o755)).expect("chmod");
        harden_dir(&sub);
        assert_eq!(mode(&sub), 0o700);

        // Neither call may panic on something that is not there, or on the wrong kind of thing.
        harden_existing(&dir.join("nope"));
        harden_dir(&dir.join("nope"));
        harden_existing(&sub);
        assert_eq!(mode(&sub), 0o700, "a directory is not a file to harden");
        std::fs::remove_dir_all(&dir).ok();
    }
}
