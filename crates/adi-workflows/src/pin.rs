//! Pinning a workflow run to its file's content, not its path — spec section E: "A run stays
//! pinned to the file version it started with." [`crate::pour`] already hashes the file at pour
//! time and stores that hash on the run row; this module is what makes the hash actually *do*
//! something, by keeping a content-addressed copy every attempt reads instead of the live file.
//! Without it, editing `workflows/weekly.ts` while a run of it is still in flight would change
//! what the next attempt executes mid-run — exactly the hazard the spec sentence rules out.
//!
//! Written directly under the store root (`<root>/workflows-versions/<hash>.ts`), sibling to
//! `node_modules`, so a pinned copy's own `import`s resolve `@adi/workflows`/`zod` by the same
//! upward search every other `.ts` this platform runs relies on — moving a file to a different
//! directory must never break what it imports.

use std::path::PathBuf;

use adi_config::Config;

use crate::error::{Error, Result};

const VERSIONS_DIR: &str = "workflows-versions";

/// The pinned copy's path for a given content hash — present or not.
#[must_use]
pub fn pinned_path(config: &Config, hash: &str) -> PathBuf {
    config.root().join(VERSIONS_DIR).join(format!("{hash}.ts"))
}

/// Write the pinned copy for `hash` if it doesn't already exist. Idempotent and cheap to call on
/// every pour: once a hash's copy exists, every later run sharing that exact content (two
/// schedules firing the same unedited file, say) reuses it rather than rewriting the same bytes.
///
/// # Errors
/// [`Error::Store`] if the versions directory or the file can't be written.
pub fn ensure_pinned(config: &Config, hash: &str, content: &[u8]) -> Result<PathBuf> {
    let path = pinned_path(config, hash);
    if path.is_file() {
        return Ok(path);
    }
    let dir = path.parent().expect("pinned path has the versions dir as parent");
    std::fs::create_dir_all(dir)
        .map_err(|e| Error::Store(anyhow::anyhow!("couldn't create {}: {e}", dir.display())))?;
    std::fs::write(&path, content)
        .map_err(|e| Error::Store(anyhow::anyhow!("couldn't write {}: {e}", path.display())))?;
    Ok(path)
}

/// Read back the already-pinned path for `hash`, or an error if nothing was ever pinned under it
/// — a run whose pour never completed [`ensure_pinned`] cannot be attempted.
///
/// # Errors
/// [`Error::Store`] if the file is missing or can't be found.
pub fn require_pinned(config: &Config, hash: &str) -> Result<PathBuf> {
    let path = pinned_path(config, hash);
    if !path.is_file() {
        return Err(Error::Store(anyhow::anyhow!(
            "no pinned copy for file hash {hash} at {}",
            path.display()
        )));
    }
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> Config {
        let root = std::env::temp_dir().join(format!(
            "adi-workflows-pin-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id(),
        ));
        let _ = std::fs::remove_dir_all(&root);
        Config::with_root(root)
    }

    #[test]
    fn a_pinned_copy_is_written_once_and_read_back() {
        let config = scratch("roundtrip");
        let path = ensure_pinned(&config, "deadbeef", b"export const on = [];").expect("pin");
        assert_eq!(
            std::fs::read(&path).expect("read pinned"),
            b"export const on = [];"
        );
        assert_eq!(require_pinned(&config, "deadbeef").expect("require"), path);
    }

    #[test]
    fn requiring_an_unpinned_hash_is_an_error() {
        let config = scratch("missing");
        assert!(require_pinned(&config, "nope").is_err());
    }

    /// Pinning is a no-op once the copy exists — proven the cheap, decisive way: writing
    /// different bytes under an already-pinned hash never overwrites it.
    #[test]
    fn pinning_the_same_hash_twice_never_rewrites_it() {
        let config = scratch("idempotent");
        ensure_pinned(&config, "h", b"first").expect("first pin");
        ensure_pinned(&config, "h", b"second").expect("second pin");
        assert_eq!(
            std::fs::read(pinned_path(&config, "h")).expect("read"),
            b"first"
        );
    }
}
