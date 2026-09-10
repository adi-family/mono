//! Supervises a bundled binary as a per-user background service.
//!
//! - **macOS:** a launchd `LaunchAgent` (`gui/$UID`, `RunAtLoad` + `KeepAlive`), driven through
//!   `launchctl`. Mirrors Swift's `Launchd`.
//! - **Linux:** a `systemd --user` unit (`~/.config/systemd/user/<label>.service`,
//!   `Restart=always`), driven through `systemctl --user`. A fleet node is headless, so
//!   [`enable`] also asks logind for *lingering*: without it the per-user manager — and every
//!   service under it — is torn down at logout (`docs/fleet.md` §6).
//! - **Windows:** a per-user **Task Scheduler** task (logon trigger + run-now, restart-on-failure),
//!   driven through `schtasks.exe`. A scheduled task is the unprivileged analog of a per-user
//!   LaunchAgent: it runs as the interactive user with no stored credentials and no admin rights.
//!
//! All three back-ends expose the same surface — [`enable`], [`enable_periodic`], [`disable`],
//! [`is_loaded`], [`kickstart`], plus a path helper for the file they install — so the
//! [`crate::service`] layer above never learns which OS it is on. The active implementation is
//! re-exported from the per-OS submodule below.
//!
//! The gating is per **`target_os`**, not `unix`. Linux *is* unix: while the macOS module carried
//! `#[cfg(unix)]` a Linux build compiled cleanly and then shelled out to a `/bin/launchctl` that
//! does not exist there, so a node could not be supervised at all.
//!
//! Everything renderable — plist XML, unit files, and the label→filename mapping — is a pure
//! function kept *outside* the platform modules and compiled under `cfg(test)` on every host. That
//! is what makes the Linux units testable from a macOS checkout, the same trick
//! `adi-dns/src/os_routing.rs` uses.

#[cfg(target_os = "linux")]
pub use linux::*;
#[cfg(target_os = "macos")]
pub use macos::*;
#[cfg(windows)]
pub use windows::*;

/// launchd plist rendering, re-exported from every unix build rather than from the macOS module.
///
/// [`crate::dns`] stages the root front-door plist behind `#[cfg(unix)]`; narrowing this renderer
/// to macOS would break the Linux build of a file that is not this module's to change. It is a
/// pure function, so being reachable on a host that has no launchd costs nothing.
#[cfg(any(unix, test))]
pub use plist::{plist_xml, plist_xml_periodic};

/// The current uid, cached; resolved via `id -u` to avoid an `unsafe` `getuid` call. Shared by the
/// launchd back-end (the `gui/$UID` domain) and the systemd one (`XDG_RUNTIME_DIR`).
#[cfg(unix)]
fn uid() -> u32 {
    use std::sync::OnceLock;
    static UID: OnceLock<u32> = OnceLock::new();
    *UID.get_or_init(|| {
        crate::proc::run(&["/usr/bin/id", "-u"])
            .text
            .trim()
            .parse()
            .unwrap_or(0)
    })
}

// ── macOS: launchd via launchctl ────────────────────────────────────────────────────────────
#[cfg(target_os = "macos")]
mod macos {
    use std::path::PathBuf;

    use super::uid;
    use super::{plist_xml, plist_xml_periodic};
    use crate::paths;
    use crate::proc;

    #[must_use]
    pub fn gui_domain() -> String {
        format!("gui/{}", uid())
    }

    /// The `gui/$UID/<label>` service target `launchctl` addresses.
    #[must_use]
    pub fn target(label: &str) -> String {
        format!("{}/{label}", gui_domain())
    }

    #[must_use]
    pub fn plist_path(label: &str) -> PathBuf {
        paths::launch_agents_dir().join(format!("{label}.plist"))
    }

    /// Install and start the `LaunchAgent`: write the plist, boot out any stale instance (so `bootstrap` can't dupe-fail), bootstrap, then enable.
    pub fn enable(label: &str, program: &[String], log: &str, env: &[(String, String)]) {
        install(label, &plist_xml(label, program, log, env));
    }

    /// Like [`enable`] but for a periodic one-shot job: runs at load and then every
    /// `interval_secs`, with no `KeepAlive` (the job exits between runs).
    pub fn enable_periodic(
        label: &str,
        program: &[String],
        log: &str,
        env: &[(String, String)],
        interval_secs: u32,
    ) {
        install(
            label,
            &plist_xml_periodic(label, program, log, env, interval_secs),
        );
    }

    fn install(label: &str, plist: &str) {
        let dir = paths::launch_agents_dir();
        let _ = std::fs::create_dir_all(&dir);
        let path = plist_path(label);
        let _ = std::fs::write(&path, plist);

        let target = target(label);
        let _ = proc::run(&["/bin/launchctl", "bootout", &target]);
        let boot = proc::run(&[
            "/bin/launchctl",
            "bootstrap",
            &gui_domain(),
            &path.to_string_lossy(),
        ]);
        if !boot.ok() {
            eprintln!(
                "adi: launchctl bootstrap {label} failed ({}): {}",
                boot.status, boot.text
            );
        }
        let _ = proc::run(&["/bin/launchctl", "enable", &target]);
    }

    /// Stop and uninstall the `LaunchAgent`.
    pub fn disable(label: &str) {
        let _ = proc::run(&["/bin/launchctl", "bootout", &target(label)]);
        let _ = std::fs::remove_file(plist_path(label));
    }

    /// Loaded == the plist exists and `launchctl print` can address the service.
    #[must_use]
    pub fn is_loaded(label: &str) -> bool {
        plist_path(label).exists() && proc::run(&["/bin/launchctl", "print", &target(label)]).ok()
    }

    /// Atomically kill-and-restart a loaded `LaunchAgent` so it picks up a replaced binary
    /// (`kickstart -k` — no bootout/bootstrap race). A no-op if the service isn't loaded.
    pub fn kickstart(label: &str) {
        let _ = proc::run(&["/bin/launchctl", "kickstart", "-k", &target(label)]);
    }
}

// ── launchd plist rendering (pure) ──────────────────────────────────────────────────────────
#[cfg(any(unix, test))]
mod plist {
    /// Identical XML for a per-user `LaunchAgent` and a root `LaunchDaemon`; only the install location differs.
    #[must_use]
    pub fn plist_xml(
        label: &str,
        program: &[String],
        log: &str,
        env: &[(String, String)],
    ) -> String {
        render_plist(
            label,
            program,
            log,
            env,
            "    <key>RunAtLoad</key>\n    <true/>\n    <key>KeepAlive</key>\n    <true/>",
        )
    }

    /// Plist for a periodic one-shot job: fires at load and every `interval_secs`; no `KeepAlive`.
    #[must_use]
    pub fn plist_xml_periodic(
        label: &str,
        program: &[String],
        log: &str,
        env: &[(String, String)],
        interval_secs: u32,
    ) -> String {
        let lifecycle = format!(
            "    <key>RunAtLoad</key>\n    <true/>\n    <key>StartInterval</key>\n    <integer>{interval_secs}</integer>"
        );
        render_plist(label, program, log, env, &lifecycle)
    }

