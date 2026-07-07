//! The adi control panel (`adi-app`) as an ADI service: an **unprivileged** per-user
//! `LaunchAgent`, so the on/off toggle can start and stop it — and the in-process mesh it
//! runs — with no password. The root front door ([`crate::dns`]) stays always-on and just
//! proxies `app.adi` to this agent's port, which both sides take from the ports manager so
//! they always agree.

use std::net::{Ipv4Addr, SocketAddr, TcpStream};
use std::path::PathBuf;
use std::time::Duration;

use adi_ports_manager::Ports;

use crate::dns::sibling_binary;
use crate::paths;
use crate::service::Service;
use crate::status::DaemonStatus;

const LABEL: &str = "family.adi.app.control-panel";

/// The ports-manager lease the agent binds and the front door proxies to.
const PORT_SERVICE: &str = "app";
const PORT_KEY: &str = "http";

/// Fallback if the ports manager can't hand out a lease.
const DEFAULT_PORT: u16 = 8090;

/// How long the running-probe waits for a TCP connect before deciding it's down.
const PROBE_TIMEOUT: Duration = Duration::from_millis(300);

/// The control panel's stable port: reserve it (idempotent) so it exists, falling back to
/// [`DEFAULT_PORT`]. Shared with the front-door route via the same `(service, key)` lease.
#[must_use]
pub fn port() -> u16 {
    Ports::new()
        .reserve(PORT_SERVICE, PORT_KEY)
        .unwrap_or(DEFAULT_PORT)
}

/// The already-reserved port without allocating one (a read; `None` if never reserved).
fn reserved_port() -> Option<u16> {
    Ports::new().get(PORT_SERVICE, PORT_KEY).ok().flatten()
}

/// The bundled `adi-app`, resolved as a sibling of the running executable (overridable via `ADI_APP_BIN`).
fn binary_path() -> String {
    sibling_binary("adi-app", "ADI_APP_BIN")
}

/// The adi control-panel service (`adi.app.*`). Zero-sized; all state is in launchd + the
/// ports registry.
#[derive(Debug, Default, Clone, Copy)]
pub struct App;

#[allow(clippy::unused_self)]
impl App {
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

impl Service for App {
    fn id(&self) -> &'static str {
        "app"
    }
    fn name(&self) -> &'static str {
        "App"
    }
    fn label(&self) -> String {
        LABEL.to_string()
    }

    /// The control panel writes no status file — its liveness is probed by [`is_running`](Self::is_running).
    fn status_path(&self) -> PathBuf {
        adi_config::Config::open()
            .module("app")
            .raw_path("status.json")
    }

    fn log_path(&self) -> PathBuf {
        paths::logs_dir().join("adi-app.log")
    }

    /// Run `adi-app <port>` on the reserved port (its `argv[1]` sets the loopback bind).
    fn program(&self) -> Vec<String> {
        vec![binary_path(), port().to_string()]
    }

    /// Up == something is listening on the reserved control-panel port.
    fn is_running(&self) -> bool {
        let Some(p) = reserved_port() else {
            return false;
        };
        TcpStream::connect_timeout(&SocketAddr::from((Ipv4Addr::LOCALHOST, p)), PROBE_TIMEOUT)
            .is_ok()
    }

    /// The running line names the port the panel serves (and the mesh it hosts) on loopback.
    fn detail(&self, _status: Option<&DaemonStatus>) -> String {
        reserved_port().map_or_else(
            || "Running".to_string(),
            |p| format!("Running · 127.0.0.1:{p} · app.adi"),
        )
    }
}
