//! How the allocator is parameterized: the range it hands ports out of, the ranges
//! it must never touch, and where static leases persist.
//!
//! The defaults encode two hard facts about this machine (see `adi-family/CLAUDE.md`):
//! privileged ports (`0..=1023`) need root and are off-limits to unprivileged dev
//! services, and the `adi daemon` supervisor — including the protected ADI DNS on
//! `127.0.0.1:15353` — lives in a band around `15353` that must never be collided with.

use std::ops::RangeInclusive;
use std::path::PathBuf;

const ADI_DIR_ENV: &str = "ADI_DIR";
const DEFAULT_ADI_DIR: &str = ".adi";
const MONO_SUBDIR: &str = "mono";
const PORTS_SUBDIR: &str = "ports";
const REGISTRY_FILE: &str = "registry.json";

/// Default allocatable range — the 8000s, where the mono dev services already sit
/// (`app.adi` → 8010, `api.adi` → 8009, …). Wide enough to never realistically run dry.
const DEFAULT_RANGE: RangeInclusive<u16> = 8000..=9999;

/// Privileged ports: binding these needs root, and adi dev services are unprivileged,
/// so they are never handed out.
const PRIVILEGED_BAND: RangeInclusive<u16> = 0..=1023;

/// The `adi daemon` supervisor band. ADI DNS listens on `127.0.0.1:15353` and is
/// critical infra that must never be disturbed; reserve a generous margin around it.
const ADI_DAEMON_BAND: RangeInclusive<u16> = 15000..=15999;

/// Parameters controlling allocation. Construct [`Config::default`] for the standard
/// setup, or build one by hand to widen the range, add reserved bands, or point the
/// registry somewhere else (tests do the latter).
#[derive(Debug, Clone)]
pub struct Config {
    /// The inclusive range ports are drawn from.
    pub range: RangeInclusive<u16>,
    /// Ranges that are never handed out even when they fall inside [`Config::range`].
    pub reserved: Vec<RangeInclusive<u16>>,
    /// Where static (reserved) leases are persisted as JSON.
    pub registry_path: PathBuf,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            range: DEFAULT_RANGE,
            reserved: vec![PRIVILEGED_BAND, ADI_DAEMON_BAND],
            registry_path: default_registry_path(),
        }
    }
}

impl Config {
    /// The standard configuration — identical to [`Config::default`], spelled as a
    /// constructor for call sites that read better that way.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// True if `port` falls in any reserved band. Independent of [`Config::range`]:
    /// a port can be out of range *and* reserved.
    #[must_use]
    pub fn is_reserved(&self, port: u16) -> bool {
        self.reserved.iter().any(|band| band.contains(&port))
    }
}

/// `$HOME`, or `/` if unset — the same fallback `adi-core`'s `paths` module uses.
fn home() -> PathBuf {
    std::env::var_os("HOME").map_or_else(|| PathBuf::from("/"), PathBuf::from)
}

/// The `ADI_DIR` env value, trimmed; empty or unset falls back to `.adi`.
fn adi_dir_name() -> String {
    std::env::var(ADI_DIR_ENV)
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| DEFAULT_ADI_DIR.to_string())
}

/// The canonical registry location: `$HOME/$ADI_DIR/mono/ports/registry.json`
/// (default `~/.adi/mono/ports/registry.json`). Re-derives the `$HOME/$ADI_DIR/mono`
/// path locally rather than depending on `adi-core`, so this crate stays a
/// dependency-free leaf that `adi-core` itself can consume — exactly like `adi-hive`.
#[must_use]
pub fn default_registry_path() -> PathBuf {
    home()
        .join(adi_dir_name())
        .join(MONO_SUBDIR)
        .join(PORTS_SUBDIR)
        .join(REGISTRY_FILE)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_range_avoids_the_reserved_bands() {
        let cfg = Config::default();
        // Nothing in the allocatable range should be reserved by default.
        assert!(!cfg.is_reserved(*cfg.range.start()));
        assert!(!cfg.is_reserved(*cfg.range.end()));
        assert!(!cfg.is_reserved(8080));
    }

    #[test]
    fn reserved_bands_cover_privileged_and_adi_daemon_ports() {
        let cfg = Config::default();
        assert!(cfg.is_reserved(22), "ssh is privileged");
        assert!(cfg.is_reserved(80), "http is privileged");
        assert!(
            cfg.is_reserved(15353),
            "ADI DNS must never be collided with"
        );
        assert!(cfg.is_reserved(15000));
        assert!(cfg.is_reserved(15999));
        assert!(!cfg.is_reserved(16000));
    }

    #[test]
    fn default_registry_path_lives_under_the_mono_ports_namespace() {
        let p = default_registry_path();
        assert!(
            p.ends_with("mono/ports/registry.json"),
            "got {}",
            p.display()
        );
    }
}