    fn render_plist(
        label: &str,
        program: &[String],
        log: &str,
        env: &[(String, String)],
        lifecycle: &str,
    ) -> String {
        let args_xml = program
            .iter()
            .map(|a| format!("        <string>{}</string>", xml_escape(a)))
            .collect::<Vec<_>>()
            .join("\n");
        let env_xml = if env.is_empty() {
            String::new()
        } else {
            let entries = env
                .iter()
                .map(|(k, v)| {
                    format!(
                        "        <key>{}</key><string>{}</string>",
                        xml_escape(k),
                        xml_escape(v)
                    )
                })
                .collect::<Vec<_>>()
                .join("\n");
            format!("    <key>EnvironmentVariables</key>\n    <dict>\n{entries}\n    </dict>\n")
        };
        format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>{label}</string>
    <key>ProgramArguments</key>
    <array>
{args_xml}
    </array>
{env_xml}{lifecycle}
    <key>ProcessType</key>
    <string>Background</string>
    <key>StandardOutPath</key>
    <string>{log}</string>
    <key>StandardErrorPath</key>
    <string>{log}</string>
</dict>
</plist>"#,
            label = xml_escape(label),
            log = xml_escape(log),
        )
    }

    pub(super) fn xml_escape(s: &str) -> String {
        s.replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
    }
}

// ── systemd unit rendering + label→filename mapping (pure) ──────────────────────────────────
//
// Compiled on Linux and, under `cfg(test)`, everywhere — the whole point is that a macOS or
// Windows checkout still tests the units a node will run.
#[cfg(any(target_os = "linux", test))]
mod unit {
    use std::fmt::Write as _;

    /// Stem used when a label sanitizes to nothing. [`stem`] must be **total** — every label,
    /// including the empty one, has to name some file — and a caller that lands here has already
    /// lost, so the name only needs to be recognisable in `ls`.
    const FALLBACK_STEM: &str = "adi-unnamed";

    /// Longest stem we emit. Linux caps a filename at 255 bytes and systemd caps a unit name at
    /// the same, so this leaves ample room for the `.service`/`.timer` suffix. Truncation is safe
    /// on a char boundary because [`stem`] only ever emits ASCII.
    const MAX_STEM: usize = 200;

    /// The stem of the unit file for `label` — `family.adi.app.control-panel` passes through
    /// unchanged, which is the case that matters and the reason the mapping is not an escape
    /// scheme.
    ///
    /// A **total** label→filename mapping. systemd unit names admit only ASCII alphanumerics and
    /// `:-_.\`, and everything else has to be escaped; rather than systemd's reversible `\xNN`
    /// escaping we map any other character to `_`, because nothing ever recovers a label from a
    /// filename — the mapping only has to be total and confined to the unit directory.
    ///
    /// Confinement is the security property: `/` (and `\`, so a Windows-shaped label cannot smuggle
    /// one in) becomes `_`, so no label can name a file outside `~/.config/systemd/user`. `.` and
    /// `..` survive as ordinary characters, which is harmless — a suffix is always appended, so the
    /// result is never exactly `.` or `..` — but a *leading* dot is rewritten anyway, since a
    /// hidden unit file is only ever a way to confuse the operator looking for it.
    #[must_use]
    pub fn stem(label: &str) -> String {
        let mut stem: String = label
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_') {
                    c
                } else {
                    '_'
                }
            })
            .collect();
        let leading_dots = stem.chars().take_while(|c| *c == '.').count();
        stem.replace_range(..leading_dots, &"_".repeat(leading_dots));
        if stem.is_empty() {
            stem.push_str(FALLBACK_STEM);
        }
        stem.truncate(MAX_STEM);
        stem
    }

    /// The `.service` unit file name for `label`.
    #[must_use]
    pub fn service_name(label: &str) -> String {
        format!("{}.service", stem(label))
    }

    /// The `.timer` unit file name for `label` — only a periodic job has one.
    #[must_use]
    pub fn timer_name(label: &str) -> String {
        format!("{}.timer", stem(label))
    }

    /// A supervised daemon: the `KeepAlive` analog.
    ///
    /// `Restart=always` restarts on a clean exit too, which is what `KeepAlive` does. The backoff
    /// is a flat `RestartSec`; systemd only grew exponential backoff (`RestartSteps=`) in 254 and
    /// an unknown key on an older distro is a warning plus a silently different policy, so a fixed
    /// delay is the portable choice. `StartLimitIntervalSec=0` disables the *default* rate limit
    /// (5 starts in 10 s, after which systemd gives up for good) — launchd never gives up on a
    /// `KeepAlive` job, and a node whose services stay dead after one bad config edit is worse
    /// than one that keeps retrying.
    #[must_use]
    pub fn service_unit(
        label: &str,
        program: &[String],
        log: &str,
        env: &[(String, String)],
    ) -> String {
        render_service(
            label,
            program,
            log,
            env,
            "After=network-online.target\nWants=network-online.target\nStartLimitIntervalSec=0\n",
            "Type=simple\nRestart=always\nRestartSec=2\n",
            // `default.target` is the user manager's boot target — the `RunAtLoad` analog.
            Some("default.target"),
        )
    }

    /// The service half of a periodic job: a one-shot that exits, with no restart supervision
    /// (matching launchd's `StartInterval` plist, which drops `KeepAlive`) and **no `[Install]`** —
    /// the `.timer` beside it is what gets enabled, and a service enabled in its own right would
    /// also run once at every boot on top of the schedule.
    #[must_use]
    pub fn service_unit_periodic(
        label: &str,
        program: &[String],
        log: &str,
        env: &[(String, String)],
    ) -> String {
        render_service(label, program, log, env, "", "Type=oneshot\n", None)
    }

    /// The timer half of a periodic job. systemd has no `StartInterval`, so the interval lives in
    /// a second unit: `OnActiveSec` is the first run after the timer starts (i.e. after each boot,
    /// which is exactly how launchd counts too) and `OnUnitActiveSec` is every run after that.
    #[must_use]
    pub fn timer_unit(label: &str, interval_secs: u32) -> String {
        let name = service_name(label);
        format!(
            "{HEADER}\
             [Unit]\n\
             Description=ADI periodic job {desc} (timer)\n\
             \n\
             [Timer]\n\
             Unit={name}\n\
             OnActiveSec={interval_secs}\n\
             OnUnitActiveSec={interval_secs}\n\
             \n\
             [Install]\n\
             WantedBy=timers.target\n",
            desc = one_line(label),
        )
    }

    const HEADER: &str =
        "# Written by adi-core — regenerated on every enable; edits are overwritten.\n";

    fn render_service(
        label: &str,
        program: &[String],
        log: &str,
        env: &[(String, String)],
        unit_lines: &str,
        service_lines: &str,
        wanted_by: Option<&str>,
    ) -> String {
        let exec = program
            .iter()
            .map(|a| quote(a))
            .collect::<Vec<_>>()
            .join(" ");
        // A `systemd --user` unit inherits none of the login shell's PATH — it gets the manager's
        // bare default (`/usr/local/bin:/usr/bin:/bin:…`). Two things a node runs are spelled as
        // bare names and so resolve through it:
        //
        //   * `bun`, which every dashboard's runner invokes as `bun run …`;
        //   * `adi-mono`, which every generated tool shim in `tools/.bin/` `exec`s.
        //
        // Neither the package's own bin directory nor bun's default install location is on that
        // default, so on a node both failed with "not found" while the units themselves looked
        // healthy. The package dir is derived from the binary being supervised rather than
        // configured, since that binary already lives in it.
        let mut path_dirs: Vec<String> = Vec::new();
        if let Some(bin_dir) = program
            .first()
            .map(std::path::Path::new)
            .and_then(std::path::Path::parent)
            .filter(|d| !d.as_os_str().is_empty())
        {
            path_dirs.push(bin_dir.to_string_lossy().into_owned());
        }
        if let Some(home) = std::env::var_os("HOME") {
            path_dirs.push(
                std::path::Path::new(&home)
                    .join(".bun")
                    .join("bin")
                    .to_string_lossy()
                    .into_owned(),
            );
        }
        path_dirs.extend(
            [
                "/usr/local/bin",
                "/usr/bin",
                "/bin",
                "/usr/local/sbin",
                "/usr/sbin",
                "/sbin",
            ]
            .map(str::to_owned),
        );
        // The derived directory is often one of the standard ones (a package installed into
        // /usr/local/bin, say); keep first occurrence so the package's own dir still wins.
        let mut seen = std::collections::HashSet::new();
        path_dirs.retain(|d| seen.insert(d.clone()));

        // An explicit PATH from the caller wins: it is a deliberate override, and silently
        // prepending to it would make that override mean something other than what it says.
        let caller_sets_path = env.iter().any(|(k, _)| k == "PATH");
        let env_lines = env
            .iter()
            .map(|(k, v)| quote(&format!("{k}={v}")))
            .chain((!caller_sets_path).then(|| quote(&format!("PATH={}", path_dirs.join(":")))))
            .fold(String::new(), |mut lines, pair| {
                let _ = writeln!(lines, "Environment={pair}");
                lines
            });
        // launchd writes `StandardOutPath`/`StandardErrPath` to a file; `append:` is the systemd
        // spelling of the same thing, so the panel's log viewer and `adi logs` keep finding one
        // file per service instead of having to learn journalctl.
        let log_line = escape(log);
        let install =
            wanted_by.map_or_else(String::new, |t| format!("\n[Install]\nWantedBy={t}\n"));
        format!(
            "{HEADER}\
             [Unit]\n\
             Description=ADI service {desc}\n\
             {unit_lines}\
             \n\
             [Service]\n\
             {service_lines}\
             ExecStart={exec}\n\
             {env_lines}\
             StandardOutput=append:{log_line}\n\
             StandardError=append:{log_line}\n\
             SyslogIdentifier={desc}\n\
             {install}",
            desc = one_line(label),
        )
    }

    /// Escape a value for a unit-file directive: systemd expands `%` specifiers everywhere, and
    /// the unquoted C escapes `\` introduces would otherwise change the value. Newlines would end
    /// the directive outright, so they become the escape systemd unescapes back.
    fn escape(s: &str) -> String {
        s.replace('\\', "\\\\")
            .replace('%', "%%")
            .replace('\n', "\\n")
            .replace('\r', "\\r")
    }

    /// Escape and double-quote one word of a whitespace-split directive (`ExecStart=`,
    /// `Environment=`) so a path or value containing spaces stays a single word.
    fn quote(s: &str) -> String {
        format!("\"{}\"", escape(s).replace('"', "\\\""))
    }

    /// A label as a single-line free-text value (`Description=`, `SyslogIdentifier=`).
    ///
    /// Free text still has to be escaped: a newline would start a new directive outright, and a
    /// *trailing* backslash is a line continuation, which would swallow the directive on the next
    /// line rather than the value it belongs to.
    fn one_line(s: &str) -> String {
        escape(&s.replace(['\n', '\r'], " "))
    }
}

