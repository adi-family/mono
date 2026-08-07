//! The one directory the adi settings store lives in: `$HOME/$ADI_DIR/mono` — the
//! "mono" dir. Callers deal with a single directory ([`dir`]), never a composed
//! `.adi` + `mono` path. `ADI_DIR` (default `.adi`) stays the one knob for pointing
//! the whole store elsewhere (e.g. a root daemon pinned to the installing user's dir).

use std::path::PathBuf;

const ADI_DIR_ENV: &str = "ADI_DIR";
const DEFAULT_ADI_DIR: &str = ".adi";
const MONO_DIR: &str = "mono";

/// The user's home directory. On Unix that's `$HOME` (matching `NSHomeDirectory`-style
/// fallbacks); on Windows, where `HOME` is usually unset, it's `%USERPROFILE%` (then the
/// `%HOMEDRIVE%%HOMEPATH%` pair). Falls back to the platform root if nothing is set.
#[must_use]
pub fn home() -> PathBuf {
    if let Some(h) = std::env::var_os("HOME").filter(|h| !h.is_empty()) {
        return PathBuf::from(h);
    }
    #[cfg(windows)]
    {
        if let Some(profile) = std::env::var_os("USERPROFILE").filter(|p| !p.is_empty()) {
            return PathBuf::from(profile);
        }
        if let (Some(drive), Some(path)) = (
            std::env::var_os("HOMEDRIVE").filter(|p| !p.is_empty()),
            std::env::var_os("HOMEPATH").filter(|p| !p.is_empty()),
        ) {
            let mut p = std::path::PathBuf::from(drive);
            p.push(path);
            return p;
        }
        return PathBuf::from("C:\\");
    }
    #[cfg(not(windows))]
    PathBuf::from("/")
}

/// The `ADI_DIR` value, trimmed; empty/unset falls back to `.adi`.
fn resolve_dir_name(env: Option<&str>) -> String {
    match env {
        Some(v) if !v.trim().is_empty() => v.trim().to_string(),
        _ => DEFAULT_ADI_DIR.to_string(),
    }
}

/// The `ADI_DIR` name (`.adi` by default) — the knob a caller pins when a process must
/// resolve the store as a specific user (e.g. staging a root daemon). Not a directory
/// callers navigate; the store is [`dir`].
#[must_use]
pub fn dir_name() -> String {
    resolve_dir_name(std::env::var(ADI_DIR_ENV).ok().as_deref())
}

/// The store's single directory: `$HOME/<ADI_DIR>/mono`.
#[must_use]
pub fn dir() -> PathBuf {
    home().join(dir_name()).join(MONO_DIR)
}

/// The `ADI_DIR` root itself: `$HOME/<ADI_DIR>` — the parent of [`dir`]. Callers want
/// [`dir`]; this exists for the few things that belong to the whole store rather than to
/// the settings inside it, like the indexer opt-out below.
#[must_use]
pub fn root() -> PathBuf {
    home().join(dir_name())
}

/// The marker that tells macOS Spotlight to skip a directory and everything under it.
#[cfg(target_os = "macos")]
const NEVER_INDEX: &str = ".metadata_never_index";

/// Keep the store out of the platform's search index.
///
/// The store lives under `$HOME`, so Spotlight indexes it by default — and it grows to
/// tens of gigabytes of sessions, transcripts, caches and vendored dependencies that
/// nobody has ever searched for from Spotlight. The cost is not theoretical: `mds` and its
/// workers saturate the disk, and every other process on the machine queues behind them.
/// An empty `.metadata_never_index` at the root opts out the entire tree.
///
/// Called on the path that creates store directories, so a fresh install is excluded from
/// its first write rather than after someone notices the machine crawling. It runs its
/// work once per process and **never reports failure**: an unwritable marker is a
/// performance problem, not a correctness one, and no caller should fail a config write
/// over it.
///
/// A no-op off macOS, which is the only platform with this convention.
pub fn ensure_root_not_indexed() {
    #[cfg(target_os = "macos")]
    {
        use std::sync::Once;
        // The filesystem state is process-wide, so checking it once is enough; this keeps
        // the two hot creation paths from paying a `stat` on every write.
        static ONCE: Once = Once::new();
        ONCE.call_once(|| mark_never_indexed(&root()));
    }
}

/// Put the marker in `root`, creating the directory if it is not there yet.
///
/// Split out from [`ensure_root_not_indexed`] so it is reachable from a test — the `Once`
/// wrapper can only ever fire for one directory per process.
#[cfg(target_os = "macos")]
fn mark_never_indexed(root: &std::path::Path) {
    if std::fs::create_dir_all(root).is_err() {
        return;
    }
    let marker = root.join(NEVER_INDEX);
    if marker.exists() {
        return;
    }
    // `create_new` so two processes racing here cannot truncate each other's file; the
    // loser just finds it already present.
    let _ = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&marker);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dir_name_prefers_env_when_present() {
        assert_eq!(resolve_dir_name(Some(".custom")), ".custom");
        assert_eq!(resolve_dir_name(Some("  spaced  ")), "spaced");
    }

    #[test]
    fn dir_name_falls_back_to_default() {
        assert_eq!(resolve_dir_name(None), DEFAULT_ADI_DIR);
        assert_eq!(resolve_dir_name(Some("   ")), DEFAULT_ADI_DIR);
        assert_eq!(resolve_dir_name(Some("")), DEFAULT_ADI_DIR);
    }

    #[test]
    fn store_dir_is_the_mono_dir_under_home() {
        let dir = dir();
        assert!(dir.ends_with(MONO_DIR), "got {}", dir.display());
        assert!(dir.starts_with(home()));
    }

    #[test]
    fn store_root_is_the_parent_of_the_mono_dir() {
        assert_eq!(dir().parent(), Some(root().as_path()));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn marks_the_store_root_never_indexed() {
        let root = std::env::temp_dir().join(format!(
            "adi-config-layout-noindex-{}-{:?}",
            std::process::id(),
            std::thread::current().id(),
        ));
        let _ = std::fs::remove_dir_all(&root);

        // Creates the directory it is given, so a first-ever run is covered.
        mark_never_indexed(&root);
        let marker = root.join(NEVER_INDEX);
        assert!(marker.is_file(), "marker missing at {}", marker.display());

        // Idempotent, and it must not clobber a marker that is already there — a second
        // run is the normal case, not the exception.
        std::fs::write(&marker, b"existing").expect("seed");
        mark_never_indexed(&root);
        assert_eq!(std::fs::read(&marker).expect("read"), b"existing");

        let _ = std::fs::remove_dir_all(&root);
    }
}
