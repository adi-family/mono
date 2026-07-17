//! The `adi-dns` resolver as an ADI service, split by privilege so the on/off toggle
//! never needs a password: an unprivileged per-user resolver `LaunchAgent`, plus a root
//! front-door `LaunchDaemon` (`adi-hive` on `127.0.0.53:80`) installed once.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::launchd;
use crate::paths;
use crate::proc;
use crate::service::{Action, Service};
use crate::status::DaemonStatus;

const DOMAIN: &str = "adi";
const PORT: u16 = 10053;
pub(crate) const LABEL: &str = "family.adi.app.dns";

/// Kept off `127.0.0.1` so `:80` never collides with anything else serving there.
const FRONTDOOR_ADDR: &str = "127.0.0.53";
const FRONTDOOR_PORT: u16 = 80;
const FRONTDOOR_LABEL: &str = "family.adi.app.dns-landing";
const FRONTDOOR_DAEMON_PLIST: &str = "/Library/LaunchDaemons/family.adi.app.dns-landing.plist";
const FRONTDOOR_LOG: &str = "/Library/Logs/adi-hive-frontdoor.log";

// MARK: file locations (free helpers — all state is on disk / in launchd)

fn service_dir() -> PathBuf {
    adi_config::Config::open().module("dns").dir().to_path_buf()
}
fn config_path() -> PathBuf {
    service_dir().join("adi-dns.toml")
}
fn status_file() -> PathBuf {
    // A resolver-specific name: the front-door adi-hive writes its OWN `status.json` in this
    // same dir (it sits beside `hive-frontdoor.yaml`), so sharing the name makes the two
    // clobber each other — the GUI then misreads the proxy's status as the resolver's, its
    // shape doesn't match, and the service shows a stuck "starting…". Keep them separate.
    service_dir().join("resolver.json")
}
fn stage_path() -> PathBuf {
    service_dir().join(format!("resolver-{DOMAIN}"))
}
fn resolver_file() -> PathBuf {
    PathBuf::from(format!("/etc/resolver/{DOMAIN}"))
}
fn frontdoor_config_path() -> PathBuf {
    service_dir().join("hive-frontdoor.yaml")
}
fn frontdoor_plist_stage() -> PathBuf {
    service_dir().join(format!("{FRONTDOOR_LABEL}.plist"))
}

// MARK: front-door settings — the .adi hosts the front door proxies to the control panel

/// Simple, user-editable settings for the always-on front door: the `.adi` hosts proxied to
/// the control panel (`adi-app`). Every host is an alternative name for the *same* adi-app
/// process — they all share its single ports-manager-allocated port — so e.g. `api.adi` reaches
/// the very `/api` that `app.adi` serves. Lives at `~/.adi/mono/dns/frontdoor.toml`; edit
/// `hosts` to add or rename entries.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct FrontdoorSettings {
    hosts: Vec<String>,
}

impl Default for FrontdoorSettings {
    fn default() -> Self {
        Self {
            hosts: vec![format!("app.{DOMAIN}"), format!("api.{DOMAIN}")],
        }
    }
}

/// The typed `frontdoor.toml` settings file within the `dns` module.
fn frontdoor_settings() -> adi_config::ConfigFile<FrontdoorSettings> {
    adi_config::Config::open()
        .module("dns")
        .file("frontdoor.toml")
}

/// The front-door hosts to render, materializing the default `frontdoor.toml` on first use so
/// it's there to edit. Any read/parse failure, or an empty list, falls back to the defaults —
/// the front door must always render *something* (never an empty proxy).
fn frontdoor_hosts() -> Vec<String> {
    let hosts = frontdoor_settings()
        .load_or_create()
        .unwrap_or_default()
        .hosts;
    if hosts.is_empty() {
        FrontdoorSettings::default().hosts
    } else {
        hosts
    }
}

/// The bundled `adi-dns`, resolved as a sibling of the running executable, overridable via `ADI_DNS_BIN`.
fn binary_path() -> String {
    sibling_binary("adi-dns", "ADI_DNS_BIN")
}

/// The bundled `adi-hive` (the front-door proxy), resolved like `adi-dns`, overridable via `ADI_HIVE_BIN`.
fn hive_binary_path() -> String {
    sibling_binary("adi-hive", "ADI_HIVE_BIN")
}