// ── Linux: systemd --user via systemctl ─────────────────────────────────────────────────────
#[cfg(target_os = "linux")]
mod linux {
    use std::path::{Path, PathBuf};

    use super::{uid, unit};
    use crate::paths;
    use crate::proc;

    /// `~/.config/systemd/user/<label>.service` — the unit file `systemctl --user` loads.
    #[must_use]
    pub fn unit_path(label: &str) -> PathBuf {
        paths::launch_agents_dir().join(unit::service_name(label))
    }

    /// `~/.config/systemd/user/<label>.timer` — written only for a periodic job.
    #[must_use]
    pub fn timer_path(label: &str) -> PathBuf {
        paths::launch_agents_dir().join(unit::timer_name(label))
    }

    /// Install and start a supervised daemon: write the unit, reload, enable it for boot, and
    /// (re)start it now.
    pub fn enable(label: &str, program: &[String], log: &str, env: &[(String, String)]) {
        ensure_linger();
        install(
            &unit_path(label),
            &unit::service_unit(label, program, log, env),
            log,
        );
        // A label that used to be periodic still has its timer on disk; left there it would keep
        // re-triggering the unit we just turned into a long-running service.
        if timer_path(label).exists() {
            let _ = systemctl(&["disable", "--now", &unit::timer_name(label)]);
            let _ = std::fs::remove_file(timer_path(label));
        }
        daemon_reload();
        let name = unit::service_name(label);
        report(label, &["enable", &name]);
        report(label, &["restart", &name]);
    }

    /// Like [`enable`] but for a periodic one-shot job: a `.timer` beside the `.service`, since
    /// systemd has no `StartInterval`.
    pub fn enable_periodic(
        label: &str,
        program: &[String],
        log: &str,
        env: &[(String, String)],
        interval_secs: u32,
    ) {
        ensure_linger();
        install(
            &unit_path(label),
            &unit::service_unit_periodic(label, program, log, env),
            log,
        );
        let _ = std::fs::write(timer_path(label), unit::timer_unit(label, interval_secs));
        daemon_reload();
        let timer = unit::timer_name(label);
        report(label, &["enable", &timer]);
        report(label, &["restart", &timer]);
        // launchd's `RunAtLoad` fires a periodic job the moment its plist loads; a timer only
        // starts counting. Run it once now for parity — `--no-block` because a one-shot job here
        // downloads an update, and `systemctl start` would otherwise wait for it.
        let _ = systemctl(&["start", "--no-block", &unit::service_name(label)]);
    }

    /// Write a unit file, creating both the unit directory and the log's directory —
    /// `StandardOutput=append:` creates the file but never its parent, and a missing parent fails
    /// the whole unit at start.
    fn install(path: &Path, body: &str, log: &str) {
        let _ = std::fs::create_dir_all(paths::launch_agents_dir());
        if let Some(dir) = Path::new(log).parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(path, body);
    }

