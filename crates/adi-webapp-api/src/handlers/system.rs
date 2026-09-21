//! The `/api/system` surface: live status of every managed service, the platform-wide power
//! switch, a bounce for what's running, and a diagnostic report to download — the web twin of
//! the mac app's menu-bar window (`apps/macos/Sources/AppModel.swift`).
//!
//! **Two of these mutations restart the very process answering them.** Neither
//! [`run_system_power`] nor [`restart_system`] calls `adi_core::Adi` in process for that reason —
//! each spawns the bundled `adi-mono` CLI detached, in its own process group, the same shape
//! [`super::update::run_update`] hands an install off in: the request this process is already
//! answering gets its reply, and what tears the process down runs from somewhere that outlives
//! it. [`run_system_action`] is the third mutation here and the *safe* one — every action a
//! service offers on its own row except the `app` service's own toggle, which
//! `adi_core::Adi::run_action` refuses outright for the same reason.
//!
//! [`diagnose_system`] is neither: `adi_core::diagnose` reads only and starts, stops or
//! reconfigures nothing, so it runs in place like any other read.

use std::path::PathBuf;
use std::process::{Command, Stdio};

use adi_agents::Agents;
use adi_core::Adi;

use crate::types::{
    Accepted, DiagnosticReport, RunSystemAction, SetSystemPower, SystemAction, SystemService,
    SystemSetup, SystemStatus,
};

use super::response::{Response, error, ok_json};

/// `GET /api/system` — the page's whole read.
#[must_use]
pub fn system_status(agents: &Agents) -> Response {
    ok_json(&status_dto(agents))
}

fn status_dto(agents: &Agents) -> SystemStatus {
    let report = Adi::new().report();
    SystemStatus {
        any_running: report.any_running,
        services: report
            .services
            .into_iter()
            .map(|s| SystemService {
                id: s.id,
                name: s.name,
                enabled: s.enabled,
                running: s.running,
                detail: s.detail,
                actions: s
                    .actions
                    .into_iter()
                    .map(|a| SystemAction {
                        id: a.id,
                        title: a.title,
                        args: a.args,
                    })
                    .collect(),
            })
            .collect(),
        setup: SystemSetup {
            location_durable: report.setup.location_durable,
            dns_route: report.setup.dns_route,
            front_door: report.setup.front_door,
            front_door_answering: report.setup.front_door_answering,
            ready: report.setup.ready,
        },
        live_runs: u32::try_from(agents.run_load().total()).unwrap_or(u32::MAX),
    }
}

/// `POST /api/system/action` — one action a service offered on its own row (`SystemService`'s own
/// `actions`), run in place. Refuses exactly what `adi_core::Adi::run_action` refuses (the `app`
/// service's own toggle) and nothing more; answers with the fresh status either way it succeeds.
#[must_use]
pub fn run_system_action(agents: &Agents, body: &[u8]) -> Response {
    let Ok(req) = serde_json::from_slice::<RunSystemAction>(body) else {
        return error(400, "expected JSON body { args: [service, verb] }");
    };
    match Adi::new().run_action(&req.args) {
        Ok(()) => ok_json(&status_dto(agents)),
        Err(e) => error(400, &e),
    }
}

/// `POST /api/system/power` — the platform-wide switch (`adi-mono enable` / `adi-mono disable`),
/// the same command the toggle in `apps/macos` runs. Spawned detached: both directions touch the
/// `app` service, which is this very process (`Service::enable`/`disable` bootout/restart a
/// loaded unit), so calling either in place would tear it down mid-reply.
#[must_use]
pub fn run_system_power(body: &[u8]) -> Response {
    let Ok(req) = serde_json::from_slice::<SetSystemPower>(body) else {
        return error(400, "expected JSON body { on: <bool> }");
    };
    let verb = if req.on { "enable" } else { "disable" };
    respond_to_spawn(spawn_detached(&[verb], "power.log"))
}

/// `POST /api/system/restart` — bounce every running service except DNS (`adi_core::Adi::restart`),
/// spawned the same detached way as [`run_system_power`] and for the same reason: the `app`
/// service among the ones it bounces is this very process.
#[must_use]
pub fn restart_system() -> Response {
    respond_to_spawn(spawn_detached(&["restart", "--json"], "restart.log"))
}

