//! The persisted ledger of static (reserved) leases.
//!
//! Only *static* allocations land here — a `(service, key)` pair that must resolve to
//! the same port across restarts. Dynamic allocations are computed on the fly and never
//! recorded (see [`crate::Ports::allocate_dynamic`]). The on-disk form is a small,
//! human-readable JSON array of leases; lookups are linear because the set is tiny.

use std::collections::HashSet;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

/// One durable reservation: the port promised to a service's named port slot (the same
/// keys `hive.yaml` uses under `rollout.recreate.ports`, e.g. `http`, `db`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Lease {
    /// The service that owns the port (e.g. `frontend`).
    pub service: String,
    /// The named port slot within that service (e.g. `http`).
    pub key: String,
    /// The reserved port.
    pub port: u16,
}

/// The full set of static leases, as loaded from (and saved to) the registry file.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Registry {
    #[serde(default)]
    leases: Vec<Lease>,
}

impl Registry {
    /// Load the registry from `path`. A missing file is an empty registry, not an error.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Corrupt`] if the file exists but is not valid registry JSON, or
    /// [`Error::Io`] on any other read failure.
    pub fn load(path: &Path) -> Result<Self> {
        let raw = match std::fs::read_to_string(path) {
            Ok(raw) => raw,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Self::default()),
            Err(e) => return Err(Error::Io(e)),
        };
        serde_json::from_str(&raw).map_err(|source| Error::Corrupt {
            path: path.to_path_buf(),
            source,
        })
    }

    /// Persist the registry to `path`, creating parent directories as needed. The write
    /// is atomic: the JSON is written to a sibling temp file and renamed into place, so a
    /// reader never sees a half-written registry.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Io`] if serialization or any filesystem step fails.
    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let json = serde_json::to_vec_pretty(self).map_err(std::io::Error::other)?;

        // Per-pid temp name: writers are already serialized by the registry lock, but a
        // distinct name keeps the rename atomic and collision-free regardless.
        let file_name = path.file_name().map_or_else(
            || "registry.json".to_string(),
            |n| n.to_string_lossy().into_owned(),
        );
        let tmp = path.with_file_name(format!("{file_name}.{}.tmp", std::process::id()));

        std::fs::write(&tmp, &json)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            // World-readable so a per-user GUI can inspect a root daemon's registry.
            let _ = std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o644));
        }
        std::fs::rename(&tmp, path)?;
        Ok(())
    }

    /// The port leased to `(service, key)`, if any.
    #[must_use]
    pub fn get(&self, service: &str, key: &str) -> Option<u16> {
        self.leases
            .iter()
            .find(|l| l.service == service && l.key == key)
            .map(|l| l.port)
    }

    /// Record `port` for `(service, key)`, replacing any existing lease for that pair.
    pub fn insert(&mut self, service: &str, key: &str, port: u16) {
        if let Some(existing) = self
            .leases
            .iter_mut()
            .find(|l| l.service == service && l.key == key)
        {
            existing.port = port;
        } else {
            self.leases.push(Lease {
                service: service.to_string(),
                key: key.to_string(),
                port,
            });
        }
    }

    /// Drop the lease for `(service, key)`, returning the port it held, if any.
    pub fn remove(&mut self, service: &str, key: &str) -> Option<u16> {
        let idx = self
            .leases
            .iter()
            .position(|l| l.service == service && l.key == key)?;
        Some(self.leases.remove(idx).port)
    }

    /// Every port currently reserved, as a set for fast collision checks.
    #[must_use]
    pub fn ports(&self) -> HashSet<u16> {
        self.leases.iter().map(|l| l.port).collect()
    }

    /// A snapshot of every lease.
    #[must_use]
    pub fn leases(&self) -> Vec<Lease> {
        self.leases.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn insert_get_and_remove_round_trip() {
        let mut reg = Registry::default();
        assert_eq!(reg.get("frontend", "http"), None);

        reg.insert("frontend", "http", 8010);
        reg.insert("backend", "http", 8009);
        assert_eq!(reg.get("frontend", "http"), Some(8010));
        assert_eq!(reg.get("backend", "http"), Some(8009));
        assert_eq!(reg.ports(), HashSet::from([8010, 8009]));

        // Re-inserting the same pair replaces rather than duplicates.
        reg.insert("frontend", "http", 8011);
        assert_eq!(reg.get("frontend", "http"), Some(8011));
        assert_eq!(reg.leases().len(), 2);

        assert_eq!(reg.remove("frontend", "http"), Some(8011));
        assert_eq!(reg.get("frontend", "http"), None);
        assert_eq!(reg.remove("frontend", "http"), None);
    }

    #[test]
    fn missing_file_loads_as_empty() {
        let path =
            std::env::temp_dir().join(format!("adi-ports-missing-{}.json", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let reg = Registry::load(&path).expect("missing file is empty, not an error");
        assert!(reg.leases().is_empty());
    }

    #[test]
    fn save_then_load_preserves_leases() {
        let path =
            std::env::temp_dir().join(format!("adi-ports-roundtrip-{}.json", std::process::id()));
        let _ = std::fs::remove_file(&path);

        let mut reg = Registry::default();
        reg.insert("frontend", "http", 8010);
        reg.save(&path).expect("save");

        let loaded = Registry::load(&path).expect("load");
        assert_eq!(loaded.get("frontend", "http"), Some(8010));

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn corrupt_file_is_reported() {
        let path =
            std::env::temp_dir().join(format!("adi-ports-corrupt-{}.json", std::process::id()));
        std::fs::write(&path, b"{ not json").expect("write garbage");
        let err = Registry::load(&path).expect_err("corrupt must error");
        assert!(matches!(err, Error::Corrupt { .. }));
        let _ = std::fs::remove_file(&path);
    }
}
