//! adi-ports-manager — allocate and track TCP ports for adi services: a pure library
//! (no CLI, no daemon) that hands out collision-free ports via static (persisted,
//! stable) or dynamic (ephemeral) allocation.

mod config;
mod error;
mod lock;
mod probe;
mod registry;
mod template;

use std::collections::HashSet;
use std::path::PathBuf;

pub use config::{Config, default_registry_path};
pub use error::{Error, Result};
pub use probe::is_bindable;
pub use registry::{Lease, Registry};
// Config-field port commands: the `bash`…`` preprocessor (`preprocess`/`Commands`/`with_commands`)
// plus serde `deserialize_with` helpers (`port`/`ports_map`) that run `datacommand:<hash>`
// placeholders on read. See the `template` module.
pub use template::{Commands, Port, port, ports_map, preprocess, with_commands, with_ports};

use lock::FileLock;

/// The allocator facade. Cheap to clone; all persistent state lives in the registry file.
#[derive(Debug, Clone)]
pub struct Ports {
    config: Config,
}

impl Default for Ports {
    fn default() -> Self {
        Self::new()
    }
}

impl Ports {
    /// A manager with the [standard configuration](Config::default).
    #[must_use]
    pub fn new() -> Self {
        Self {
            config: Config::default(),
        }
    }

    /// A manager with a caller-supplied [`Config`].
    #[must_use]
    pub fn with_config(config: Config) -> Self {
        Self { config }
    }

    /// The configuration this manager was built with.
    #[must_use]
    pub fn config(&self) -> &Config {
        &self.config
    }

    /// Reserve a **static** port for `(service, key)`, persisting it. Idempotent.
    ///
    /// # Errors
    /// Fails if the lock times out, the range is exhausted, or the registry I/O fails.
    pub fn reserve(&self, service: &str, key: &str) -> Result<u16> {
        let _lock = FileLock::acquire(&self.lock_path())?;
        let mut registry = Registry::load(&self.config.registry_path)?;
        if let Some(port) = registry.get(service, key) {
            return Ok(port);
        }
        let taken = registry.ports();
        let port = self.find_free(&taken)?;
        registry.insert(service, key, port);
        registry.save(&self.config.registry_path)?;
        Ok(port)
    }

    /// Release the static lease for `(service, key)`, returning the port it held.
    ///
    /// # Errors
    /// Fails if the registry lock or file I/O fails.
    pub fn release(&self, service: &str, key: &str) -> Result<Option<u16>> {
        let _lock = FileLock::acquire(&self.lock_path())?;
        let mut registry = Registry::load(&self.config.registry_path)?;
        let freed = registry.remove(service, key);
        if freed.is_some() {
            registry.save(&self.config.registry_path)?;
        }
        Ok(freed)
    }

    /// The static port leased to `(service, key)`, if any.
    ///
    /// # Errors
    /// Fails if the registry cannot be read.
    pub fn get(&self, service: &str, key: &str) -> Result<Option<u16>> {
        let registry = Registry::load(&self.config.registry_path)?;
        Ok(registry.get(service, key))
    }

    /// A snapshot of every static lease currently recorded.
    ///
    /// # Errors
    /// Fails if the registry cannot be read.
    pub fn leases(&self) -> Result<Vec<Lease>> {
        Ok(Registry::load(&self.config.registry_path)?.leases())
    }

    /// Allocate a **dynamic** port: the first free port in the range, *not* persisted.
    ///
    /// # Errors
    /// Fails if the range is exhausted or the registry cannot be read.
    pub fn allocate_dynamic(&self) -> Result<u16> {
        // Read-only: honor static leases without taking the write lock.
        let taken = Registry::load(&self.config.registry_path)?.ports();
        self.find_free(&taken)
    }

    /// True if `port` is allocatable and free right now (in range, not reserved, bindable).
    #[must_use]
    pub fn is_available(&self, port: u16) -> bool {
        self.config.range.contains(&port)
            && !self.config.is_reserved(port)
            && probe::is_bindable(port)
    }

    /// True if `port` falls in a reserved band.
    #[must_use]
    pub fn is_reserved(&self, port: u16) -> bool {
        self.config.is_reserved(port)
    }

    /// Scan the range for the first port that is not reserved, not in `taken`, and bindable.
    fn find_free(&self, taken: &HashSet<u16>) -> Result<u16> {
        for port in self.config.range.clone() {
            if self.config.is_reserved(port) || taken.contains(&port) {
                continue;
            }
            if probe::is_bindable(port) {
                return Ok(port);
            }
        }
        Err(Error::Exhausted {
            range: self.config.range.clone(),
        })
    }