/// Resolve a bundled binary as a sibling of the running executable, honoring `env_override` first.
pub(crate) fn sibling_binary(name: &str, env_override: &str) -> String {
    if let Some(p) = std::env::var_os(env_override)
        && !p.is_empty()
    {
        return p.to_string_lossy().into_owned();
    }
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.join(name)))
        .map_or_else(|| name.to_string(), |p| p.to_string_lossy().into_owned())
}

// MARK: config rendering (pure — unit-tested)

fn render_config() -> String {
    format!(
        "# Written by adi-core — edits are overwritten when the CLI rewrites it.\n\
         domain = \"{DOMAIN}\"\n\
         bind_addr = \"127.0.0.1\"\n\
         preferred_port = {PORT}\n\
         fallback_ports = []\n\
         upstreams = [\"1.1.1.1:53\", \"8.8.8.8:53\"]\n\
         manage_os_routing = false\n\
         status_file = \"{status}\"\n\
         \n\
         # Route .{DOMAIN} to the front-door address so http://<name>.{DOMAIN}/ hits adi-hive.\n\
         [[overrides]]\n\
         suffix = \"{DOMAIN}\"\n\
         address = \"{FRONTDOOR_ADDR}\"\n",
        status = status_file().to_string_lossy(),
    )
}

/// The front-door `hive.yaml`: adi-hive binds `127.0.0.53:80` and **proxies** every host in
/// `hosts` (from [`frontdoor_hosts`]) to the control panel (`adi-app`) on `app_port` — all to
/// the same process, so `api.adi` reaches the same `/api` `app.adi` serves. It no longer *runs*
/// adi-app — that's a separate per-user `LaunchAgent` ([`crate::app`]) so the on/off toggle can
/// start/stop it (and its in-process mesh) without a password. Any other host gets the 4XX page.
fn render_frontdoor_hive(hosts: &[String], app_port: u16) -> String {
    // One `services:` entry per host, keyed by the host's first label (`app.adi` → `app`). All
    // point at the same `app_port` — different names for one upstream. Built as a plain literal
    // so YAML indentation is exact.
    use std::fmt::Write as _;
    let mut routes = String::new();
    for host in hosts {
        let name = host
            .split('.')
            .next()
            .filter(|s| !s.is_empty())
            .unwrap_or(host);
        let _ = write!(
            routes,
            "  {name}:\n    proxy:\n      host: {host}\n    rollout:\n      recreate:\n        ports:\n          http: {app_port}\n"
        );
    }
    format!(
        "# Written by adi-core — adi-hive front door for the .{DOMAIN} zone.
# Always-on plumbing: proxies the hosts below to the adi control panel (adi-app), which runs
# as its own per-user LaunchAgent on this reserved port so it can be toggled without a
# password. Hosts come from ~/.adi/mono/dns/frontdoor.toml. Any other host gets the 4XX page.
proxy:
  bind:
    - \"{FRONTDOOR_ADDR}:{FRONTDOOR_PORT}\"
services:
{routes}"
    )
}

fn write_config() {
    let _ = std::fs::create_dir_all(service_dir());
    let _ = std::fs::write(config_path(), render_config());
}

/// Stage the front-door daemon's config + plist (unprivileged); pins `HOME`/`ADI_DIR` to the installing user's, since the root daemon would otherwise use `/var/root/.adi`.
fn write_frontdoor_artifacts() {
    let _ = std::fs::create_dir_all(service_dir());
    let _ = std::fs::write(
        frontdoor_config_path(),
        render_frontdoor_hive(&frontdoor_hosts(), crate::app::port()),
    );
    let env = [
        ("RUST_LOG".to_string(), "info".to_string()),
        (
            "HOME".to_string(),
            std::env::var("HOME").unwrap_or_default(),
        ),
        ("ADI_DIR".to_string(), paths::dir_name()),
        // The front door is the one root piece the updater can't kickstart without a
        // password, so it watches its own binary and exits when the bundle is swapped —
        // launchd's KeepAlive then respawns the new build (see adi-hive's self-watch).
        ("ADI_WATCH_SELF".to_string(), "1".to_string()),
    ];
    let plist = launchd::plist_xml(
        FRONTDOOR_LABEL,
        &[
            hive_binary_path(),
            frontdoor_config_path().to_string_lossy().into_owned(),
        ],
        FRONTDOOR_LOG,
        &env,
    );
    let _ = std::fs::write(frontdoor_plist_stage(), plist);
}

/// True when the installed front-door config already matches what we'd render now, so no
/// privileged update/restart is needed. A mismatch (or missing file) means the front door
/// is running an old config and should be refreshed once.
fn frontdoor_config_current() -> bool {
    let rendered = render_frontdoor_hive(&frontdoor_hosts(), crate::app::port());
    std::fs::read_to_string(frontdoor_config_path()).is_ok_and(|on_disk| on_disk == rendered)
}