fn respond_to_spawn(spawned: Result<u32, String>) -> Response {
    match spawned {
        Ok(_pid) => ok_json(&Accepted { ok: true }),
        Err(e) => error(500, &e),
    }
}

/// Spawn the bundled `adi-mono <args>` detached, in its own process group, with its combined
/// output truncated into `<store>/system/<log_name>` — the shape `super::update::run_update`
/// hands an install off in; `power`/`restart` reuse it for the same reason: whatever this spawns
/// may restart or stop the very process spawning it.
fn spawn_detached(args: &[&str], log_name: &str) -> Result<u32, String> {
    let bin = super::update::mono_bin();
    if !bin.exists() {
        return Err(format!(
            "the bundled CLI is not where this build expects it ({}); set ADI_MONO_BIN to the \
             adi-mono that should run this",
            bin.display()
        ));
    }
    let module = adi_config::Config::open().module("system");
    let _ = module.ensure_dir();

    let mut cmd = Command::new(&bin);
    cmd.args(args).stdin(Stdio::null());
    // Its own process group, so it survives whatever it does to the service that spawned it —
    // same as the updater's own detached install.
    adi_osext::detach_process_group(&mut cmd);
    match std::fs::File::create(module.raw_path(log_name)) {
        Ok(log) => match log.try_clone() {
            Ok(errlog) => {
                cmd.stdout(Stdio::from(log)).stderr(Stdio::from(errlog));
            }
            Err(_) => {
                cmd.stdout(Stdio::from(log)).stderr(Stdio::null());
            }
        },
        Err(_) => {
            cmd.stdout(Stdio::null()).stderr(Stdio::null());
        }
    }
    cmd.spawn()
        .map(|child| child.id())
        .map_err(|e| format!("could not start `adi-mono {}`: {e}", args.join(" ")))
}

/// `POST /api/system/diagnose` — collect one archive now and say where the page can download it.
/// Read-only and synchronous: `adi_core::diagnose` starts, stops and reconfigures nothing, so —
/// unlike power/restart — there is no reason to hand this to a detached process.
#[must_use]
pub fn diagnose_system() -> Response {
    match adi_core::Diagnose::new().collect(None) {
        Ok(bundle) => {
            let file_name = bundle
                .path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            ok_json(&DiagnosticReport {
                download_url: format!("/api/system/diagnose/download/{file_name}"),
                file_name,
                bytes: bundle.bytes,
                files: bundle.files,
                findings: bundle.findings,
            })
        }
        Err(e) => error(500, &e.to_string()),
    }
}

/// Resolve a diagnostic archive's bare file name (from [`DiagnosticReport::download_url`]) to its
/// path on disk — never anything a client-supplied path could escape with: no `/`, no `\`, and the
/// join must still land inside `adi_core::diagnose::reports_dir()` and name a real file. Read by
/// `adi-app`'s own byte-serving route, since a [`Response`] here can only carry JSON.
///
/// `None` for anything that doesn't resolve to a real file in that one directory.
#[must_use]
pub fn diagnose_download_path(file_name: &str) -> Option<PathBuf> {
    if file_name.is_empty() || file_name.contains(['/', '\\']) {
        return None;
    }
    let path = adi_core::diagnose::reports_dir().join(file_name);
    // `join` above can't escape (the name is checked bare above), but confirming the parent
    // still matches is what keeps this correct if that ever stops being true rather than relying
    // on it staying true forever.
    (path.parent() == Some(adi_core::diagnose::reports_dir().as_path()) && path.is_file())
        .then_some(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_bare_or_escaping_name_never_resolves() {
        assert!(diagnose_download_path("").is_none());
        assert!(diagnose_download_path("../evil").is_none());
        assert!(diagnose_download_path("a/b").is_none());
        assert!(diagnose_download_path(r"a\b").is_none());
        // Well-formed but nothing on disk by that name.
        assert!(diagnose_download_path("adi-report-nope.zip").is_none());
    }

    #[test]
    fn a_bad_action_body_is_a_400() {
        assert_eq!(run_system_power(b"not json").status, 400);
    }
}
