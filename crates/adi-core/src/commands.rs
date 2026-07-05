//! The command surface the GUI triggers: `Adi` is the top-level facade (`adi.enable()`,
//! `adi.disable()`, `adi.status()`), and `Adi::dns()` returns the `Dns` subsystem
//! (`adi.dns.enable()`, `adi.dns.disable()`, …). The `adi-mono` CLI is a thin argv
//! adapter over this.

use serde::Serialize;

use crate::dns::Dns;
use crate::service::{Service, ServiceReport};

/// Aggregate live state across every managed service — the JSON the GUI polls.
#[derive(Debug, Serialize)]
pub struct Report {
    pub any_running: bool,
    pub services: Vec<ServiceReport>,
}

/// The adi platform command surface. Zero-sized; groups the platform-wide commands
/// and hands out subsystem facades like [`Dns`].
#[derive(Debug, Default, Clone, Copy)]
pub struct Adi;

// `Adi` is a zero-sized facade; some methods take `self` only for `adi.dns()`-style
// call-site ergonomics rather than because they read state.
#[allow(clippy::unused_self)]
impl Adi {
    #[must_use]
    pub fn new() -> Self {
        Self
    }

    /// The DNS subsystem facade — `Adi::new().dns().enable()` mirrors `adi.dns.enable()`.
    #[must_use]
    pub fn dns(self) -> Dns {
        Dns::new()
    }

    /// Every managed service, in display order. New services slot in here.
    fn services(self) -> Vec<Box<dyn Service>> {
        vec![Box::new(Dns::new())]
    }

    /// Enable every service (`adi.enable()`).
    pub fn enable(self) {
        for svc in self.services() {
            svc.enable();
        }
    }

    /// Disable every service (`adi.disable()`).
    pub fn disable(self) {
        for svc in self.services() {
            svc.disable();
        }
    }

    /// Live state across all services (`adi.status()`).
    #[must_use]
    pub fn report(self) -> Report {
        let services: Vec<ServiceReport> = self.services().iter().map(|s| s.report()).collect();
        Report {
            any_running: services.iter().any(|s| s.running),
            services,
        }
    }
}