/// True when the installed root daemon plist is the standard one we manage — it runs
/// the rendered front-door config. A dev machine may deliberately repoint the daemon
/// at another binary/config (e.g. `target/release/adi-hive` with the full
/// `hive/hive.yaml`); that plist is hand-managed and `up` must never overwrite it.
fn frontdoor_plist_managed() -> bool {
    let marker = frontdoor_config_path();
    let marker = marker.to_string_lossy();
    std::fs::read_to_string(FRONTDOOR_DAEMON_PLIST).is_ok_and(|p| p.contains(marker.as_ref()))
}

/// True when the installed root daemon plist already carries the self-watch env — the
/// one-time migration that lets auto-updates restart the front door without a password.
/// Deliberately a marker check, not a byte compare: the plist embeds the machine's
/// binary path, which legitimately differs between installs.
fn frontdoor_plist_current() -> bool {
    std::fs::read_to_string(FRONTDOOR_DAEMON_PLIST).is_ok_and(|p| p.contains("ADI_WATCH_SELF"))
}

/// The DNS command surface (`adi.dns.*`) — a zero-sized facade; all state lives on disk / in launchd.
#[derive(Debug, Default, Clone, Copy)]
pub struct Dns;

#[allow(clippy::unused_self)]
impl Dns {
    #[must_use]
    pub fn new() -> Self {
        Self
    }

    /// Both bits must be present; a missing either re-runs the idempotent install rather than stranding a half state.
    #[must_use]
    pub fn route_installed(self) -> bool {
        resolver_file().exists() && PathBuf::from(FRONTDOOR_DAEMON_PLIST).exists()
    }

    /// The one privileged step: install the `/etc/resolver` route AND the root front-door daemon in a single admin prompt.
    pub fn install_route(self) {
        let _ = std::fs::create_dir_all(service_dir());
        let _ = std::fs::write(stage_path(), format!("nameserver 127.0.0.1\nport {PORT}\n"));
        write_frontdoor_artifacts();

        let stage = stage_path();
        let stage = stage.to_string_lossy();
        let resolver = resolver_file();
        let resolver = resolver.to_string_lossy();
        let plist_stage = frontdoor_plist_stage();
        let plist_stage = plist_stage.to_string_lossy();
        let shell = format!(
            "mkdir -p /etc/resolver\
             && cp '{stage}' '{resolver}'\
             && chmod 644 '{resolver}'\
             && cp '{plist_stage}' '{FRONTDOOR_DAEMON_PLIST}'\
             && chown root:wheel '{FRONTDOOR_DAEMON_PLIST}'\
             && chmod 644 '{FRONTDOOR_DAEMON_PLIST}'\
             && (launchctl bootout system/{FRONTDOOR_LABEL} 2>/dev/null || true)\
             && launchctl bootstrap system '{FRONTDOOR_DAEMON_PLIST}'\
             && launchctl enable system/{FRONTDOOR_LABEL}\
             && dscacheutil -flushcache\
             && killall -HUP mDNSResponder"
        );
        proc::run_admin(&shell);
    }

    /// Update the installed front door to the current config **and plist** and restart
    /// it — the one privileged step of an upgrade (a single admin prompt). Needed when
    /// the on-disk front-door config or the daemon plist is stale; after this the front
    /// door is proxy-only and the on/off toggle never touches it again.
    pub fn update_frontdoor(self) {
        write_frontdoor_artifacts();
        let plist_stage = frontdoor_plist_stage();
        let plist_stage = plist_stage.to_string_lossy();
        // A plist change (env, args) only takes effect through bootout → bootstrap;
        // `kickstart -k` restarts the job but never re-reads the plist. bootout is
        // async, so the bootstrap is retried until the old job has fully unloaded and
        // :80 can be rebound.
        let shell = format!(
            "set -e\
             ; cp '{plist_stage}' '{FRONTDOOR_DAEMON_PLIST}'\
             ; chown root:wheel '{FRONTDOOR_DAEMON_PLIST}'\
             ; chmod 644 '{FRONTDOOR_DAEMON_PLIST}'\
             ; launchctl bootout system/{FRONTDOOR_LABEL} 2>/dev/null || true\
             ; n=0\
             ; until launchctl bootstrap system '{FRONTDOOR_DAEMON_PLIST}' 2>/dev/null; do n=$((n+1)); if [ \"$n\" -ge 25 ]; then exit 1; fi; sleep 0.2; done\
             ; launchctl enable system/{FRONTDOOR_LABEL}"
        );
        proc::run_admin(&shell);
    }

