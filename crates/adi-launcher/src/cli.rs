//! The bridge to `adi-core`, through the `adi-mono` CLI sitting next to this executable.
//!
//! Deliberately the same arrangement as the macOS app's `Core.swift`: the launcher owns no
//! service, route or config logic of its own — every action is `adi-mono <args>` and all live
//! state is the JSON of `adi-mono status --json`. One definition of "running", in Rust, in
//! `adi-core`; nothing here can drift from it.

use std::path::PathBuf;
use std::process::Command;

use serde::Deserialize;

/// The CLI's file name. Slated to be renamed to `adi`; this constant and
/// `crates/adi-cli/Cargo.toml`'s `[[bin]] name` are the two places that decide it.
pub const BINARY: &str = "adi-mono";

/// The zone this install serves. Windows ships the release flavour only — there is no
/// `ADI Dev.exe` to keep off the real install, which is what the macOS bundle's `ADIDomain`
/// key exists to prevent.
pub const DOMAIN: &str = "adi";

/// Codable mirror of what `adi-mono status --json` emits (see `crates/adi-core/src/commands.rs`).
/// Only the fields the tray actually renders; serde ignores the rest, so a new service or action
/// appearing in the core needs no change here.
#[derive(Debug, Default, Clone, Deserialize)]
pub struct Report {
    #[serde(default)]
    pub any_running: bool,
    #[serde(default)]
    pub services: Vec<Service>,
    #[serde(default)]
    pub setup: Setup,
}

#[derive(Debug, Default, Clone, Deserialize)]
pub struct Service {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub detail: String,
}

/// What still has to happen before `http://app.adi/` resolves. On Windows that is one thing —
/// the NRPT rule — and it is optional: the panel is always reachable on its loopback port.
#[derive(Debug, Default, Clone, Deserialize)]
pub struct Setup {
    #[serde(default)]
    pub dns_route: bool,
}

impl Report {
    /// True when at least one service is enabled, whether or not it has come up yet — the
    /// difference between "Off" and "Starting…".
    pub fn any_enabled(&self) -> bool {
        self.services.iter().any(|s| s.enabled)
    }

    /// Where to send the browser.
    ///
    /// The friendly name only once the route is actually installed: opening `http://app.adi/`
    /// on a machine without the NRPT rule lands on a browser error page, which reads as "ADI is
    /// broken" rather than "one optional step was skipped". Without it, the panel's own loopback
    /// port — read from the `app` service's detail line, because the port is allocated by the
    /// ports registry and never hard-coded.
    pub fn dashboard_url(&self) -> String {
        if self.setup.dns_route {
            return format!("http://app.{DOMAIN}/");
        }
        self.app_port().map_or_else(
            || format!("http://app.{DOMAIN}/"),
            |p| format!("http://127.0.0.1:{p}/"),
        )
    }

    /// The loopback port from the `app` service's `detail` (`127.0.0.1:<port>`).
    fn app_port(&self) -> Option<u16> {
        let detail = &self.services.iter().find(|s| s.id == "app")?.detail;
        let rest = detail.split("127.0.0.1:").nth(1)?;
        let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
        digits.parse().ok()
    }
}

/// The bundled CLI, resolved as a sibling of this executable — the same rule `adi-core` uses to
/// find the daemons (`sibling_binary`), so the whole install stays relocatable: what matters is
/// that the five files sit in one directory, not which directory it is.
pub fn binary_path() -> PathBuf {
    let name = format!("{BINARY}{}", std::env::consts::EXE_SUFFIX);
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.join(&name)))
        .unwrap_or_else(|| PathBuf::from(name))
}

/// Run `adi-mono <args>` to completion; the exit status and its combined output.
///
/// Blocking, and every caller runs it on a thread of its own: `up` starts four services and
/// `dns install-route` waits on a UAC prompt, neither of which may stall the message loop —
/// a tray icon that stops answering is a hung app.
pub fn run(args: &[&str]) -> (i32, String) {
    let mut command = Command::new(binary_path());
    command.args(args);
    no_console(&mut command);
    match command.output() {
        Ok(out) => {
            let mut text = String::from_utf8_lossy(&out.stdout).into_owned();
            text.push_str(&String::from_utf8_lossy(&out.stderr));
            (out.status.code().unwrap_or(-1), text)
        }
        Err(e) => (-1, format!("could not run {BINARY}: {e}")),
    }
}

/// `adi-mono status --json`, decoded. `None` when the CLI could not be run or said something
/// this build does not understand — the caller keeps the last good report rather than blinking
/// the tray to "Off" on one failed poll.
pub fn report() -> Option<Report> {
    let (status, output) = run(&["status", "--json"]);
    if status != 0 {
        return None;
    }
    serde_json::from_str(&output).ok()
}

/// Keep a spawned CLI from flashing a console window on screen.
///
/// This process has no console of its own (`windows_subsystem = "windows"`), so a child that
/// wants one gets a brand new window — a black rectangle that appears and vanishes on every
/// two-second status poll.
#[cfg(windows)]
fn no_console(command: &mut Command) {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    command.creation_flags(CREATE_NO_WINDOW);
}

#[cfg(not(windows))]
fn no_console(_command: &mut Command) {}

#[cfg(test)]
mod tests {
    use super::*;

    fn report_from(json: &str) -> Report {
        serde_json::from_str(json).expect("status json")
    }

    #[test]
    fn dashboard_url_prefers_the_friendly_name_once_the_route_is_installed() {
        let report = report_from(
            r#"{"any_running":true,"setup":{"dns_route":true},
                "services":[{"id":"app","enabled":true,"detail":"serving 127.0.0.1:8000"}]}"#,
        );
        assert_eq!(report.dashboard_url(), "http://app.adi/");
    }

    #[test]
    fn dashboard_url_falls_back_to_the_allocated_loopback_port() {
        let report = report_from(
            r#"{"any_running":true,"setup":{"dns_route":false},
                "services":[{"id":"dns","enabled":true,"detail":"127.0.0.1:53"},
                            {"id":"app","enabled":true,"detail":"serving 127.0.0.1:8123 (pid 4)"}]}"#,
        );
        assert_eq!(report.dashboard_url(), "http://127.0.0.1:8123/");
    }

    #[test]
    fn an_unparsable_status_is_not_a_crash() {
        // Every field is optional on purpose: an older or newer core that adds or drops one
        // must leave the tray running, not take it down.
        let report = report_from(r#"{"services":[]}"#);
        assert!(!report.any_running);
        assert!(!report.any_enabled());
        assert_eq!(report.dashboard_url(), "http://app.adi/");
    }

    #[test]
    fn enabled_but_not_yet_answering_is_distinguishable_from_off() {
        let report = report_from(
            r#"{"any_running":false,"services":[{"id":"app","enabled":true,"detail":""}]}"#,
        );
        assert!(report.any_enabled());
        assert!(!report.any_running);
    }

    #[test]
    fn the_cli_is_looked_for_next_to_this_executable() {
        let path = binary_path();
        assert_eq!(
            path.file_name().and_then(|n| n.to_str()),
            Some(format!("{BINARY}{}", std::env::consts::EXE_SUFFIX).as_str())
        );
        assert_eq!(
            path.parent(),
            std::env::current_exe()
                .ok()
                .and_then(|e| e.parent().map(std::path::Path::to_path_buf))
                .as_deref()
        );
    }
}
