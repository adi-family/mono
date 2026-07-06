//! How the allocator is parameterized: the range it hands ports out of, the ranges
//! it must never touch, and where static leases persist.

use std::ops::RangeInclusive;
use std::path::PathBuf;

/// The store module ports state lives under, and the raw registry file within it.
const PORTS_MODULE: &str = "ports";
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

/// The canonical registry location: `$HOME/$ADI_DIR/mono/ports/registry.json`.
/// The path comes from the shared [`adi_config`] store; this crate owns the file's
/// JSON format and persists it as a raw file within the `ports` module.
#[must_use]
pub fn default_registry_path() -> PathBuf {
    adi_config::Config::open()
        .module(PORTS_MODULE)
        .raw_path(REGISTRY_FILE)
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