    /// Stop and uninstall the unit (and its timer, if it had one).
    pub fn disable(label: &str) {
        if timer_path(label).exists() {
            let _ = systemctl(&["disable", "--now", &unit::timer_name(label)]);
        }
        let _ = systemctl(&["disable", "--now", &unit::service_name(label)]);
        let _ = std::fs::remove_file(unit_path(label));
        let _ = std::fs::remove_file(timer_path(label));
        daemon_reload();
    }

    /// Loaded == the unit file exists and systemd will bring it up on its own (it is *enabled*),
    /// the same question the launchd back-end answers with "the plist exists and `launchctl print`
    /// can address it" — installed and known to the supervisor, not necessarily running right now.
    /// For a periodic job the enabled unit is the timer, so that is what gets asked.
    #[must_use]
    pub fn is_loaded(label: &str) -> bool {
        if !unit_path(label).exists() {
            return false;
        }
        if timer_path(label).exists() {
            return systemctl(&["is-enabled", &unit::timer_name(label)]).ok();
        }
        systemctl(&["is-enabled", &unit::service_name(label)]).ok()
    }

    /// Restart a loaded unit so it picks up a replaced binary. A no-op if not installed.
    pub fn kickstart(label: &str) {
        if !is_loaded(label) {
            return;
        }
        let _ = systemctl(&["restart", &unit::service_name(label)]);
    }

    /// systemd caches unit files; one just written is invisible until this runs.
    fn daemon_reload() {
        let _ = systemctl(&["daemon-reload"]);
    }

    /// Run `systemctl --user <args>`.
    ///
    /// `XDG_RUNTIME_DIR` names the bus socket of the per-user manager. A login shell has it, but
    /// a non-interactive context does not — `ssh node adi up`, or a cron job — and without it
    /// `systemctl --user` fails with "Failed to connect to bus" no matter how healthy the manager
    /// is. Supplying the well-known path is strictly better than failing on an environment detail.
    fn systemctl(args: &[&str]) -> proc::Output {
        let mut invocation = vec!["systemctl", "--user"];
        invocation.extend_from_slice(args);
        if std::env::var_os("XDG_RUNTIME_DIR").is_some() {
            proc::run(&invocation)
        } else {
            proc::run_with_env(
                &[("XDG_RUNTIME_DIR", format!("/run/user/{}", uid()))],
                &invocation,
            )
        }
    }

    /// [`systemctl`], reporting a failure the way the launchd back-end reports a failed
    /// `bootstrap`: loudly on stderr, but never fatally — a supervisor that cannot be reached
    /// must not take the CLI down with it.
    fn report(label: &str, args: &[&str]) {
        let out = systemctl(args);
        if !out.ok() {
            eprintln!(
                "adi: systemctl --user {} failed for {label} ({}): {}",
                args.join(" "),
                out.status,
                out.text.trim()
            );
        }
    }

    /// Ask logind to keep this user's manager alive with no login session.
    ///
    /// Without lingering, the per-user systemd manager — and therefore every adi service — is torn
    /// down at the last logout. A fleet node is headless: the operator installs over ssh and logs
    /// out, so this is the difference between a node that survives the install and one that does
    /// not. The stock polkit rule (`org.freedesktop.login1.set-self-linger`) allows an *active*
    /// session to enable it for its own user with no password, which covers the normal ssh
    /// install; when it is refused we print the exact command, because a silent failure here
    /// looks later like "the node dies whenever nobody is logged in".
    fn ensure_linger() {
        let user = proc::run(&["id", "-un"]).text.trim().to_string();
        if user.is_empty() {
            return;
        }
        let shown = proc::run(&[
            "loginctl",
            "show-user",
            &user,
            "--property=Linger",
            "--value",
        ]);
        // Tolerates both the `--value` output (`yes`) and the older `Linger=yes`; anything else,
        // including the error from a user with no logind entry, falls through to the attempt.
        if shown.text.trim().ends_with("yes") {
            return;
        }
        let out = proc::run(&["loginctl", "enable-linger", &user]);
        if !out.ok() {
            eprintln!(
                "adi: could not enable lingering for {user} ({}): {}",
                out.status,
                out.text.trim()
            );
            eprintln!(
                "adi: run `sudo loginctl enable-linger {user}` — without it every adi service \
                 stops when {user} logs out"
            );
        }
    }
}

// ── Windows: Task Scheduler via schtasks.exe ────────────────────────────────────────────────
#[cfg(windows)]
mod windows {
    use std::path::PathBuf;

    use crate::paths;
    use crate::proc;

    /// Path of the task-definition XML `schtasks /Create /XML` imports for `label`.
    #[must_use]
    pub fn task_xml_path(label: &str) -> PathBuf {
        paths::launch_agents_dir().join(format!("{label}.xml"))
    }

    /// Install and start a long-running, auto-restarting task (the `KeepAlive` analog).
    pub fn enable(label: &str, program: &[String], log: &str, env: &[(String, String)]) {
        ensure_log_dir(log);
        install(label, &task_xml(label, program, log, env, None));
    }

    /// Install a periodic task: fires at logon, then repeats every `interval_secs`; the process
    /// exits between runs, so there is no restart-on-failure supervision (matches launchd's
    /// `StartInterval` job with no `KeepAlive`).
    pub fn enable_periodic(
        label: &str,
        program: &[String],
        log: &str,
        env: &[(String, String)],
        interval_secs: u32,
    ) {
        ensure_log_dir(log);
        install(
            label,
            &task_xml(label, program, log, env, Some(interval_secs)),
        );
    }

    /// The task action is `cmd /C ... > <log>`, and cmd's redirection fails outright when the
    /// log's directory is missing -- the task then dies the instant it starts, with no output
    /// anywhere to say why. Nothing else creates this directory on a fresh install.
    fn ensure_log_dir(log: &str) {
        if let Some(parent) = std::path::Path::new(log).parent() {
            let _ = std::fs::create_dir_all(parent);
        }
    }

    fn install(label: &str, xml: &str) {
        let dir = paths::launch_agents_dir();
        let _ = std::fs::create_dir_all(&dir);
        let path = task_xml_path(label);
        // Task Scheduler is happiest with UTF-16LE; write a BOM + UTF-16 so `schtasks /XML`
        // parses non-ASCII (paths, env values) correctly on any locale.
        let _ = std::fs::write(&path, utf16le_with_bom(xml));

        // Replace any stale registration, then start it now (the logon trigger covers future
        // sessions; `/Run` is the `RunAtLoad`-now analog).
        let create = proc::run(&[
            "schtasks",
            "/Create",
            "/TN",
            label,
            "/XML",
            &path.to_string_lossy(),
            "/F",
        ]);
        if !create.ok() {
            eprintln!(
                "adi: schtasks /Create {label} failed ({}): {}",
                create.status, create.text
            );
        }
        let _ = proc::run(&["schtasks", "/Run", "/TN", label]);
    }

    /// Stop and unregister the task.
    pub fn disable(label: &str) {
        let _ = proc::run(&["schtasks", "/End", "/TN", label]);
        let _ = proc::run(&["schtasks", "/Delete", "/TN", label, "/F"]);
        let _ = std::fs::remove_file(task_xml_path(label));
    }

