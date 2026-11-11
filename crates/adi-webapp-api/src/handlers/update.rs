//! The `/api/update` surface: the version this machine is on, whether a newer one is
//! published, and the one control that installs it. Three endpoints, in cost order — a
//! `GET` that reads two files, a `check` that fetches the release manifest, and a `run`
//! that hands the install to the CLI.
//!
//! **The install is deliberately not done in this process.** `adi-update`'s engine swaps
//! the binaries this very server is running from and then restarts the stack, so a handler
//! that called it inline would be killed part-way through its own reply and the page would
//! never learn how it went. The endpoint spawns the bundled `adi-mono update run` in its
//! own process group instead — the same way [`super::services`] spawns a project's server,
//! and for the same reason: it has to outlive the restart. The page then follows along by
//! polling, with `state.json` for the verdict and a marker file for the gap before it.
//!
//! That split is also what makes this work identically on a node: nothing here knows a DMG
//! from a tarball, or launchd from systemd from Task Scheduler. Every platform difference
//! is behind the CLI it shells out to.

use std::path::PathBuf;
use std::process::{Command, Stdio};

use adi_update::{Engine, Version, host_platform};
use serde::{Deserialize, Serialize};

use crate::types::UpdateState;

use super::response::{Response, error, ok_json};

/// The marker written when an install is handed to the CLI, beside the updater's own
/// `state.json`.
///
/// `state.json` cannot answer "is one running": the engine writes it once the swap is
/// *done*, which is minutes after the button was pressed and — since the app is restarted
/// onto the new binaries in between — on the far side of this process's own death. So the
/// panel records the child itself, and the next reader decides whether it is still going.
const MARKER: &str = "installing.json";

/// Where the spawned install's output goes. Truncated per run: it is read when an update
/// went wrong, and the run that went wrong is the last one.
const RUN_LOG: &str = "run.log";

/// How long a marker is believed at most. The updater gives the restarted stack 90s for its
/// health check, and a download over a slow link is the long pole before that; past half an
/// hour the run is gone in a way that left no other trace, and a pill stuck on "Updating…"
/// forever is worse than one that goes quiet.
const MARKER_MAX_SECS: u64 = 30 * 60;

/// The record of an install this panel started.
#[derive(Serialize, Deserialize)]
struct Installing {
    pid: u32,
    /// The child's start time, so a recycled pid doesn't read as an update still in flight.
    /// `0` when the platform wouldn't say — see [`adi_osext::process_start_millis`].
    started_millis: u64,
    started_unix: u64,
}

/// `GET /api/update` — the version pill's whole state, read from disk. No network, so the
/// page may poll it.
#[must_use]
pub fn update_state() -> Response {
    ok_json(&snapshot())
}

/// `POST /api/update/check` — fetch the release manifest and compare it against the
/// installed version, persisting the result. This is the one endpoint here that leaves the
/// machine, and the page fires it only when [`UpdateState::stale`] says the record is old.
///
/// A fetch failure is *not* a failed request: being offline is the ordinary case, and the
/// engine records it as `outcome: error` with the reason. Answering 200 with that recorded
/// keeps the pill showing the version it knows instead of blanking on a flaky network.
#[must_use]
pub fn check_update() -> Response {
    let _ = Engine::open().check();
    ok_json(&snapshot())
}

