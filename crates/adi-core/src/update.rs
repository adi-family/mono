//! Auto-update as an ADI service: the [`Update`] facade over the `adi-update` engine,
//! plus [`Updater`] — a *periodic* `LaunchAgent` (`family.adi.app.updater`) that runs
//! `adi-mono update run --quiet` at login and every few hours. One update swaps the
//! whole app bundle, so every bundled binary — including ones added by future
//! releases — ships in a single artifact with no updater changes.

use std::path::{Path, PathBuf};

use adi_update::{Check, Engine, Error as UpdateError, State};
use serde::Serialize;

use crate::dns::sibling_binary;
use crate::launchd;
use crate::paths;
use crate::proc;
use crate::service::{Action, Service};
use crate::status::DaemonStatus;

const LABEL: &str = "family.adi.app.updater";

/// What `update run` did — the CLI prints it (or serializes it with `--json`).
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "outcome", rename_all = "kebab-case")]
pub enum RunOutcome {
    UpToDate {
        installed: String,
        latest: String,
    },
    Installed {
        from: String,
        to: String,
        restarted: bool,
    },
}

/// The update command surface (`adi.update().*`) — a zero-sized facade like `Dns`.
#[derive(Debug, Default, Clone, Copy)]
pub struct Update;

#[allow(clippy::unused_self)]
impl Update {
    #[must_use]
    pub fn new() -> Self {
        Self
    }

    /// Fetch the release manifest and compare against the installed app.
    ///
    /// # Errors
    /// [`UpdateError`] when the manifest can't be fetched or parsed.
    pub fn check(self) -> Result<Check, UpdateError> {
        Engine::open().check()
    }

    /// The persisted last check/install record.
    #[must_use]
    pub fn state(self) -> State {
        Engine::open().state()
    }

    /// Check and, when a newer version is published (or `force`), download + verify +
    /// install it. With `restart`, the running stack is moved onto the new binaries.
    ///
    /// # Errors
    /// Any [`UpdateError`]; a failed swap rolls the previous install back in place.
    pub fn run(self, force: bool, restart: bool) -> Result<RunOutcome, UpdateError> {
        let engine = Engine::open();
        let check = engine.check()?;
        if !check.update_available && !force {
            return Ok(RunOutcome::UpToDate {
                installed: check.installed,
                latest: check.latest,
            });
        }
        let installed = engine.install(&check.manifest)?;
        if restart {
            restart_onto(&installed.app);
        }
        Ok(RunOutcome::Installed {
            from: installed.from,
            to: installed.to,
            restarted: restart,
        })
    }
}

/// Move the running stack onto the freshly-swapped bundle:
///
/// * kickstart the per-user agents (DNS resolver + control panel) — no password;
/// * the root front door is **not** touched here — it watches its own binary
///   (`ADI_WATCH_SELF`, see `adi-hive`) and exits once the bundle changes, so
///   launchd's `KeepAlive` respawns the new build without an admin prompt;
/// * relaunch the menu-bar app if it was running;
/// * run the **new** `adi-mono up` to reconcile — this is what enables services a
///   newer version introduces, so future additions need no updater changes.
fn restart_onto(app: &Path) {
    for label in [crate::dns::LABEL, crate::app::LABEL] {
        if launchd::is_loaded(label) {
            launchd::kickstart(label);
        }
    }
    relaunch_menubar(app);
    let mono = app.join("Contents/Resources").join(crate::BIN_NAME);
    if mono.exists() {
        let up = proc::run(&[mono.to_string_lossy().as_ref(), "up"]);
        if !up.ok() {
            eprintln!(
                "adi: post-update `{} up` failed: {}",
                crate::BIN_NAME,
                up.text.trim()
            );
        }
    }
}