    /// Tear down both privileged bits, best-effort (incl. the `lo0` alias).
    pub fn remove_route(self) {
        let resolver = resolver_file();
        let resolver = resolver.to_string_lossy();
        let shell = format!(
            "(launchctl bootout system/{FRONTDOOR_LABEL} 2>/dev/null || true)\
             ; rm -f '{FRONTDOOR_DAEMON_PLIST}'\
             ; rm -f '{resolver}'\
             ; (ifconfig lo0 -alias {FRONTDOOR_ADDR} 2>/dev/null || true)\
             ; dscacheutil -flushcache\
             ; killall -HUP mDNSResponder"
        );
        proc::run_admin(&shell);
    }
}

impl Service for Dns {
    fn id(&self) -> &'static str {
        "dns"
    }
    fn name(&self) -> &'static str {
        "DNS"
    }
    fn label(&self) -> String {
        LABEL.to_string()
    }
    fn status_path(&self) -> PathBuf {
        status_file()
    }
    fn log_path(&self) -> PathBuf {
        paths::logs_dir().join("adi-dns.log")
    }

    fn program(&self) -> Vec<String> {
        write_config();
        vec![binary_path(), config_path().to_string_lossy().into_owned()]
    }

    // Installed once and left in place, so toggling never re-prompts; removal is an explicit
    // action. The one exception is a stale front-door config or daemon plist (e.g. upgrading
    // from the old runner-based front door, or rolling out the self-watch env) — update it
    // once here. A hand-repointed daemon plist (dev machines) is never auto-migrated:
    // `install-route` stays the explicit way to reclaim it.
    fn on_enable(&self) {
        if !self.route_installed() {
            self.install_route();
        } else if frontdoor_plist_managed()
            && (!frontdoor_config_current() || !frontdoor_plist_current())
        {
            self.update_frontdoor();
        }
    }

    fn detail(&self, status: Option<&DaemonStatus>) -> String {
        status.map_or_else(String::new, |s| format!("Running · 127.0.0.1:{}", s.port))
    }

    fn extra_actions(&self) -> Vec<Action> {
        vec![route_action(self.route_installed())]
    }
}

/// The install/remove-route action for the current route state.
fn route_action(installed: bool) -> Action {
    let (title, verb) = if installed {
        (format!("Remove .{DOMAIN} route + page"), "remove-route")
    } else {
        (format!("Install .{DOMAIN} route + page…"), "install-route")
    };
    Action {
        id: "route".to_string(),
        title,
        args: vec!["dns".to_string(), verb.to_string()],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_has_expected_fields() {
        let cfg = render_config();
        assert!(cfg.contains("domain = \"adi\""));
        assert!(cfg.contains("preferred_port = 10053"));
        assert!(cfg.contains("suffix = \"adi\""));
        assert!(cfg.contains("address = \"127.0.0.53\""));
        assert!(cfg.contains("status_file = \""));
    }

    #[test]
    fn frontdoor_hive_proxies_the_control_panel_and_is_valid_yaml() {
        let hosts = vec!["app.adi".to_string(), "api.adi".to_string()];
        let cfg = render_frontdoor_hive(&hosts, 8091);
        assert!(cfg.contains("- \"127.0.0.53:80\""));

        let v: serde_yaml_ng::Value = serde_yaml_ng::from_str(&cfg).expect("valid YAML");
        assert_eq!(v["proxy"]["bind"][0].as_str(), Some("127.0.0.53:80"));

        for (name, host) in [("app", "app.adi"), ("api", "api.adi")] {
            let svc = &v["services"][name];
            assert_eq!(svc["proxy"]["host"].as_str(), Some(host));
            assert_eq!(
                svc["rollout"]["recreate"]["ports"]["http"].as_u64(),
                Some(8091)
            );
            assert!(svc["runner"].is_null());
        }
    }

    #[test]
    fn frontdoor_settings_default_to_app_and_api_dot_adi() {
        assert_eq!(FrontdoorSettings::default().hosts, ["app.adi", "api.adi"]);
    }

    #[test]
    fn route_action_reflects_installed_state() {
        assert_eq!(route_action(false).args, vec!["dns", "install-route"]);
        assert_eq!(route_action(true).args, vec!["dns", "remove-route"]);
    }
}
