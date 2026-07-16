//! The command surface the GUI triggers: `Adi` is the top-level facade and `Adi::dns()`
//! returns the `Dns` subsystem. The `adi-mono` CLI is a thin argv adapter over this.

use serde::Serialize;

use crate::app::App;
use crate::dns::Dns;
use crate::service::{Service, ServiceReport};

/// Aggregate live state across every managed service — the JSON the GUI polls.
#[derive(Debug, Serialize)]
pub struct Report {
    pub any_running: bool,
    pub services: Vec<ServiceReport>,
}

/// The adi platform command surface — a zero-sized facade over the platform commands.
#[derive(Debug, Default, Clone, Copy)]
pub struct Adi;

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

    /// The projects registry backed by the standard store — `Adi::new().projects().list()`.
    #[must_use]
    pub fn projects(self) -> adi_projects::Projects {
        adi_projects::Projects::open()
    }

    /// The task tree backed by the standard store — `Adi::new().tasks().list(...)`.
    #[must_use]
    pub fn tasks(self) -> adi_tasks::Tasks {
        adi_tasks::Tasks::open()
    }

    /// The agent-definition registry backed by the standard store.
    #[must_use]
    pub fn agents(self) -> adi_agents::Agents {
        adi_agents::Agents::open()
    }

    /// The trigger-definition registry backed by the standard store.
    #[must_use]
    pub fn triggers(self) -> adi_triggers::Triggers {
        adi_triggers::Triggers::open()
    }

    /// Every managed service, in display + apply order. DNS is first so, when enabling, its
    /// `on_enable` migrates the front door (proxy-only) before the control-panel agent
    /// binds the shared port — otherwise the old runner-supervised adi-app would collide
    /// with the new agent on it.
    fn services(self) -> Vec<Box<dyn Service>> {
        vec![Box::new(Dns::new()), Box::new(App::new())]
    }

    /// Enable every service (`adi.enable()`).
    pub fn enable(self) {
        for svc in self.services() {
            svc.enable();
        }
    }

    /// Bring every service up **without restarting any that are already running**
    /// (`adi.up()`) — the launch-time bootstrap. On a fresh machine this installs and
    /// starts everything (one admin prompt for the privileged DNS route/front door); on a
    /// machine where the stack is already up it's a no-op that never interrupts the DNS.
    pub fn ensure_enabled(self) {
        for svc in self.services() {
            svc.ensure_enabled();
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