/// If the menu-bar app is running, terminate it (plain SIGTERM — never Apple Events,
/// which would trigger a TCC automation prompt) and reopen it from the new bundle.
fn relaunch_menubar(app: &Path) {
    let exe = app.join("Contents/MacOS/ADI");
    let exe = exe.to_string_lossy().into_owned();
    if !proc::run(&["/usr/bin/pgrep", "-f", &exe]).ok() {
        return;
    }
    let _ = proc::run(&["/usr/bin/pkill", "-TERM", "-f", &exe]);
    for _ in 0..50 {
        if !proc::run(&["/usr/bin/pgrep", "-f", &exe]).ok() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    let _ = proc::run(&["/usr/bin/open", &app.to_string_lossy()]);
}

/// The background auto-updater as a service row. A periodic one-shot job, not a
/// daemon — "running" mirrors whether it's scheduled, and the detail line summarizes
/// the last check from `update/state.json`.
#[derive(Debug, Default, Clone, Copy)]
pub struct Updater;

impl Updater {
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

impl Service for Updater {
    fn id(&self) -> &'static str {
        "update"
    }
    fn name(&self) -> &'static str {
        "Updates"
    }
    fn label(&self) -> String {
        LABEL.to_string()
    }
    fn status_path(&self) -> PathBuf {
        adi_config::Config::open()
            .module("update")
            .raw_path("state.json")
    }
    fn log_path(&self) -> PathBuf {
        paths::logs_dir().join("adi-updater.log")
    }

    /// The agent runs the bundled CLI itself: `adi-mono update run --quiet`.
    fn program(&self) -> Vec<String> {
        vec![
            sibling_binary(crate::BIN_NAME, "ADI_MONO_BIN"),
            "update".to_string(),
            "run".to_string(),
            "--quiet".to_string(),
        ]
    }

    /// Periodic install: `StartInterval` from the updater settings instead of `KeepAlive`.
    fn enable(&self) {
        let interval = Engine::open().settings().check_interval_secs();
        launchd::enable_periodic(
            &self.label(),
            &self.program(),
            &self.log_path().to_string_lossy(),
            &self.env(),
            interval,
        );
        self.on_enable();
    }

    /// A one-shot job has no long-lived PID; scheduled == running.
    fn is_running(&self) -> bool {
        launchd::is_loaded(&self.label())
    }

    fn detail(&self, _status: Option<&DaemonStatus>) -> String {
        detail_line(&Engine::open().state())
    }

    fn extra_actions(&self) -> Vec<Action> {
        vec![Action {
            id: "check".to_string(),
            title: "Check for updates now".to_string(),
            args: vec!["update".to_string(), "run".to_string()],
        }]
    }
}

/// One human line for the GUI row, from the persisted state.
fn detail_line(state: &State) -> String {
    let installed = state.installed_version.as_deref().unwrap_or("?");
    match state.last_outcome.as_deref() {
        Some("update-available") => format!(
            "Update {} available",
            state.latest_version.as_deref().unwrap_or("?")
        ),
        Some("installed") => format!("Updated to {installed}"),
        Some("error") => format!("{installed} · last check failed"),
        Some("up-to-date") => format!("{installed} · up to date"),
        _ => "Auto-update scheduled".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detail_line_covers_every_outcome() {
        let mut state = State {
            installed_version: Some("0.1.0".to_string()),
            latest_version: Some("0.2.0".to_string()),
            ..State::default()
        };
        assert_eq!(detail_line(&state), "Auto-update scheduled");

        state.last_outcome = Some("up-to-date".to_string());
        assert_eq!(detail_line(&state), "0.1.0 · up to date");

        state.last_outcome = Some("update-available".to_string());
        assert_eq!(detail_line(&state), "Update 0.2.0 available");

        state.last_outcome = Some("installed".to_string());
        assert_eq!(detail_line(&state), "Updated to 0.1.0");

        state.last_outcome = Some("error".to_string());
        assert_eq!(detail_line(&state), "0.1.0 · last check failed");
    }

    #[test]
    fn updater_program_is_the_cli_run_quietly() {
        let program = Updater::new().program();
        assert!(program[0].ends_with(crate::BIN_NAME));
        assert_eq!(&program[1..], ["update", "run", "--quiet"]);
    }
}
