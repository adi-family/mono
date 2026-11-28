//! The adi control panel (`adi-app`) as an ADI service: an **unprivileged** per-user
//! `LaunchAgent`, so the on/off toggle can start and stop it — and the in-process mesh it
//! runs — with no password. The root front door ([`crate::dns`]) stays always-on and just
//! proxies `app.adi` to this agent's port, which both sides take from the ports manager so
//! they always agree.

use std::net::{Ipv4Addr, SocketAddr, TcpStream};
use std::path::PathBuf;
use std::time::Duration;

use adi_config::Flavor;
use adi_ports_manager::Ports;

use crate::dns::sibling_binary;
use crate::paths;
use crate::service::Service;
use crate::status::DaemonStatus;

pub(crate) fn label() -> String {
    Flavor::current().label("control-panel")
}

/// The ports-manager lease the agent binds and the front door proxies to.
const PORT_SERVICE: &str = "app";
const PORT_KEY: &str = "http";

/// Fallback if the ports manager can't hand out a lease.
const DEFAULT_PORT: u16 = 8090;

/// How long the running-probe waits for a TCP connect before deciding it's down.
const PROBE_TIMEOUT: Duration = Duration::from_millis(300);

/// How long the oldest undelivered event may wait before event delivery is called stopped. The
/// dispatcher polls once a second and drains two hundred a tick, so two minutes is not a backlog
/// being worked through.
const DISPATCH_STALLED_AFTER_SECS: u64 = 120;

/// The two loops inside the panel that everything waiting depends on, read from what they leave
/// on disk: the await scheduler's heartbeat and the event dispatcher's cursor.
///
/// A listening port says the process exists. It said so for eleven hours on 2026-11-24 while
/// neither loop did: nothing woke, no event was delivered, and this report was green.
struct Loops {
    scheduler: adi_agents::awaits::Scheduler,
    dispatch: Option<adi_events::Lag>,
    now: u64,
}

impl Loops {
    fn read() -> Self {
        let config = adi_config::Config::open();
        Self {
            scheduler: adi_agents::awaits::Awaits::with_config(config.clone()).scheduler(),
            dispatch: adi_events::Events::with_config(config)
                .lag(adi_triggers::SUBSCRIBER)
                .ok(),
            now: adi_config::now_unix(),
        }
    }

    /// The liveness number for a healthy panel: "awaits ticked 1s ago".
    fn note(&self) -> Option<String> {
        let age = self.scheduler.age(self.now)?;
        (!self.scheduler.stalled(self.now)).then(|| {
            format!(
                "awaits ticked {} ago",
                adi_agents::awaits::duration_words(age)
            )
        })
    }

    fn problems(&self) -> Vec<String> {
        let mut problems = Vec::new();
        if self.scheduler.stalled(self.now) {
            problems.push(self.scheduler.describe(self.now));
        }
        if let Some(lag) = self.dispatch
            && let Some(waiting) = lag.waiting_secs(self.now)
            && waiting > DISPATCH_STALLED_AFTER_SECS
        {
            problems.push(format!(
                "event delivery has stopped: {} undelivered, the oldest for {} — no event \
                 trigger and no await on an event fires until adi-app is restarted",
                lag.behind,
                adi_agents::awaits::duration_words(waiting)
            ));
        }
        problems
    }
}

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
        label()
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

    /// Lead with the URL an operator can actually open, not the socket behind it: `app.adi`
    /// once `.adi` is routed on this machine, the loopback form as the fallback when it is not
    /// (a plain node, say, where `.adi` was never installed — `apps/linux/README.md`).
    fn detail(&self, _status: Option<&DaemonStatus>) -> String {
        let reach = match reserved_port() {
            None => "Running".to_string(),
            Some(p) if crate::dns::Dns::new().route_installed() => {
                format!("Running · http://app.adi (127.0.0.1:{p})")
            }
            Some(p) => format!("Running · http://127.0.0.1:{p} (.adi not routed)"),
        };
        match Loops::read().note() {
            Some(note) => format!("{reach} · {note}"),
            None => reach,
        }
    }

    fn problems(&self) -> Vec<String> {
        Loops::read().problems()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use adi_agents::awaits::Scheduler;
    use adi_events::Lag;

    const NOW: u64 = 1_795_580_000;

    fn loops(last_tick_ago: Option<u64>, oldest_unread_ago: Option<u64>) -> Loops {
        Loops {
            scheduler: Scheduler {
                last_tick_at: last_tick_ago.map(|ago| NOW - ago),
                pid: Some(1_108_161),
            },
            dispatch: Some(Lag {
                cursor: 17_670,
                behind: if oldest_unread_ago.is_some() { 212 } else { 0 },
                oldest_unread_at: oldest_unread_ago.map(|ago| NOW - ago),
            }),
            now: NOW,
        }
    }

    #[test]
    fn a_ticking_panel_reports_its_liveness_number_and_no_problem() {
        let healthy = loops(Some(1), None);
        assert_eq!(healthy.note().as_deref(), Some("awaits ticked 1s ago"));
        assert!(healthy.problems().is_empty());

        // A burst being drained is behind, not stopped.
        assert!(loops(Some(1), Some(5)).problems().is_empty());
    }

    /// hetzner-nosh on the morning of 2026-11-25, as this report would have shown it.
    #[test]
    fn a_panel_whose_loops_died_is_degraded_and_says_which_and_for_how_long() {
        let dead = loops(Some(11 * 3600 + 40 * 60), Some(11 * 3600 + 38 * 60));
        assert_eq!(dead.note(), None, "no healthy number to show");
        let problems = dead.problems();
        assert_eq!(problems.len(), 2, "{problems:?}");
        assert!(
            problems[0].contains("last ticked 11h 40m ago"),
            "{problems:?}"
        );
        assert!(problems[0].contains("pid 1108161"), "{problems:?}");
        assert!(problems[1].contains("212 undelivered"), "{problems:?}");
        assert!(problems[1].contains("11h 38m"), "{problems:?}");
    }

    /// A store no heartbeat was ever written to is unknown, not dead: an app from before
    /// heartbeats must not be reported stalled by a newer CLI.
    #[test]
    fn no_heartbeat_at_all_is_neither_a_number_nor_a_problem() {
        let unknown = loops(None, None);
        assert_eq!(unknown.note(), None);
        assert!(unknown.problems().is_empty());
    }
}