    /// The lock file sitting beside the registry (`registry.json.lock`).
    fn lock_path(&self) -> PathBuf {
        let file_name = self.config.registry_path.file_name().map_or_else(
            || "registry.json".to_string(),
            |n| n.to_string_lossy().into_owned(),
        );
        self.config
            .registry_path
            .with_file_name(format!("{file_name}.lock"))
    }
}

#[cfg(test)]
mod tests {
    use std::net::{Ipv4Addr, SocketAddr, TcpListener};
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU32, Ordering};

    use super::*;

    /// A unique temp registry path per call, so tests never share state.
    fn temp_registry() -> PathBuf {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!(
            "adi-ports-mgr-{}-{n}/registry.json",
            std::process::id()
        ))
    }

    fn ports_with(range: std::ops::RangeInclusive<u16>) -> (Ports, PathBuf) {
        let path = temp_registry();
        let cfg = Config {
            range,
            reserved: vec![],
            registry_path: path.clone(),
        };
        (Ports::with_config(cfg), path)
    }

    fn cleanup(path: &Path) {
        if let Some(parent) = path.parent() {
            let _ = std::fs::remove_dir_all(parent);
        }
    }

    #[test]
    fn reserve_is_idempotent_and_persists_across_instances() {
        let (ports, path) = ports_with(8000..=9000);
        let first = ports.reserve("frontend", "http").expect("reserve");
        let again = ports.reserve("frontend", "http").expect("re-reserve");
        assert_eq!(first, again, "same pair must return the same port");

        let reopened = Ports::with_config(ports.config().clone());
        assert_eq!(reopened.get("frontend", "http").expect("get"), Some(first));
        cleanup(&path);
    }

    #[test]
    fn distinct_pairs_get_distinct_ports() {
        let (ports, path) = ports_with(8000..=9000);
        let a = ports.reserve("frontend", "http").expect("a");
        let b = ports.reserve("frontend", "db").expect("b");
        let c = ports.reserve("backend", "http").expect("c");
        assert_ne!(a, b);
        assert_ne!(b, c);
        assert_ne!(a, c);
        cleanup(&path);
    }

    #[test]
    fn release_frees_the_lease() {
        let (ports, path) = ports_with(8000..=9000);
        let port = ports.reserve("frontend", "http").expect("reserve");
        assert_eq!(
            ports.release("frontend", "http").expect("release"),
            Some(port)
        );
        assert_eq!(ports.get("frontend", "http").expect("get"), None);
        assert_eq!(
            ports.release("frontend", "http").expect("release again"),
            None
        );
        cleanup(&path);
    }

    #[test]
    fn allocation_skips_a_port_that_is_live_on_the_machine() {
        // Hold an OS-chosen port, then constrain the range to exactly it: the only
        // candidate is bound, so allocation must report the range exhausted.
        let held =
            TcpListener::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, 0))).expect("bind ephemeral");
        let busy = held.local_addr().expect("addr").port();

        let (ports, path) = ports_with(busy..=busy);
        let err = ports.allocate_dynamic().expect_err("only port is busy");
        assert!(matches!(err, Error::Exhausted { .. }));

        let err = ports.reserve("svc", "http").expect_err("only port is busy");
        assert!(matches!(err, Error::Exhausted { .. }));
        cleanup(&path);
    }

    #[test]
    fn dynamic_allocation_avoids_a_static_lease() {
        // Range of two ports; reserve one statically, then dynamic must pick the other.
        let (ports, path) = ports_with(8100..=8101);
        let reserved = ports.reserve("svc", "http").expect("reserve");
        let dynamic = ports.allocate_dynamic().expect("dynamic");
        assert_ne!(dynamic, reserved, "dynamic must not stomp a static lease");
        cleanup(&path);
    }

    #[test]
    fn is_available_and_is_reserved_reflect_config() {
        let path = temp_registry();
        let cfg = Config {
            range: 8000..=9000,
            reserved: vec![8500..=8600],
            registry_path: path.clone(),
        };
        let ports = Ports::with_config(cfg);
        assert!(ports.is_reserved(8550));
        assert!(!ports.is_available(8550), "reserved port is not available");
        assert!(
            !ports.is_available(100),
            "out-of-range port is not available"
        );
        cleanup(&path);
    }
}