/// `POST /api/update/run` — install the published release: download, verify, swap, restart,
/// roll back if the stack doesn't come back.
///
/// Answers as soon as the CLI is running, not when it finishes — see the module header. The
/// page watches [`UpdateState::installing`] from there.
#[must_use]
pub fn run_update() -> Response {
    let engine = Engine::open();
    let module = engine.module();

    if installing(module).is_some() {
        return error(409, "an update is already installing on this machine");
    }

    let bin = mono_bin();
    if !bin.exists() {
        return error(
            500,
            &format!(
                "the bundled CLI is not where this build expects it ({}); \
                 set ADI_MONO_BIN to the adi-mono that should run the install",
                bin.display()
            ),
        );
    }

    let _ = module.ensure_dir();
    let mut cmd = Command::new(&bin);
    // `--quiet` is what the scheduled updater runs: an unreachable manifest is routine and
    // must not read as a failed job. Nobody is watching this exit status either.
    cmd.args(["update", "run", "--quiet"]).stdin(Stdio::null());
    // Its own process group, so it survives the restart it is about to perform on us: the
    // supervisor kills the app service's group, and a child inside that group would be torn
    // down between the swap and the health check — leaving the machine on unverified binaries
    // with nothing left running to roll them back.
    adi_osext::detach_process_group(&mut cmd);
    match std::fs::File::create(module.raw_path(RUN_LOG)) {
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

    let mut child = match cmd.spawn() {
        Ok(child) => child,
        Err(e) => return error(500, &format!("could not start the update: {e}")),
    };
    let pid = child.id();
    let marker = Installing {
        pid,
        started_millis: adi_osext::process_start_millis(pid).unwrap_or(0),
        started_unix: now_unix(),
    };
    if let Ok(bytes) = serde_json::to_vec(&marker) {
        let _ = module.write_raw(MARKER, &bytes);
    }
    // Reaped on a thread of its own. A successful install restarts the stack and orphans this
    // child to init anyway, but the ordinary case — offline, a bad manifest, anything that fails
    // before the swap — exits without ever doing that, and without a `wait()` it would sit as a
    // zombie for as long as this process runs.
    std::thread::spawn(move || {
        let _ = child.wait();
    });

    let mut state = snapshot();
    // Say so on this reply rather than making the page wait a poll to find out: the marker
    // was written a line ago, but a client that got `installing: false` back from the very
    // call that started one would flash the button back on.
    state.installing = true;
    ok_json(&state)
}

/// Everything the pill shows, from the updater's persisted record plus the installed version.
fn snapshot() -> UpdateState {
    let engine = Engine::open();
    let state = engine.state();
    let installed = Engine::installed_version();
    let stale_after = u64::from(engine.settings().check_interval_secs());

    // Recomputed rather than read from `last_outcome`: the record is written by whichever run
    // last checked, and an install (or a rollback) since then has moved `installed` out from
    // under that verdict without touching it.
    let update_available = state.latest_version.as_ref().is_some_and(|latest| {
        Version::is_newer(latest, &installed) && state.latest_has_artifact != Some(false)
    });

    let error_kind = state.last_error.as_deref().map(|e| {
        classify_error(
            e,
            engine.settings().auth_header.is_some(),
            is_licence_service(&engine.settings().manifest_url),
        )
        .to_string()
    });

    let now = now_unix();
    UpdateState {
        installed,
        running: adi_update::BUILT_VERSION.to_string(),
        latest: state.latest_version,
        update_available,
        platform: host_platform(),
        has_artifact: state.latest_has_artifact,
        notes: state.latest_notes,
        outcome: state.last_outcome,
        error: state.last_error,
        error_kind,
        checked_secs_ago: state.last_check_unix.map(|at| now.saturating_sub(at)),
        installed_secs_ago: state.last_install_unix.map(|at| now.saturating_sub(at)),
        stale: state
            .last_check_unix
            .is_none_or(|at| now.saturating_sub(at) >= stale_after),
        installing: installing(engine.module()).is_some(),
    }
}

/// The licence-service's own host (`docs/enterprise-update-channel.md` §3, §9): once a box's
/// `manifest_url` points here, a 401/402/403/404/409 is one of *this* host's licence states, not
/// a plain GitHub repository's bare "no credentials" 404 — see [`classify_error`].
const LICENCE_SERVICE_HOST: &str = "licence.withadi.dev";

/// Whether `manifest_url` is pointed at the licence-service host, by its host alone — the
/// licence service's two routes (§3) live at paths under it, but any path on that host answers
/// the same status table.
fn is_licence_service(manifest_url: &str) -> bool {
    manifest_url
        .split("://")
        .nth(1)
        .and_then(|rest| rest.split(['/', ':']).next())
        .is_some_and(|host| host == LICENCE_SERVICE_HOST)
}

/// A rough category for a persisted manifest-fetch error, for a panel that wants to say
/// something more specific than curl's own words. Matched off the message rather than a new
/// `adi_update::Error` variant threaded all the way through `state.json` — the file already
/// holds a human string, and the two things worth telling apart here both show up in it as
/// different wording:
///
/// - `curl -fsSL` reports any HTTP failure response as `(22)` with the status code quoted in
///   the text (`docs/adi-update.md` §8's manifest-fetch path), and that status digit is the
///   only signal that survives `-f`'s discarded body — which is exactly why the licence-service
///   (`docs/enterprise-update-channel.md` §5) assigns one status per licence state instead of
///   distinguishing them in a body nobody downstream ever reads.
/// - Pointed at the licence-service host, those codes are licence states: `401` no/garbage
///   credentials, `402` expired, `403` revoked, `404` a licence code the service has never
///   heard of, `409` this deployment isn't a seat holder. Pointed anywhere else, a 401/403/404
///   with no `auth_header` configured is the older, simpler case this used to be the only
///   reading of: a private channel nobody has given this machine a token for, which is the
///   Enterprise-install default until one is minted (or a manifest someone hand-reverted to
///   plain GitHub, per §5's note that `private-channel` is not retired).
/// - DNS, connection and timeout failures from the same curl are offline in the ordinary
///   sense — nothing answered, rather than something answering "no".
///
/// Anything else is reported through `error` as plain text instead of guessing at a category.
fn classify_error(err: &str, auth_header_set: bool, licence_service_host: bool) -> &'static str {
    if licence_service_host {
        if err.contains("401") {
            return "licence-unauthenticated";
        }
        if err.contains("402") {
            return "licence-expired";
        }
        if err.contains("403") {
            return "licence-revoked";
        }
        if err.contains("404") {
            return "licence-unknown";
        }
        if err.contains("409") {
            return "licence-seat-conflict";
        }
    } else {
        let looks_denied = err.contains("404") || err.contains("403") || err.contains("401");
        if looks_denied && !auth_header_set {
            return "private-channel";
        }
    }
    let looks_unreachable = err.contains("Could not resolve host")
        || err.contains("Couldn't connect")
        || err.contains("Connection refused")
        || err.contains("Operation timed out")
        || err.contains("SSL certificate problem");
    if looks_unreachable {
        return "offline";
    }
    "other"
}