    /// Loaded == the task is registered (`schtasks /Query` addresses it).
    #[must_use]
    pub fn is_loaded(label: &str) -> bool {
        proc::run(&["schtasks", "/Query", "/TN", label]).ok()
    }

    /// Stop-and-restart the task so it picks up a replaced binary. A no-op if not registered.
    pub fn kickstart(label: &str) {
        if !is_loaded(label) {
            return;
        }
        let _ = proc::run(&["schtasks", "/End", "/TN", label]);
        let _ = proc::run(&["schtasks", "/Run", "/TN", label]);
    }

    /// Build a Task Scheduler 1.2 task definition. `repeat_secs = None` ⇒ a long-running service
    /// (logon trigger + restart-on-failure); `Some(n)` ⇒ a job that repeats every `n` seconds.
    #[must_use]
    fn task_xml(
        label: &str,
        program: &[String],
        log: &str,
        env: &[(String, String)],
        repeat_secs: Option<u32>,
    ) -> String {
        let (comspec, comspec_args) = action(launcher_path().as_deref(), program, log, env);

        let user = user_id_element(current_user_id().as_deref());

        let repetition = repeat_secs.map_or(String::new(), |secs| {
            format!(
                "\n      <Repetition>\n        <Interval>{}</Interval>\n        <StopAtDurationEnd>false</StopAtDurationEnd>\n      </Repetition>",
                iso8601_duration(secs)
            )
        });
        // A long-running service restarts on failure; a periodic job does not (it is meant to exit).
        let restart = if repeat_secs.is_none() {
            "\n    <RestartOnFailure>\n      <Interval>PT1M</Interval>\n      <Count>999</Count>\n    </RestartOnFailure>"
        } else {
            ""
        };

        format!(
            r#"<?xml version="1.0" encoding="UTF-16"?>
<Task version="1.2" xmlns="http://schemas.microsoft.com/windows/2004/02/mit/task">
  <RegistrationInfo>
    <Description>ADI service {desc}</Description>
  </RegistrationInfo>
  <Triggers>
    <LogonTrigger>{user}
      <Enabled>true</Enabled>{repetition}
    </LogonTrigger>
  </Triggers>
  <Principals>
    <Principal id="Author">{user}
      <LogonType>InteractiveToken</LogonType>
      <RunLevel>LeastPrivilege</RunLevel>
    </Principal>
  </Principals>
  <Settings>
    <MultipleInstancesPolicy>IgnoreNew</MultipleInstancesPolicy>
    <DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries>
    <StopIfGoingOnBatteries>false</StopIfGoingOnBatteries>
    <AllowHardTerminate>true</AllowHardTerminate>
    <StartWhenAvailable>true</StartWhenAvailable>
    <RunOnlyIfNetworkAvailable>false</RunOnlyIfNetworkAvailable>
    <IdleSettings>
      <StopOnIdleEnd>false</StopOnIdleEnd>
      <RestartOnIdle>false</RestartOnIdle>
    </IdleSettings>
    <AllowStartOnDemand>true</AllowStartOnDemand>
    <Enabled>true</Enabled>
    <Hidden>false</Hidden>
    <RunOnlyIfIdle>false</RunOnlyIfIdle>
    <WakeToRun>false</WakeToRun>
    <ExecutionTimeLimit>PT0S</ExecutionTimeLimit>
    <Priority>7</Priority>{restart}
  </Settings>
  <Actions Context="Author">
    <Exec>
      <Command>{comspec}</Command>
      <Arguments>{args}</Arguments>
    </Exec>
  </Actions>
</Task>"#,
            desc = xml_escape(label),
            comspec = xml_escape(&comspec),
            args = xml_escape(&comspec_args),
        )
    }

    /// The command line a task runs, as `(program, arguments)`.
    ///
    /// **`ADI.exe --supervise` whenever the launcher is installed beside us**, which is every
    /// packaged install. It is a GUI-subsystem binary, so the service gets no console window —
    /// and a console window is not cosmetic on Windows: closing one kills what is attached to
    /// it, which is how people stopped the platform by tidying their desktop. It also holds the
    /// service in a job object, so ending the task ends the service instead of orphaning it.
    ///
    /// The `cmd /C set VAR=… && prog > log` wrapper is the fallback for a tree that has no
    /// ADI.exe — a cargo checkout running `adi-mono up` — where a console is the least of it.
    fn action(
        launcher: Option<&str>,
        program: &[String],
        log: &str,
        env: &[(String, String)],
    ) -> (String, String) {
        if let Some(launcher) = launcher {
            let launcher = launcher.to_string();
            let mut args = format!("--supervise --log {}", quote_argv(log));
            for (k, v) in env {
                args.push_str(&format!(" --env {}", quote_argv(&format!("{k}={v}"))));
            }
            args.push_str(" --");
            for part in program {
                args.push(' ');
                args.push_str(&quote_argv(part));
            }
            return (launcher, args);
        }

        let (command, arguments) = split_program(program);
        let mut inner = String::new();
        for (k, v) in env {
            inner.push_str(&format!("set \"{}={}\" && ", cmd_escape(k), cmd_escape(v)));
        }
        inner.push_str(&quote_cmd(&command));
        if !arguments.is_empty() {
            inner.push(' ');
            inner.push_str(&arguments);
        }
        inner.push_str(&format!(" > {} 2>&1", quote_cmd(log)));
        ("cmd.exe".to_string(), format!("/C {inner}"))
    }

    /// The app's own file name, which `apps/windows/build.sh` renames the launcher binary to.
    const LAUNCHER_EXE: &str = "ADI.exe";

    /// `ADI.exe` beside this executable, if it is there. The package puts every binary in one
    /// `bin\` directory, so whichever of them is registering the task is a sibling of it.
    fn launcher_path() -> Option<String> {
        let path = std::env::current_exe().ok()?.parent()?.join(LAUNCHER_EXE);
        path.is_file().then(|| path.to_string_lossy().into_owned())
    }

    /// Quote one argument the way `CommandLineToArgvW` reads it — which is how Task Scheduler
    /// hands `<Arguments>` to the program, and how Rust's own `std::env::args` parses them back
    /// on the other side. Not the same rules as [`quote_cmd`]: there a quote is doubled, here a
    /// quote is backslash-escaped and the backslashes before it are themselves doubled.
    fn quote_argv(s: &str) -> String {
        if !s.is_empty() && !s.contains([' ', '\t', '"']) {
            return s.to_string();
        }
        let mut out = String::from("\"");
        let mut backslashes = 0;
        for ch in s.chars() {
            match ch {
                '\\' => {
                    backslashes += 1;
                    out.push('\\');
                }
                '"' => {
                    // The run of backslashes before a quote is doubled, then the quote escaped.
                    for _ in 0..=backslashes {
                        out.push('\\');
                    }
                    backslashes = 0;
                    out.push('"');
                }
                _ => {
                    backslashes = 0;
                    out.push(ch);
                }
            }
        }
        // A trailing backslash run would escape the closing quote if left alone.
        for _ in 0..backslashes {
            out.push('\\');
        }
        out.push('"');
        out
    }

