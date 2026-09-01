//! The dashboards supervisor as an ADI service.
//!
//! A dashboard is a pair of bun servers, and something has to run them. On a developer machine
//! that something has always existed by hand — a per-user agent pointed at
//! `~/.adi/mono/dashboards/hive.yaml`. It was never part of what `adi-mono up` installs, which
//! nobody noticed while the only machines running dashboards were set up by hand.
//!
//! On a node it is fatal, and silently so: creating a dashboard scaffolds its files and derives
//! its host, `GET /api/dashboards` lists it, and **nothing ever starts it**. The node looks
//! healthy and serves a dead hostname. Since reaching a remote dashboard is the whole point of
//! the fleet (`docs/fleet.md` §1, §4), the supervisor has to be something the platform installs,
//! on every OS, like any other service.
//!
//! It is deliberately a *second* hive, not the front door:
//!
//! - It runs **unprivileged**, so it may launch user code. A root `adi-hive` launches *nothing* —
//!   not an imported runner and not one of its own — which is exactly why the root front door can
//!   only route.
//! - It owns **no routes**. Routing dashboards is the front door's job, through the same
//!   imports. This instance exists to run processes, and binds one throwaway loopback port only
//!   because adi-hive requires an address to start.

use std::net::{Ipv4Addr, SocketAddr, TcpStream};
use std::path::PathBuf;
use std::time::Duration;

use adi_config::Flavor;

use crate::dns::sibling_binary;
use crate::paths;
use crate::service::Service;
use crate::status::DaemonStatus;

pub(crate) fn label() -> String {
    Flavor::current().label("dashboards")
}

/// The store module this supervisor lives in, and the config it runs.
const MODULE: &str = "dashboards";
const CONFIG_FILE: &str = "hive.yaml";

/// The supervisor's own bind address. It serves nothing — adi-hive just refuses to start with
/// no bindable address — so the port is deliberately outside the ports manager's `8000..=9999`
/// allocation range, where no lease can ever land on it, and nowhere near the `adi daemon` band.
///
/// Per flavour, and not for decoration: this is the one port in the stack that is not leased
/// from the store, so two installs sharing it is the one collision a separate store does not
/// already prevent. Sharing it means the second install's supervisor never starts, and — worse
/// — the first one's answers its probe, so it reports itself healthy while running nothing.
fn bind_port() -> u16 {
    Flavor::current().supervisor_port
}

fn bind() -> String {
    format!("127.0.0.1:{}", bind_port())
}

/// How long the running-probe waits for a TCP connect before deciding it is down.
const PROBE_TIMEOUT: Duration = Duration::from_millis(300);

/// The config this supervisor runs, written on enable when absent.
///
/// `$ADI_DASHBOARDS_DIR` / `$ADI_PROJECTS_DIR` are expanded by adi-hive's import loader, so the
/// file is store-relative and needs no rewriting when `$ADI_DIR` moves.
fn supervisor_config() -> String {
    format!(
        r#"# The per-user service supervisor — the ONE hive that RUNS things.
#
# Written by `adi-mono up`; edit freely, it is not regenerated once it exists.
#
# Runs UNPRIVILEGED, which is the point: a root adi-hive launches nothing at all — not an imported
# runner, not one of its own — so this instance, unlike the root front door, is what actually
# launches every dashboard's bun servers and every project's services. The front door only ROUTES.
#
# It owns no routes of its own. adi-hive still requires one bindable address to start, so it gets
# a harmless loopback port, deliberately outside the ports manager's 8000-9999 range so a future
# lease can never collide with it.
#
# Adding a dashboard needs no edit here: dropping `<id>/.adi/hive.yaml` into this directory is
# enough — the imports are re-read every few seconds.

version: "1"

proxy:
  bind:
    - "{bind}"

# A `*` is one directory level: a config has a fixed home (`<id>/.adi/hive.yaml`, or the id's own
# dir), so these lines name it rather than searching for it. The search form (`**`) costs a walk of
# everything a dashboard keeps inside itself, and it ran on every reload tick.
imports:
  - $ADI_DASHBOARDS_DIR/*/.adi/hive.yaml
  - $ADI_DASHBOARDS_DIR/*/hive.yaml
  - $ADI_PROJECTS_DIR/*/.adi/hive.yaml
  - $ADI_PROJECTS_DIR/*/hive.yaml
"#,
        bind = bind()
    )
}

/// The config path within the store.
fn config_path() -> PathBuf {
    adi_config::Config::open()
        .module(MODULE)
        .raw_path(CONFIG_FILE)
}

