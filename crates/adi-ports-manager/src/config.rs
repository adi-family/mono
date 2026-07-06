//! How the allocator is parameterized: the range it hands ports out of, the ranges
//! it must never touch, and where static leases persist.

use std::ops::RangeInclusive;
use std::path::PathBuf;

const ADI_DIR_ENV: &str = "ADI_DIR";
const DEFAULT_ADI_DIR: &str = ".adi";
const MONO_SUBDIR: &str = "mono";
const PORTS_SUBDIR: &str = "ports";
const REGISTRY_FILE: &str = "registry.json";

/// Default allocatable range — the 8000s, where the mono dev services already sit.
const DEFAULT_RANGE: RangeInclusive<u16> = 8000..=9999;

/// Privileged ports: binding these needs root, so they are never handed out.
const PRIVILEGED_BAND: RangeInclusive<u16> = 0..=1023;

/// The `adi daemon` supervisor band around the protected ADI DNS on `127.0.0.1:15353`.
const ADI_DAEMON_BAND: RangeInclusive<u16> = 15000..=15999;

/// Parameters controlling allocation.
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
    /// The standard configuration — identical to [`Config::default`].
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// True if `port` falls in any reserved band.
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

/// The canonical registry location: `$HOME/$ADI_DIR/mono/ports/registry.json`.
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