    /// Who the task runs as, as Task Scheduler spells it: `DOMAIN\user`, or a bare user name
    /// when the machine reports no domain.
    fn current_user_id() -> Option<String> {
        let user = std::env::var("USERNAME").ok().filter(|u| !u.is_empty())?;
        match std::env::var("USERDOMAIN") {
            Ok(domain) if !domain.is_empty() => Some(format!("{domain}\\{user}")),
            _ => Some(user),
        }
    }

    /// The `<UserId>` line shared by the trigger and the principal.
    ///
    /// Load-bearing, not decoration: a `<LogonTrigger>` carrying no `<UserId>` means "at logon
    /// of *any* user", and registering that is an administrator-only act -- `schtasks /Create`
    /// answers "ERROR: Access is denied." for an ordinary user, so on a stock Windows account
    /// not one adi service can be registered and `adi up` brings up nothing. Naming the user
    /// makes the byte-identical registration succeed unelevated.
    fn user_id_element(user: Option<&str>) -> String {
        user.map_or(String::new(), |u| {
            format!("\n      <UserId>{}</UserId>", xml_escape(u))
        })
    }

    /// Split `[program, arg, ...]` into the command and a single quoted arguments string.
    fn split_program(program: &[String]) -> (String, String) {
        match program.split_first() {
            Some((cmd, rest)) => (cmd.clone(), join_args(rest)),
            None => (String::new(), String::new()),
        }
    }

