//! The `adi-dns` resolver as an ADI service, split by privilege so the on/off toggle
//! never needs a password: an unprivileged per-user resolver `LaunchAgent`, plus a root
//! front-door `LaunchDaemon` (`adi-hive` on `127.0.0.53:80`) installed once.

use std::path::PathBuf;

use crate::launchd;
use crate::paths;
use crate::proc;
use crate::service::{Action, Service};
use crate::status::DaemonStatus;

const DOMAIN: &str = "adi";
const PORT: u16 = 10053;
const LABEL: &str = "family.adi.app.dns";

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
    service_dir().join("status.json")
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

/// The bundled `adi-dns`, resolved as a sibling of the running executable, overridable via `ADI_DNS_BIN`.
fn binary_path() -> String {
    sibling_binary("adi-dns", "ADI_DNS_BIN")
}

/// The bundled `adi-hive` (the front-door proxy), resolved like `adi-dns`, overridable via `ADI_HIVE_BIN`.
fn hive_binary_path() -> String {
    sibling_binary("adi-hive", "ADI_HIVE_BIN")
}

/// The bundled `adi-app` (the control panel the front door runs), overridable via `ADI_APP_BIN`.
fn app_binary_path() -> String {
    sibling_binary("adi-app", "ADI_APP_BIN")
}

/// Resolve a bundled binary as a sibling of the running executable, honoring `env_override` first.
fn sibling_binary(name: &str, env_override: &str) -> String {
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

/// The front-door `hive.yaml`: adi-hive binds `127.0.0.53:80` and serves the adi control panel (`adi-app`) at `app.{DOMAIN}`; any other host gets the 4XX page.
fn render_frontdoor_hive(app_bin: &str) -> String {
    // Plain multi-line literal so YAML indentation is exact (`\`-continuation would strip leading spaces).
    format!(
        "# Written by adi-core — adi-hive front door for the .{DOMAIN} zone.
# Serves the adi control panel (adi-app) at app.{DOMAIN}; adi-hive takes its port
# from the ports manager. Any other host gets the animated 4XX page.
proxy:
  bind:
    - \"{FRONTDOOR_ADDR}:{FRONTDOOR_PORT}\"
services:
  app:
    proxy:
      host: app.{DOMAIN}
    restart: on-failure
    runner:
      type: script
      script:
        run: \"'{app_bin}'\"
"
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
        render_frontdoor_hive(&app_binary_path()),
    );
    let env = [
        ("RUST_LOG".to_string(), "info".to_string()),
        (
            "HOME".to_string(),
            std::env::var("HOME").unwrap_or_default(),
        ),
        ("ADI_DIR".to_string(), paths::dir_name()),
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

    // Installed once and left in place, so toggling never re-prompts; removal is an explicit action.
    fn on_enable(&self) {
        if !self.route_installed() {
            self.install_route();
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
    fn frontdoor_hive_serves_the_control_panel_and_is_valid_yaml() {
        let cfg = render_frontdoor_hive("/opt/ADI.app/Contents/Resources/adi-app");
        assert!(cfg.contains("- \"127.0.0.53:80\""));

        // It must parse as YAML with the expected shape (catches indentation bugs).
        let v: serde_yaml_ng::Value = serde_yaml_ng::from_str(&cfg).expect("valid YAML");
        assert_eq!(v["proxy"]["bind"][0].as_str(), Some("127.0.0.53:80"));
        let app = &v["services"]["app"];
        assert_eq!(app["proxy"]["host"].as_str(), Some("app.adi"));
        assert_eq!(
            app["runner"]["script"]["run"].as_str(),
            Some("'/opt/ADI.app/Contents/Resources/adi-app'")
        );
        // No port declared -> adi-hive allocates it from the ports manager.
        assert!(app["rollout"].is_null());
    }

    #[test]
    fn route_action_reflects_installed_state() {
        assert_eq!(route_action(false).args, vec!["dns", "install-route"]);
        assert_eq!(route_action(true).args, vec!["dns", "remove-route"]);
    }
}