/// The bundled `adi-hive`, resolved as a sibling of the running executable.
fn binary_path() -> String {
    sibling_binary("adi-hive", "ADI_HIVE_BIN")
}

/// The dashboards supervisor service (`adi-mono dashboards …`).
#[derive(Debug, Default, Clone, Copy)]
pub struct Dashboards;

#[allow(clippy::unused_self)]
impl Dashboards {
    #[must_use]
    pub fn new() -> Self {
        Self
    }

    /// The supervisor config, materialized when it does not exist yet.
    ///
    /// Never overwrites: the file is meant to be edited (an operator may add imports or move the
    /// bind), and `up` is run repeatedly. Writing it unconditionally would silently discard that.
    fn ensure_config(self) {
        let path = config_path();
        if path.exists() {
            return;
        }
        if let Some(dir) = path.parent()
            && let Err(e) = std::fs::create_dir_all(dir)
        {
            eprintln!("adi: could not create {}: {e}", dir.display());
            return;
        }
        if let Err(e) = std::fs::write(&path, supervisor_config()) {
            eprintln!("adi: could not write {}: {e}", path.display());
        }
    }
}

impl Service for Dashboards {
    fn id(&self) -> &'static str {
        "dashboards"
    }
    fn name(&self) -> &'static str {
        "Dashboards"
    }
    fn label(&self) -> String {
        label()
    }

    /// adi-hive writes its own status file beside the config it was given.
    fn status_path(&self) -> PathBuf {
        adi_config::Config::open()
            .module(MODULE)
            .raw_path("status.json")
    }

    fn log_path(&self) -> PathBuf {
        paths::logs_dir().join("adi-dashboards.log")
    }

    /// The config has to exist before the runner starts, or adi-hive falls back to its built-in
    /// defaults and supervises nothing.
    fn program(&self) -> Vec<String> {
        self.ensure_config();
        vec![binary_path(), config_path().to_string_lossy().into_owned()]
    }

    fn on_enable(&self) {
        self.ensure_config();
    }

    /// Up == the supervisor's own loopback port answers. Its status file is written by adi-hive,
    /// but the port is the cheaper and more direct question.
    fn is_running(&self) -> bool {
        TcpStream::connect_timeout(
            &SocketAddr::from((Ipv4Addr::LOCALHOST, bind_port())),
            PROBE_TIMEOUT,
        )
        .is_ok()
    }

    /// Name what it is supervising, not the address it binds — the address is an implementation
    /// detail nobody connects to, while the dashboard count is the thing an operator wants.
    fn detail(&self, _status: Option<&DaemonStatus>) -> String {
        let bind = bind();
        let dashboards = std::fs::read_dir(adi_config::Config::open().module(MODULE).dir())
            .map(|entries| {
                entries
                    .flatten()
                    .filter(|e| e.path().join(".adi").join("hive.yaml").is_file())
                    .count()
            })
            .unwrap_or(0);
        match dashboards {
            0 => format!("Running · {bind} · no dashboards yet"),
            1 => format!("Running · {bind} · 1 dashboard"),
            n => format!("Running · {bind} · {n} dashboards"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The supervisor must keep the two properties the front door relies on: it runs user code
    /// (so it must not be the root instance) and it claims no hostnames (so it can never shadow
    /// a front-door route).
    #[test]
    fn the_supervisor_config_runs_dashboards_and_routes_nothing() {
        let hive: serde_yaml_ng::Value =
            serde_yaml_ng::from_str(&supervisor_config()).expect("the shipped config parses");

        let imports = hive["imports"].as_sequence().expect("imports");
        let imports: Vec<&str> = imports
            .iter()
            .filter_map(serde_yaml_ng::Value::as_str)
            .collect();
        assert!(
            imports.iter().any(|i| i.contains("ADI_DASHBOARDS_DIR")),
            "dashboards must be imported or nothing runs them: {imports:?}"
        );
        assert!(
            imports.iter().any(|i| i.contains("ADI_PROJECTS_DIR")),
            "project services are supervised here too: {imports:?}"
        );

        assert!(
            hive.get("services").is_none(),
            "this instance declares no services of its own — it only imports"
        );

        let binds = hive["proxy"]["bind"].as_sequence().expect("a bind address");
        assert_eq!(binds.len(), 1, "one throwaway address, not a front door");
        let bind = binds[0].as_str().expect("bind is a string");
        assert_eq!(bind, super::bind(), "the config and the probe must agree");
        let port: u16 = bind
            .rsplit(':')
            .next()
            .expect("port")
            .parse()
            .expect("numeric");
        assert!(
            !(8000..=9999).contains(&port),
            "must sit outside the ports manager's allocation range, or a lease could take it"
        );
    }
}