/// The install still running, if there is one — clearing the marker when there isn't.
///
/// A marker whose process is gone is the *normal* end state: the CLI knows nothing about
/// this file, so nobody else will ever remove it. Reading is what collects it.
fn installing(module: &adi_config::Module) -> Option<Installing> {
    let marker: Installing = module
        .read_raw(MARKER)
        .ok()
        .flatten()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())?;

    // `pid_alive_as` closes the reuse hole; a marker written where the platform wouldn't
    // report a start time falls back to bare liveness rather than reading as finished the
    // moment it is written.
    let alive = if marker.started_millis == 0 {
        adi_osext::pid_alive(marker.pid)
    } else {
        adi_osext::pid_alive_as(marker.pid, marker.started_millis)
    };
    if alive && now_unix().saturating_sub(marker.started_unix) < MARKER_MAX_SECS {
        return Some(marker);
    }
    let _ = module.remove_raw(MARKER);
    None
}

/// The CLI to hand the install to: `$ADI_MONO_BIN`, else [`mono_beside_us`]. The same
/// resolution `adi-core` uses for the scheduled updater agent, so the button and the
/// background run reach the same binary.
///
/// `pub(crate)`: [`super::system`]'s power/restart mutations spawn the same CLI, detached, for
/// the same reason this one does (see that module's header) — one resolution rather than a
/// second copy that could point the two at different binaries.
pub(crate) fn mono_bin() -> PathBuf {
    match std::env::var_os("ADI_MONO_BIN").filter(|p| !p.is_empty()) {
        Some(path) => PathBuf::from(path),
        None => mono_beside_us(),
    }
}

/// `adi-mono` in the directory the running binary sits in — **not** whatever `PATH` finds.
/// An install must be performed by the CLI shipped with this build: in a macOS bundle both
/// sit in `Contents/Resources`, on a Linux or Windows node both sit in the node's binary
/// directory, and a stray older `adi-mono` earlier in `PATH` is exactly the copy that must
/// not be asked to swap the machine's install.
fn mono_beside_us() -> PathBuf {
    let name = format!("adi-mono{}", std::env::consts::EXE_SUFFIX);
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.join(&name)))
        .unwrap_or_else(|| PathBuf::from(name))
}

