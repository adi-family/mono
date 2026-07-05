//! The `Service` trait every managed service conforms to, plus the report types the
//! GUI renders. A service describes *what* to run and *where* its files live; the
//! default trait methods drive the launchd lifecycle uniformly (mirroring Swift's
//! `ManagedService` + `Launchd`), so adding a service is data, not new control flow.

use std::path::PathBuf;

use serde::Serialize;

use crate::launchd;
use crate::status::{self, DaemonStatus};

/// One selectable action for a service, rendered as a button by the GUI. `args` is
/// the argv to invoke on the `adi-mono` CLI to perform it — so the GUI stays generic
/// and every action round-trips back through the same command surface.
#[derive(Debug, Clone, Serialize)]
pub struct Action {
    pub id: String,
    pub title: String,
    pub args: Vec<String>,
}

/// A service's live state plus its available actions — one row in the GUI.
#[derive(Debug, Clone, Serialize)]
pub struct ServiceReport {
    pub id: String,
    pub name: String,
    pub enabled: bool,
    pub running: bool,
    pub detail: String,
    pub actions: Vec<Action>,
}

pub trait Service {
    /// Short, stable id and CLI namespace (e.g. `dns` → `adi-mono dns …`).
    fn id(&self) -> &'static str;
    fn name(&self) -> &'static str;
    /// launchd label, namespaced `family.adi.app.*`.
    fn label(&self) -> String;
    fn status_path(&self) -> PathBuf;
    fn log_path(&self) -> PathBuf;

    /// Full argv (binary + args). May write a config file as a side effect.
    fn program(&self) -> Vec<String>;

    fn env(&self) -> Vec<(String, String)> {
        vec![("RUST_LOG".to_string(), "info".to_string())]
    }

    fn on_enable(&self) {}
    fn on_disable(&self) {}

    /// Human status line shown when the service is running.
    fn detail(&self, status: Option<&DaemonStatus>) -> String {
        status.map_or_else(String::new, |s| format!("Running · {}", s.bound_addr))
    }

    /// Service-specific actions beyond the universal enable/disable toggle.
    fn extra_actions(&self) -> Vec<Action> {
        Vec::new()
    }

    // MARK: lifecycle — uniform across services; only the data above differs.

    fn enable(&self) {
        let program = self.program();
        launchd::enable(
            &self.label(),
            &program,
            &self.log_path().to_string_lossy(),
            &self.env(),
        );
        self.on_enable();
    }

    fn disable(&self) {
        launchd::disable(&self.label());
        self.on_disable();
    }

    /// Build this service's live report: loaded state, running PID, status line, and
    /// the toggle action followed by any service-specific actions.
    fn report(&self) -> ServiceReport {
        let enabled = launchd::is_loaded(&self.label());
        let status = status::read(&self.status_path());
        let running = status
            .as_ref()
            .is_some_and(|s| status::process_alive(s.pid));

        let detail = if running {
            self.detail(status.as_ref())
        } else if enabled {
            "Enabled · starting…".to_string()
        } else {
            "Stopped".to_string()
        };

        let toggle = Action {
            id: "toggle".to_string(),
            title: format!(
                "{} {}",
                if enabled { "Disable" } else { "Enable" },
                self.name()
            ),
            args: vec![
                self.id().to_string(),
                if enabled { "disable" } else { "enable" }.to_string(),
            ],
        };
        let mut actions = vec![toggle];
        actions.extend(self.extra_actions());

        ServiceReport {
            id: self.id().to_string(),
            name: self.name().to_string(),
            enabled,
            running,
            detail,
            actions,
        }
    }
}
