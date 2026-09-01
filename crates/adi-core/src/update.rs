//! Auto-update as an ADI service: the [`Update`] facade over the `adi-update` engine,
//! plus [`Updater`] — a *periodic* `LaunchAgent` (`family.adi.app.updater`) that runs
//! `adi-mono update run --quiet` at login and every few hours. One update swaps the
//! whole app bundle, so every bundled binary — including ones added by future
//! releases — ships in a single artifact with no updater changes.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use adi_update::{Check, Engine, Error as UpdateError, State};
use serde::Serialize;

use crate::commands::Adi;
use crate::dns::sibling_binary;
use crate::launchd;
use crate::paths;
use crate::proc;
use crate::service::{Action, Service};
use crate::status::DaemonStatus;

const LABEL: &str = "family.adi.app.updater";

/// How long the restarted stack gets to come back before the update is judged a failure and
/// rolled back. Generous: a cold start compiles nothing but does bind ports, read the store and
/// (on a node) wait for the mesh, and a false rollback is worse than a slow one.
const HEALTH_TIMEOUT: Duration = Duration::from_secs(90);

/// How often the health check re-asks. Short enough that a healthy update finishes promptly.
const HEALTH_POLL: Duration = Duration::from_millis(500);

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
    /// The new version installed but the stack didn't come back, so the previous one was put
    /// back and restarted. `from` is the version that failed.
    RolledBack {
        from: String,
        to: String,
        why: String,
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
    /// install it. With `restart`, the running stack is moved onto the new binaries — and if
    /// it doesn't come back, the previous version is restored and restarted.
    ///
    /// # Errors
    /// Any [`UpdateError`]; a failed swap rolls the previous install back in place. A rollback
    /// that *itself* fails is an error rather than a [`RunOutcome`], because at that point the
    /// machine is running a version known to be broken and nobody should have to read past a
    /// success line to find that out.
    pub fn run(self, force: bool, restart: bool) -> Result<RunOutcome, UpdateError> {
        let engine = Engine::open();
        let check = engine.check()?;
        if !check.update_available && !force {
            return Ok(RunOutcome::UpToDate {
                installed: check.installed,
                latest: check.latest,
            });
        }

        // Snapshot before anything moves: only services that were *already* up are expected
        // back afterwards. Without this, a stack the user had deliberately stopped would read
        // as a failed update and roll back a release that is perfectly fine.
        let expected = if restart {
            running_services()
        } else {
            Vec::new()
        };

        let installed = engine.install(&check.manifest)?;
        if !restart {
            return Ok(RunOutcome::Installed {
                from: installed.from,
                to: installed.to,
                restarted: false,
            });
        }

        restart_onto(&installed.path);
        if let Err(why) = wait_healthy(&expected) {
            engine.rollback(&installed, &why)?;
            // `installed.path` holds the previous version again — restart onto that.
            restart_onto(&installed.path);
            return Ok(RunOutcome::RolledBack {
                from: installed.to,
                to: installed.from,
                why,
            });
        }
        Ok(RunOutcome::Installed {
            from: installed.from,
            to: installed.to,
            restarted: true,
        })
    }
}

/// The ids of every managed service currently up — the health check's expectations.
fn running_services() -> Vec<String> {
    Adi::new()
        .services()
        .iter()
        .filter(|svc| svc.is_running())
        .map(|svc| svc.id().to_string())
        .collect()
}

/// Wait until every service in `expected` is running again, or give up after
/// [`HEALTH_TIMEOUT`] and name the ones still down.
///
/// This is what stands between a bad release and a bricked machine: the supervisors restart a
/// crashing binary forever, so without a verdict here a broken update would sit in a crash loop
/// with no DNS, no front door, and no way left to deliver the fix.
fn wait_healthy(expected: &[String]) -> Result<(), String> {
    if expected.is_empty() {
        return Ok(());
    }
    let deadline = Instant::now() + HEALTH_TIMEOUT;
    loop {
        let down: Vec<String> = Adi::new()
            .services()
            .iter()
            .filter(|svc| expected.iter().any(|id| id == svc.id()) && !svc.is_running())
            .map(|svc| svc.name().to_string())
            .collect();
        if down.is_empty() {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(format!(
                "{} did not come back within {}s of the restart",
                down.join(", "),
                HEALTH_TIMEOUT.as_secs()
            ));
        }
        std::thread::sleep(HEALTH_POLL);
    }
}

/// The bundled CLI inside an install — `Contents/Resources/adi-mono` in a macOS bundle, or
/// the binary directory itself on a node.
fn mono_in(install: &Path) -> PathBuf {
    let file = format!("{}{}", crate::BIN_NAME, std::env::consts::EXE_SUFFIX);
    if cfg!(target_os = "macos") {
        install.join("Contents/Resources").join(file)
    } else {
        install.join(file)
    }
}

/// Move the running stack onto the freshly-swapped install:
///
/// * kickstart the per-user services (DNS resolver + control panel) through whichever
///   supervisor this OS uses — launchd, `systemd --user`, or Task Scheduler; no password;
/// * on macOS the root front door is **not** touched here — it watches its own binary
///   (`ADI_WATCH_SELF`, see `adi-hive`) and exits once the bundle changes, so
///   launchd's `KeepAlive` respawns the new build without an admin prompt;
/// * relaunch the menu-bar app if it was running (macOS only — a node is headless);
/// * run the **new** `adi-mono up` to reconcile — this is what enables services a
///   newer version introduces, so future additions need no updater changes.
fn restart_onto(app: &Path) {
    for label in [crate::dns::label(), crate::app::label()] {
        if launchd::is_loaded(&label) {
            launchd::kickstart(&label);
        }
    }
    if cfg!(target_os = "macos") {
        relaunch_menubar(app);
    }
    let mono = mono_in(app);
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
        Some("rolled-back") => format!("{installed} · an update was rolled back"),
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

        state.last_outcome = Some("rolled-back".to_string());
        assert_eq!(detail_line(&state), "0.1.0 · an update was rolled back");
    }

    #[test]
    fn the_cli_is_found_where_this_platform_keeps_it() {
        let install = std::path::Path::new("/tmp/install");
        let mono = mono_in(install);
        assert!(mono.starts_with(install));
        assert_eq!(
            mono.file_stem().and_then(|s| s.to_str()),
            Some(crate::BIN_NAME)
        );
        // The macOS payload is a bundle; every other platform installs loose binaries.
        assert_eq!(
            mono.parent() == Some(install),
            !cfg!(target_os = "macos"),
            "a bundle nests the CLI under Contents/Resources, a node package does not"
        );
    }

    #[test]
    fn an_empty_expectation_set_is_healthy_without_waiting() {
        // Nothing was running before the update, so nothing has to come back — and the check
        // must not sit through its timeout deciding that.
        let started = std::time::Instant::now();
        assert!(wait_healthy(&[]).is_ok());
        assert!(started.elapsed() < HEALTH_TIMEOUT);
    }

    #[test]
    fn updater_program_is_the_cli_run_quietly() {
        let program = Updater::new().program();
        // The resolved binary is the CLI itself — `adi-mono` on Unix, `adi-mono.exe` on Windows,
        // where `sibling_binary` appends the executable suffix.
        let file = std::path::Path::new(&program[0])
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or(&program[0]);
        assert_eq!(file, crate::BIN_NAME);
        assert_eq!(&program[1..], ["update", "run", "--quiet"]);
    }
}