/// Seconds since the Unix epoch (0 if the clock is before it).
fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The real string this records on an Enterprise box with the manifest private and no
    /// `auth_header` set yet — the exact shape of ADI-34's known-good failure (see
    /// `~/.adi/mono/update/config.toml`'s own comment on that box). Not a licence-service host,
    /// so this must stay `private-channel` exactly as before (`docs/enterprise-update-channel.md`
    /// §5: "`private-channel` itself is not retired").
    #[test]
    fn a_404_with_no_auth_header_is_a_private_channel() {
        let err = "could not fetch the release manifest: curl: (22) The requested URL returned \
                    error: 404 (https://github.com/adi-family/mono-enterprise/releases/latest/\
                    download/manifest.json)";
        assert_eq!(classify_error(err, false, false), "private-channel");
    }

    /// The same response with a token configured is a *real* 404 — wrong URL, deleted release,
    /// anything but "no credentials" — and must not be reported as the credentials case.
    #[test]
    fn the_same_404_with_an_auth_header_set_is_not_a_private_channel() {
        let err = "could not fetch the release manifest: curl: (22) The requested URL returned \
                    error: 404 (…)";
        assert_eq!(classify_error(err, true, false), "other");
    }

    #[test]
    fn dns_and_timeout_failures_are_offline() {
        assert_eq!(
            classify_error("could not fetch the release manifest: Could not resolve host: github.com", false, false),
            "offline"
        );
        assert_eq!(
            classify_error("could not fetch the release manifest: Operation timed out after 30000 milliseconds", true, false),
            "offline"
        );
    }

    #[test]
    fn an_unrecognised_message_is_just_other() {
        assert_eq!(classify_error("the download failed its checksum", false, false), "other");
    }

    /// §5's table, one status per row, against the licence-service host — none of these may
    /// read as `private-channel` or `other`, however `auth_header_set` is passed, since a
    /// licence-service host always has one (§9's licence handler keeps it in sync every beam).
    #[test]
    fn licence_service_statuses_map_to_the_table_in_section_5() {
        let cases: &[(&str, &str)] = &[
            ("401", "licence-unauthenticated"),
            ("402", "licence-expired"),
            ("403", "licence-revoked"),
            ("404", "licence-unknown"),
            ("409", "licence-seat-conflict"),
        ];
        for (code, kind) in cases {
            let err = format!(
                "could not fetch the release manifest: curl: (22) The requested URL returned \
                 error: {code} (https://licence.withadi.dev/v1/updates/manifest?deployment_id=…)"
            );
            assert_eq!(classify_error(&err, true, true), *kind, "status {code}");
        }
    }

    /// The same five status codes against a non-licence-service host read through the old
    /// table instead — proving the branch is keyed on the host, not the status digit alone.
    #[test]
    fn the_same_statuses_off_the_licence_service_host_are_unaffected() {
        assert_eq!(classify_error("curl: (22) ... 401 ...", false, false), "private-channel");
        assert_eq!(classify_error("curl: (22) ... 401 ...", true, false), "other");
        assert_eq!(classify_error("curl: (22) ... 402 ...", true, false), "other");
        assert_eq!(classify_error("curl: (22) ... 409 ...", true, false), "other");
    }

    #[test]
    fn is_licence_service_matches_the_host_alone() {
        assert!(is_licence_service(
            "https://licence.withadi.dev/v1/updates/manifest?deployment_id=1"
        ));
        assert!(!is_licence_service(
            "https://github.com/adi-family/mono-enterprise/releases/latest/download/manifest.json"
        ));
        assert!(!is_licence_service("https://evil.example/licence.withadi.dev"));
        assert!(!is_licence_service("not a url"));
    }

    fn scratch(tag: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "adi-webapp-update-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id(),
        ))
    }

    /// A marker naming a process that is gone reads as "not installing" *and* is collected —
    /// nothing else ever deletes it, so a leak here is a pill stuck on "Updating…".
    #[test]
    fn a_dead_marker_is_cleared_by_reading_it() {
        let dir = scratch("dead");
        let _ = std::fs::remove_dir_all(&dir);
        let module = adi_config::Config::with_root(&dir).module("update");
        let _ = module.ensure_dir();

        // Pid 0 is never a live process (`adi_osext::pid_alive` reads it as dead by
        // construction), which is what makes it usable as "definitely finished" here.
        let marker = Installing {
            pid: 0,
            started_millis: 0,
            started_unix: now_unix(),
        };
        module
            .write_raw(MARKER, &serde_json::to_vec(&marker).expect("encode"))
            .expect("write");

        assert!(installing(&module).is_none());
        assert!(
            module.read_raw(MARKER).expect("read").is_none(),
            "the marker should have been collected"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A marker older than [`MARKER_MAX_SECS`] is abandoned even while its pid is alive: the
    /// run is long gone and the pid belongs to whatever was recycled onto it.
    #[test]
    fn a_stale_marker_is_abandoned_however_alive_its_pid_looks() {
        let dir = scratch("stale");
        let _ = std::fs::remove_dir_all(&dir);
        let module = adi_config::Config::with_root(&dir).module("update");
        let _ = module.ensure_dir();

        let me = std::process::id();
        let marker = Installing {
            pid: me,
            started_millis: 0,
            started_unix: now_unix().saturating_sub(MARKER_MAX_SECS + 1),
        };
        module
            .write_raw(MARKER, &serde_json::to_vec(&marker).expect("encode"))
            .expect("write");

        assert!(adi_osext::pid_alive(me), "this test's own process is alive");
        assert!(installing(&module).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The one thing that must never be wrong: the CLI is looked for beside the running
    /// binary, not on `PATH`, so a machine with an older `adi-mono` earlier in `PATH` cannot
    /// be asked to install with it.
    #[test]
    fn the_cli_is_resolved_beside_the_running_binary() {
        let bin = mono_beside_us();
        let exe = std::env::current_exe().expect("current exe");
        assert_eq!(bin.parent(), exe.parent());
        assert_eq!(
            bin.file_name().and_then(|n| n.to_str()),
            Some(format!("adi-mono{}", std::env::consts::EXE_SUFFIX).as_str())
        );
    }
}