    fn join_args(args: &[String]) -> String {
        args.iter()
            .map(|a| quote_cmd(a))
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// Quote an argument for a `cmd /C` command line: wrap in double quotes if it contains a
    /// space or is empty, doubling any embedded quote.
    fn quote_cmd(s: &str) -> String {
        if s.is_empty() || s.contains([' ', '\t']) {
            format!("\"{}\"", s.replace('"', "\"\""))
        } else {
            s.to_string()
        }
    }

    /// Escape a value destined for `cmd`'s `set VAR=value` (the risky metacharacters inside a
    /// double-quoted `set`).
    fn cmd_escape(s: &str) -> String {
        s.replace('%', "%%").replace('"', "\"\"")
    }

    fn xml_escape(s: &str) -> String {
        s.replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
            .replace('"', "&quot;")
    }

    /// Seconds → an ISO-8601 duration (`PT6H`, `PT90S`, …) for a task `<Interval>`.
    fn iso8601_duration(mut secs: u32) -> String {
        let h = secs / 3600;
        secs %= 3600;
        let m = secs / 60;
        let s = secs % 60;
        let mut out = String::from("PT");
        if h > 0 {
            out.push_str(&format!("{h}H"));
        }
        if m > 0 {
            out.push_str(&format!("{m}M"));
        }
        if s > 0 || (h == 0 && m == 0) {
            out.push_str(&format!("{s}S"));
        }
        out
    }

    fn utf16le_with_bom(s: &str) -> Vec<u8> {
        let mut bytes = vec![0xFF, 0xFE];
        for unit in s.encode_utf16() {
            bytes.extend_from_slice(&unit.to_le_bytes());
        }
        bytes
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn service_task_has_logon_trigger_and_restart() {
            let xml = task_xml(
                "family.adi.app.dns",
                &[
                    "C:\\adi\\adi-dns.exe".to_string(),
                    "C:\\cfg.toml".to_string(),
                ],
                "C:\\log.txt",
                &[("RUST_LOG".to_string(), "info".to_string())],
                None,
            );
            assert!(xml.contains("<LogonTrigger>"));
            assert!(xml.contains("<RestartOnFailure>"));
            // Both the trigger and the principal must name the user, or registering the task
            // needs administrator rights and a normal install can start nothing.
            if current_user_id().is_some() {
                assert_eq!(xml.matches("<UserId>").count(), 2);
            }
            assert!(xml.contains("family.adi.app.dns"));
            // Program, its arg, the log redirect, and the env var all ride in the cmd wrapper.
            assert!(xml.contains("adi-dns.exe"));
            // The cmd wrapper, because a test binary has no ADI.exe beside it — see
            // `a_packaged_install_runs_the_service_under_the_launcher` for what ships.
            assert!(xml.contains("set &quot;RUST_LOG=info&quot;"));
            assert!(xml.contains("2&gt;&amp;1"));
        }

        #[test]
        fn periodic_task_repeats_and_does_not_restart() {
            let xml = task_xml(
                "family.adi.app.updater",
                &["C:\\adi\\adi-mono.exe".to_string(), "update".to_string()],
                "C:\\log.txt",
                &[],
                Some(21600),
            );
            assert!(xml.contains("<Repetition>"));
            assert!(xml.contains("<Interval>PT6H</Interval>"));
            assert!(!xml.contains("RestartOnFailure"));
        }

        fn dns_program() -> Vec<String> {
            vec![
                r"C:\Program Files\ADI\bin\adi-dns.exe".to_string(),
                r"C:\cfg.toml".to_string(),
            ]
        }

        #[test]
        fn a_packaged_install_runs_the_service_under_the_launcher() {
            // No console window and no orphan: ADI.exe is a GUI-subsystem binary that holds the
            // service in a job object. Everything cmd used to carry is an argument now.
            let (command, args) = action(
                Some(r"C:\Program Files\ADI\bin\ADI.exe"),
                &dns_program(),
                r"C:\Users\adi\.adi\mono\logs\adi-dns.log",
                &[("RUST_LOG".to_string(), "info".to_string())],
            );
            assert_eq!(command, r"C:\Program Files\ADI\bin\ADI.exe");
            assert_eq!(
                args,
                concat!(
                    r#"--supervise --log C:\Users\adi\.adi\mono\logs\adi-dns.log "#,
                    r#"--env RUST_LOG=info "#,
                    r#"-- "C:\Program Files\ADI\bin\adi-dns.exe" C:\cfg.toml"#
                )
            );
        }

        #[test]
        fn a_checkout_with_no_launcher_falls_back_to_the_cmd_wrapper() {
            let (command, args) = action(
                None,
                &dns_program(),
                r"C:\log.txt",
                &[("RUST_LOG".to_string(), "info".to_string())],
            );
            assert_eq!(command, "cmd.exe");
            assert!(args.starts_with(r#"/C set "RUST_LOG=info" && "#));
            assert!(args.ends_with(r"> C:\log.txt 2>&1"));
        }

        #[test]
        fn argv_quoting_follows_commandlinetoargvw() {
            assert_eq!(quote_argv("plain"), "plain");
            assert_eq!(quote_argv(r"C:\adi\bin\adi-app.exe"), r"C:\adi\bin\adi-app.exe");
            assert_eq!(
                quote_argv(r"C:\Program Files\ADI\bin\adi-app.exe"),
                "\"C:\\Program Files\\ADI\\bin\\adi-app.exe\""
            );
            // A trailing backslash must not escape the closing quote.
            assert_eq!(quote_argv(r"C:\Program Files\ADI\"), "\"C:\\Program Files\\ADI\\\\\"");
            assert_eq!(quote_argv(r#"say "hi""#), r#""say \"hi\"""#);
            assert_eq!(quote_argv(""), "\"\"");
        }

        #[test]
        fn user_id_element_is_named_or_absent() {
            assert_eq!(
                user_id_element(Some("ADI-WIN-TEST\\adi")),
                "\n      <UserId>ADI-WIN-TEST\\adi</UserId>"
            );
            assert_eq!(user_id_element(None), "");
        }

        #[test]
        fn iso8601_formats_durations() {
            assert_eq!(iso8601_duration(21600), "PT6H");
            assert_eq!(iso8601_duration(90), "PT1M30S");
            assert_eq!(iso8601_duration(45), "PT45S");
            assert_eq!(iso8601_duration(0), "PT0S");
        }

        #[test]
        fn utf16_output_starts_with_bom() {
            let bytes = utf16le_with_bom("A");
            assert_eq!(&bytes[..2], &[0xFF, 0xFE]);
            assert_eq!(&bytes[2..], &[0x41, 0x00]);
        }
    }
}

// ── Tests for the pure renderers — they run on every host, including the two that will never
//    execute the back-end they cover ───────────────────────────────────────────────────────────
#[cfg(test)]
mod tests {
    use super::{plist, unit};

    fn env(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect()
    }

    fn argv(args: &[&str]) -> Vec<String> {
        args.iter().map(|a| (*a).to_string()).collect()
    }

    // MARK: launchd plists (macOS)

    #[test]
    fn plist_contains_label_and_program() {
        let xml = plist::plist_xml(
            "family.adi.app.dns",
            &argv(&["/opt/adi-dns", "/cfg.toml"]),
            "/tmp/log",
            &env(&[("RUST_LOG", "info")]),
        );
        assert!(xml.contains("<string>family.adi.app.dns</string>"));
        assert!(xml.contains("<string>/opt/adi-dns</string>"));
        assert!(xml.contains("<string>/cfg.toml</string>"));
        assert!(xml.contains("<key>RUST_LOG</key><string>info</string>"));
        assert!(xml.contains("<key>KeepAlive</key>"));
    }

    #[test]
    fn plist_omits_env_dict_when_empty() {
        let xml = plist::plist_xml("l", &argv(&["/bin/x"]), "/tmp/log", &[]);
        assert!(!xml.contains("EnvironmentVariables"));
    }

    #[test]
    fn periodic_plist_swaps_keepalive_for_a_start_interval() {
        let xml = plist::plist_xml_periodic(
            "family.adi.app.updater",
            &argv(&["/opt/adi-mono", "update"]),
            "/tmp/log",
            &[],
            21600,
        );
        assert!(xml.contains("<key>StartInterval</key>"));
        assert!(xml.contains("<integer>21600</integer>"));
        assert!(xml.contains("<key>RunAtLoad</key>"));
        assert!(!xml.contains("KeepAlive"));
    }

    #[test]
    fn xml_escapes_markup() {
        assert_eq!(
            plist::xml_escape("a & b < c > d"),
            "a &amp; b &lt; c &gt; d"
        );
    }

    // MARK: systemd units (Linux)

    /// A unit file is a flat list of `Key=value` lines under `[Section]` headers. Anything else
    /// on a line means a value escaped its directive, which is the failure mode every escaping
    /// rule below exists to prevent — so assert the shape, not just the presence of substrings.
    fn assert_well_formed(text: &str) {
        for line in text.lines() {
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            assert!(
                (line.starts_with('[') && line.ends_with(']')) || line.contains('='),
                "stray line in unit file: {line:?}\n---\n{text}"
            );
        }
    }

    fn daemon_unit() -> String {
        unit::service_unit(
            "family.adi.app.dns",
            &argv(&["/opt/adi/adi-dns", "/home/n/.adi/mono/dns/adi-dns.toml"]),
            "/home/n/.adi/mono/logs/adi-dns.log",
            &env(&[("RUST_LOG", "info")]),
        )
    }

    #[test]
    fn service_unit_carries_program_env_and_log() {
        let u = daemon_unit();
        assert_well_formed(&u);
        assert!(u.contains("[Unit]") && u.contains("[Service]") && u.contains("[Install]"));
        assert!(u.contains("Description=ADI service family.adi.app.dns"));
        assert!(
            u.contains("ExecStart=\"/opt/adi/adi-dns\" \"/home/n/.adi/mono/dns/adi-dns.toml\"\n"),
            "got: {u}"
        );
        assert!(u.contains("Environment=\"RUST_LOG=info\"\n"));
        // Same log file the rest of adi-core computes, so the panel's log view keeps working.
        assert!(u.contains("StandardOutput=append:/home/n/.adi/mono/logs/adi-dns.log\n"));
        assert!(u.contains("StandardError=append:/home/n/.adi/mono/logs/adi-dns.log\n"));
        assert!(u.contains("WantedBy=default.target"));
    }

    #[test]
    fn service_unit_restarts_forever_with_a_backoff() {
        let u = daemon_unit();
        assert!(u.contains("Restart=always\n"), "got: {u}");
        assert!(u.contains("RestartSec=2\n"), "got: {u}");
        // The KeepAlive analog: systemd's default start limit would give up after five restarts.
        assert!(u.contains("StartLimitIntervalSec=0\n"), "got: {u}");
    }

    #[test]
    fn service_unit_omits_caller_environment_lines_when_there_is_no_env() {
        let u = unit::service_unit("l", &argv(&["/bin/x"]), "/tmp/log", &[]);
        // One line remains no matter what: the PATH every unit carries. Nothing from the caller.
        assert_eq!(u.matches("Environment=").count(), 1, "got: {u}");
        assert!(u.contains("Environment=\"PATH="), "got: {u}");
        assert!(!u.contains("RUST_LOG"), "got: {u}");
    }

    /// A `systemd --user` unit inherits the manager's bare PATH, and a node runs two things by
    /// bare name through it: `bun` (every dashboard runner) and `adi-mono` (every `tools/.bin`
    /// shim). Both live outside the default, so the unit has to carry them or a node supervises
    /// services that cannot find the programs they are made of.
    #[test]
    fn service_unit_path_covers_the_package_dir_and_bun() {
        let u = unit::service_unit(
            "family.adi.app.dashboards",
            &argv(&[
                "/home/adi/.local/adi/bin/adi-hive",
                "/home/adi/.adi/mono/x.yaml",
            ]),
            "/tmp/log",
            &[],
        );
        let path = u
            .lines()
            .find(|l| l.starts_with("Environment=\"PATH="))
            .unwrap_or_else(|| panic!("no PATH line: {u}"));
        // The package's own bin dir, derived from the supervised binary, and ahead of the rest.
        assert!(
            path.starts_with("Environment=\"PATH=/home/adi/.local/adi/bin:"),
            "got: {path}"
        );
        assert!(path.contains("/.bun/bin:"), "got: {path}");
        assert!(path.contains("/usr/bin"), "got: {path}");
        // Deduped: the standard list must not repeat a directory already contributed above.
        let dirs: Vec<&str> = path
            .trim_start_matches("Environment=\"PATH=")
            .trim_end_matches('"')
            .split(':')
            .collect();
        let mut unique = dirs.clone();
        unique.sort_unstable();
        unique.dedup();
        assert_eq!(unique.len(), dirs.len(), "duplicate dirs in {path}");
    }

    /// An explicit PATH is an override, not a suggestion — prepending to it silently would make
    /// it mean something other than what the caller wrote.
    #[test]
    fn an_explicit_path_from_the_caller_is_left_alone() {
        let u = unit::service_unit(
            "l",
            &argv(&["/opt/adi/adi-dns"]),
            "/tmp/log",
            &[("PATH".to_string(), "/only/this".to_string())],
        );
        assert!(u.contains("Environment=\"PATH=/only/this\"\n"), "got: {u}");
        assert_eq!(u.matches("Environment=").count(), 1, "got: {u}");
    }

    #[test]
    fn periodic_service_is_a_oneshot_that_the_timer_owns() {
        let u = unit::service_unit_periodic(
            "family.adi.app.updater",
            &argv(&["/opt/adi/adi-mono", "update", "run", "--quiet"]),
            "/tmp/log",
            &[],
        );
        assert_well_formed(&u);
        assert!(u.contains("Type=oneshot\n"), "got: {u}");
        // A job meant to exit must not be restarted, and must not be enabled in its own right —
        // an [Install] section would run it once per boot on top of the schedule.
        assert!(!u.contains("Restart="), "got: {u}");
        assert!(!u.contains("[Install]"), "got: {u}");
        assert!(u.contains("ExecStart=\"/opt/adi/adi-mono\" \"update\" \"run\" \"--quiet\"\n"));
    }

    #[test]
    fn timer_unit_drives_its_service_on_the_interval() {
        let t = unit::timer_unit("family.adi.app.updater", 21600);
        assert_well_formed(&t);
        assert!(t.contains("[Timer]"));
        assert!(
            t.contains("Unit=family.adi.app.updater.service\n"),
            "got: {t}"
        );
        // First run after the timer starts, then every interval — systemd has no StartInterval.
        assert!(t.contains("OnActiveSec=21600\n"), "got: {t}");
        assert!(t.contains("OnUnitActiveSec=21600\n"), "got: {t}");
        assert!(t.contains("WantedBy=timers.target"), "got: {t}");
    }

    // MARK: label → filename, a total function confined to the unit directory

    #[test]
    fn ordinary_labels_map_to_their_own_name() {
        assert_eq!(
            unit::service_name("family.adi.app.control-panel"),
            "family.adi.app.control-panel.service"
        );
        assert_eq!(
            unit::timer_name("family.adi.app.updater"),
            "family.adi.app.updater.timer"
        );
    }

    #[test]
    fn hostile_labels_cannot_escape_the_unit_directory() {
        for label in [
            "../../etc/systemd/user/sshd",
            "..",
            ".",
            "a/b",
            "..\\..\\evil",
            "/etc/passwd",
            "sudo rm -rf /",
            "nul\0byte",
            "юникод",
            "tab\there",
        ] {
            let name = unit::service_name(label);
            assert!(!name.contains('/'), "{label:?} → {name:?}");
            assert!(!name.contains('\\'), "{label:?} → {name:?}");
            assert!(
                !name.contains(std::path::MAIN_SEPARATOR),
                "{label:?} → {name:?}"
            );
            assert!(!name.starts_with('.'), "{label:?} → {name:?}");
            assert!(name.ends_with(".service"), "{label:?} → {name:?}");
            assert!(name.is_ascii(), "{label:?} → {name:?}");
            // Joining onto the unit directory must stay inside it: one component, and not `..`.
            let joined = std::path::Path::new("/u").join(&name);
            assert_eq!(joined.parent(), Some(std::path::Path::new("/u")));
        }
    }

    #[test]
    fn the_empty_label_still_names_a_file() {
        assert_eq!(unit::service_name(""), "adi-unnamed.service");
        assert_eq!(unit::timer_name(""), "adi-unnamed.timer");
    }

    #[test]
    fn a_very_long_label_is_truncated_to_a_legal_filename() {
        let name = unit::service_name(&"x".repeat(4096));
        assert!(name.len() < 255, "len {}", name.len());
        assert!(name.ends_with(".service"));
    }

    // MARK: escaping — a value must never become a directive

    #[test]
    fn env_values_escape_specifiers_quotes_and_backslashes() {
        let u = unit::service_unit(
            "l",
            &argv(&["/bin/x"]),
            "/tmp/log",
            &env(&[
                ("PROMPT", "100%"),
                ("QUOTED", "a\"b"),
                ("WIN", "C:\\adi"),
                ("MULTI", "one\ntwo"),
            ]),
        );
        assert_well_formed(&u);
        // `%` is a systemd specifier prefix; `%%` is the literal.
        assert!(u.contains("Environment=\"PROMPT=100%%\"\n"), "got: {u}");
        assert!(u.contains("Environment=\"QUOTED=a\\\"b\"\n"), "got: {u}");
        assert!(u.contains("Environment=\"WIN=C:\\\\adi\"\n"), "got: {u}");
        // A raw newline would have started a new directive.
        assert!(u.contains("Environment=\"MULTI=one\\ntwo\"\n"), "got: {u}");
    }

    #[test]
    fn program_arguments_stay_single_words() {
        let u = unit::service_unit(
            "l",
            &argv(&["/opt/adi tools/adi-dns", "--config=/tmp/a b.toml"]),
            "/tmp/log",
            &[],
        );
        assert_well_formed(&u);
        assert!(
            u.contains("ExecStart=\"/opt/adi tools/adi-dns\" \"--config=/tmp/a b.toml\"\n"),
            "got: {u}"
        );
    }

    #[test]
    fn a_newline_in_the_label_cannot_inject_a_directive() {
        let u = unit::service_unit(
            "evil\nExecStartPre=/bin/rm -rf /",
            &argv(&["/bin/x"]),
            "/tmp/log",
            &[],
        );
        assert_well_formed(&u);
        assert!(!u.contains("\nExecStartPre="), "got: {u}");
    }

    #[test]
    fn a_trailing_backslash_in_the_label_cannot_swallow_the_next_directive() {
        // An unescaped `evil\` would make systemd read the *following* line as part of the
        // description, silently dropping whatever that line configured.
        let u = unit::service_unit("evil\\", &argv(&["/bin/x"]), "/tmp/log", &[]);
        assert_well_formed(&u);
        assert!(u.contains("Description=ADI service evil\\\\\n"), "got: {u}");
        assert!(u.contains("\nAfter=network-online.target"), "got: {u}");
    }

    #[test]
    fn a_percent_in_free_text_is_not_a_specifier() {
        let u = unit::service_unit("100%done", &argv(&["/bin/x"]), "/tmp/log", &[]);
        assert!(
            u.contains("Description=ADI service 100%%done\n"),
            "got: {u}"
        );
    }
}
