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
}
