//! One archive that answers "why is it not working on this machine".
//!
//! The person hitting a failure here is the one least able to say what broke: it could be the
//! `.adi` route, the front door's port, a launchd job that never started, a bundle Gatekeeper is
//! running from a randomised path, or an update that rolled back. Two hand-written shell scripts
//! used to be airdropped to them one at a time, each guessing at one of those; this collects all
//! of it at once, from inside the install, and leaves a single file to send.
//!
//! Two rules govern what goes in.
//!
//! **Everything is read and nothing is changed.** No service is started, stopped or reconfigured,
//! so a report is safe to take from a machine mid-incident — including one whose DNS is down,
//! which is the case this exists for and the case in which the control panel cannot be reached to
//! ask for anything.
//!
//! **No secret lands in the archive.** The store's `secrets/`, its database and its agent
//! transcripts are listed but never opened, the front door's TLS keys are never copied, and every
//! line written goes through [`redact`] first — so a config key whose name says it carries a
//! credential ships as `«redacted»` rather than as one.

use std::fmt;
use std::fmt::Write as _;
use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use serde::Serialize;

use adi_config::Flavor;

use crate::commands::{Adi, Report};
use crate::install;
use crate::launchd;
use crate::paths;
use crate::proc;

/// How much of each log the report carries, taken from the end.
///
/// A tail rather than the file: `adi-dashboards.log` on a working machine reaches a gigabyte,
/// and a collector that read it whole would hang the app it is supposed to be reporting on.
const LOG_TAIL_BYTES: u64 = 256 * 1024;

/// How many crash reports to carry, newest first, and how much of each. The first frames are
/// what identify a crash; the rest is thread state nobody reads from an archive.
const CRASH_REPORTS_KEPT: usize = 5;
const CRASH_REPORT_BYTES: u64 = 128 * 1024;

/// The window of `log show` the macOS section carries — long enough to hold the launch that
/// went wrong, short enough that the command returns while someone is watching a spinner.
const UNIFIED_LOG_WINDOW: &str = "30m";

/// Seconds a probe (a DNS query, an HTTP request) gets before it is reported as a timeout.
/// Every probe here is against loopback, so anything slower than this *is* the finding.
const PROBE_TIMEOUT: &str = "4";

/// Substrings that mark a key as carrying a credential, matched case-insensitively against the
/// left of the first `=` or `:` on a line.
///
/// Spelled out rather than shortened to `auth`: `codesign` reports the signing chain as
/// `Authority=Developer ID Application: …`, and a prefix match would redact the one line that
/// says whether the bundle is genuine.
const SENSITIVE_KEYS: [&str; 11] = [
    "secret",
    "token",
    "password",
    "passwd",
    "apikey",
    "api_key",
    "api-key",
    "authorization",
    "auth_header",
    "credential",
    "private_key",
];

/// What a redacted value is replaced with. Visible on purpose: a blank would read as a config
/// key that was never set, which is a different diagnosis.
const REDACTED: &str = "«redacted»";

/// The archive extension for this OS — see [`archive`].
const ARCHIVE_EXT: &str = if cfg!(target_os = "macos") {
    "zip"
} else {
    "tar.gz"
};

/// Why a report could not be produced. Both arms are terminal for the collection: a section that
/// merely fails to read something records the failure in its own text and the report still ships.
#[derive(Debug)]
pub enum Error {
    /// The staging directory, or a file in it, could not be written.
    Write(String),
    /// The archiver refused. The text is what it said.
    Archive(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Write(why) => write!(f, "could not write the report: {why}"),
            Self::Archive(why) => write!(f, "could not archive the report: {why}"),
        }
    }
}

impl std::error::Error for Error {}

/// A finished report: the file to send, and enough about it to be described without opening it.
#[derive(Debug, Clone, Serialize)]
pub struct Bundle {
    /// The archive itself — the one thing to hand over.
    pub path: PathBuf,
    pub bytes: u64,
    /// Every file inside, in the order they were collected.
    pub files: Vec<String>,
    /// What the collector could already see was wrong. The archive holds the evidence; this is
    /// the reading of it, so a report is also a self-check somebody can act on without sending
    /// anything at all.
    pub findings: Vec<String>,
}

/// The diagnostics command surface (`adi.diagnose().collect(…)`) — a zero-sized facade like
/// [`crate::Dns`].
#[derive(Debug, Default, Clone, Copy)]
pub struct Diagnose;

#[allow(clippy::unused_self)]
impl Diagnose {
    #[must_use]
    pub fn new() -> Self {
        Self
    }

    /// Collect everything and archive it.
    ///
    /// `out` is a destination file, or a directory to write the default filename into; `None`
    /// puts it in the store's `reports/` directory, which needs no macOS file-access consent —
    /// `~/Desktop` would raise a TCC prompt, and a prompt is one more thing to fail on a machine
    /// somebody is already reporting as broken.
    ///
    /// # Errors
    /// [`Error::Write`] if the staging directory cannot be written, [`Error::Archive`] if the
    /// OS archiver refuses.
    pub fn collect(self, out: Option<&Path>) -> Result<Bundle, Error> {
        let stamp = Stamp::now();
        let report = Adi::new().report();
        let (network, network_findings) = network_report();

        let mut findings = setup_findings(&report);
        findings.extend(service_findings(&report));
        findings.extend(network_findings);
        findings.extend(update_findings());

        let mut parts = vec![
            Part::new("README.txt", readme()),
            Part::new("summary.txt", summary(&stamp, &report, &findings)),
            Part::new("status.json", status_json(&report)),
            Part::new("install.txt", install_report()),
            Part::new("services.txt", services_report()),
            Part::new("network.txt", network),
            Part::new("update.txt", update_report()),
            Part::new("store.txt", store_report()),
            Part::new("environment.txt", environment_report()),
        ];
        parts.extend(log_parts());
        parts.extend(crash_parts());
        parts.extend(unified_log_part());

        let name = format!("adi-report-{}-{}", crate::VERSION, stamp.file);
        let staging = std::env::temp_dir().join(&name);
        let _ = fs::remove_dir_all(&staging);
        write_parts(&staging, &parts)?;

        let path = destination(out, &name);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|e| Error::Write(e.to_string()))?;
        }
        // A stale archive of the same name would be *appended* to by some archivers and silently
        // kept by others, so the file that gets sent has to be the one just built.
        let _ = fs::remove_file(&path);
        archive(&staging, &path)?;
        let _ = fs::remove_dir_all(&staging);

        Ok(Bundle {
            bytes: fs::metadata(&path).map(|m| m.len()).unwrap_or_default(),
            path,
            files: parts.iter().map(|p| p.name.clone()).collect(),
            findings,
        })
    }
}

/// One file in the archive.
struct Part {
    /// Its path inside the archive, `/`-separated.
    name: String,
    body: String,
}

impl Part {
    fn new(name: impl Into<String>, body: String) -> Self {
        Self {
            name: name.into(),
            body,
        }
    }
}

fn write_parts(staging: &Path, parts: &[Part]) -> Result<(), Error> {
    fs::create_dir_all(staging).map_err(|e| Error::Write(e.to_string()))?;
    for part in parts {
        let path = staging.join(&part.name);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|e| Error::Write(e.to_string()))?;
        }
        fs::write(&path, redact(&part.body)).map_err(|e| Error::Write(e.to_string()))?;
    }
    Ok(())
}

fn destination(out: Option<&Path>, name: &str) -> PathBuf {
    let file = format!("{name}.{ARCHIVE_EXT}");
    match out {
        Some(path) if path.is_dir() => path.join(file),
        Some(path) => path.to_path_buf(),
        None => reports_dir().join(file),
    }
}

/// `~/.adi/mono/reports`, falling back to the temp directory. A store that cannot be written to
/// is itself a plausible reason someone is running this, so it must not be the thing that stops
/// them.
fn reports_dir() -> PathBuf {
    let module = adi_config::Config::open().module("reports");
    module
        .ensure_dir()
        .map_or_else(|_| std::env::temp_dir(), Path::to_path_buf)
}

/// Pack `staging` into `out`, keeping the directory name so the archive expands into a folder
/// rather than scattering a dozen files into whatever the reader unpacked it in.
///
/// `ditto` on macOS and `tar` elsewhere: both ship with the OS, which is what every other
/// external step in this workspace relies on rather than linking an archiver.
fn archive(staging: &Path, out: &Path) -> Result<(), Error> {
    let (staging, out) = (staging.to_string_lossy(), out.to_string_lossy());
    let result = if cfg!(target_os = "macos") {
        // `--norsrc --noextattr` rather than the usual `--sequesterRsrc`: every file here is
        // plain text this process just wrote, and sequestering produces a `__MACOSX` shadow of
        // the whole tree that doubles the file count in whatever the reader unpacks it with.
        proc::run(&[
            "ditto",
            "-c",
            "-k",
            "--norsrc",
            "--noextattr",
            "--keepParent",
            &staging,
            &out,
        ])
    } else {
        let parent = Path::new(staging.as_ref())
            .parent()
            .map_or_else(|| ".".to_string(), |p| p.to_string_lossy().into_owned());
        let name = Path::new(staging.as_ref())
            .file_name()
            .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
        proc::run(&["tar", "-czf", &out, "-C", &parent, &name])
    };
    if result.ok() {
        Ok(())
    } else {
        Err(Error::Archive(result.text.trim().to_string()))
    }
}

// MARK: redaction

/// Blank out the value of any line whose key names a credential.
///
/// Line- and key-based rather than pattern-based on purpose: a rule that hunted for
/// "things that look like a token" would both miss a short one and redact a build hash,
/// and a report full of `«redacted»` where the useful values were is no more sendable than one
/// with a secret in it.
#[must_use]
pub fn redact(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for line in text.lines() {
        match split_key(line) {
            Some((key, separator, value)) if is_sensitive(key) && !value.trim().is_empty() => {
                out.push_str(key);
                out.push(separator);
                out.push(' ');
                out.push_str(REDACTED);
            }
            _ => out.push_str(line),
        }
        out.push('\n');
    }
    out
}

/// The key, its separator, and the rest — at the first `=` or `:`, whichever comes first.
fn split_key(line: &str) -> Option<(&str, char, &str)> {
    let (index, separator) = line.char_indices().find(|&(_, c)| c == '=' || c == ':')?;
    Some((&line[..index], separator, &line[index + 1..]))
}

fn is_sensitive(key: &str) -> bool {
    let key = key.to_ascii_lowercase();
    SENSITIVE_KEYS.iter().any(|marker| key.contains(marker))
}

// MARK: the sections

fn readme() -> String {
    let bin = crate::BIN_NAME;
    format!(
        "What this is\n\
         ============\n\n\
         A diagnostic report from an ADI install, made by `{bin} diagnose` (or the app's\n\
         \"Create a report\" button). It was collected by reading state only — no service was\n\
         started, stopped or reconfigured to produce it.\n\n\
         Read summary.txt first: it carries the versions, the setup gates, every service's\n\
         state, and the collector's own reading of what looks wrong. Everything else is the\n\
         evidence behind it.\n\n\
         What is deliberately NOT in here\n\
         --------------------------------\n\
         Secrets, the shared database, agent transcripts and the front door's TLS private keys\n\
         are never copied — the store listing in store.txt names those directories and stops\n\
         there. Every line that IS included has had the value of any credential-looking key\n\
         replaced with {REDACTED}, so a config file that names a token ships without one.\n\n\
         That redaction is a filter, not a proof. Glance over the files before sending this to\n\
         somebody, the same as you would any log.\n"
    )
}

fn summary(stamp: &Stamp, report: &Report, findings: &[String]) -> String {
    let flavour = Flavor::current();
    let location = install::current();
    let mut out = String::new();

    out.push_str("ADI diagnostic report\n=====================\n\n");
    let _ = writeln!(out, "collected     {} UTC ({})", stamp.human, stamp.unix);
    let _ = writeln!(out, "version       {}", crate::VERSION);
    let _ = writeln!(
        out,
        "flavour       {} (.{}, ~/{}, {}*)",
        flavour.id, flavour.domain, flavour.dir_name, flavour.label_prefix
    );
    let _ = writeln!(out, "host          {}", host_line());
    let _ = writeln!(
        out,
        "running from  {} ({})",
        location.path().display(),
        if location.is_durable() {
            "durable"
        } else {
            "NOT durable — services must not be installed from here"
        }
    );
    let _ = writeln!(out, "store         {}", paths::support_dir().display());
    let _ = writeln!(out, "logs          {}", paths::logs_dir().display());

    let setup = &report.setup;
    out.push_str("\nSetup\n-----\n");
    for (label, granted) in [
        ("app in a durable place".to_string(), setup.location_durable),
        (
            format!(".{} route installed", flavour.domain),
            setup.dns_route,
        ),
        ("front door installed".to_string(), setup.front_door),
        // Reported beside the gate above rather than folded into it: "installed" is a file and
        // "answering" is a socket, and the whole point of printing both is the line where they
        // disagree. Kept inside the label column the other three rows share — the address it
        // knocked on is in the finding below and in network.txt, and is not worth breaking the
        // one block a reader scans first.
        (
            "front door answering".to_string(),
            setup.front_door_answering,
        ),
    ] {
        let _ = writeln!(out, "  {label:<24}{}", gate(granted));
    }

    out.push_str("\nServices\n--------\n");
    if report.services.is_empty() {
        out.push_str("  (none registered — this build has no services at all)\n");
    }
    for service in &report.services {
        let _ = writeln!(
            out,
            "  {:<12} {:<9} {:<9} {}",
            service.name,
            if service.enabled {
                "enabled"
            } else {
                "disabled"
            },
            if service.running {
                "running"
            } else {
                "stopped"
            },
            service.detail
        );
    }

    out.push_str("\nWhat looks wrong\n----------------\n");
    if findings.is_empty() {
        out.push_str("  Nothing — every check the collector makes passed.\n");
        out.push_str("  If something is still broken, the logs are where to look next.\n");
    } else {
        for finding in findings {
            let _ = writeln!(out, "  - {finding}");
        }
    }
    out
}

fn status_json(report: &Report) -> String {
    serde_json::to_string_pretty(report)
        .unwrap_or_else(|e| format!("{{\"error\":\"status could not be serialized: {e}\"}}"))
}

/// One line naming the OS and the machine, in whatever way this OS answers it.
fn host_line() -> String {
    if cfg!(target_os = "macos") {
        let name = one_line(&["sw_vers", "-productName"]);
        let version = one_line(&["sw_vers", "-productVersion"]);
        let build = one_line(&["sw_vers", "-buildVersion"]);
        let arch = one_line(&["uname", "-m"]);
        format!("{name} {version} ({build}) · {arch}")
    } else {
        one_line(&["uname", "-srm"])
    }
}

fn install_report() -> String {
    let mut out = heading("where this build is running from");
    let location = install::current();
    let _ = writeln!(out, "executable: {}", location.path().display());
    let _ = writeln!(out, "durable:    {}", gate(location.is_durable()));
    if let Some(why) = location.explain() {
        let _ = writeln!(out, "\n{why}");
    }

    if cfg!(target_os = "macos") {
        let bundle = macos_bundle();
        out.push_str(&heading("the app bundle"));
        match &bundle {
            Some(path) => {
                let _ = writeln!(out, "bundle: {}\n", path.display());
                let path = path.to_string_lossy().into_owned();
                out.push_str(&block(&[
                    "plutil",
                    "-p",
                    &format!("{path}/Contents/Info.plist"),
                ]));
                out.push_str(&heading("quarantine and other extended attributes"));
                out.push_str(&block(&["xattr", "-l", &path]));
                out.push_str(&heading("code signature"));
                out.push_str(&block(&["codesign", "-dvv", &path]));
                out.push_str(&block(&[
                    "codesign",
                    "--verify",
                    "--strict",
                    "--verbose=2",
                    &path,
                ]));
                out.push_str(&heading("Gatekeeper assessment"));
                out.push_str(&block(&["spctl", "-a", "-vvv", "-t", "exec", &path]));
                out.push_str(&heading("architectures in the executables"));
                for name in ["adi-mono", "adi-dns", "adi-hive", "adi-app"] {
                    out.push_str(&block(&[
                        "lipo",
                        "-info",
                        &format!("{path}/Contents/Resources/{name}"),
                    ]));
                }
            }
            None => out.push_str(
                "Not running from an .app bundle — this is a development build or a node \
                 install, so there is no signature or quarantine state to report.\n",
            ),
        }
    }
    out
}

/// The `.app` this executable is inside, if it is inside one. `…/ADI.app/Contents/Resources/adi-mono`
/// is three components below the bundle.
fn macos_bundle() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let bundle = exe.ancestors().nth(3)?;
    if bundle
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("app"))
    {
        Some(bundle.to_path_buf())
    } else {
        None
    }
}

fn services_report() -> String {
    let mut out = heading("service definitions on disk");
    let dir = paths::launch_agents_dir();
    let _ = writeln!(out, "{}", dir.display());
    match fs::read_dir(&dir) {
        Ok(entries) => {
            let prefix = Flavor::current().label_prefix.clone();
            let mut found = 0;
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().into_owned();
                if !name.starts_with(&prefix) {
                    continue;
                }
                found += 1;
                let size = entry.metadata().map(|m| m.len()).unwrap_or_default();
                let _ = writeln!(out, "  {:>10}  {name}", adi_config::human_bytes(size));
            }
            if found == 0 {
                out.push_str(
                    "  (no service definitions at all — nothing has ever been enabled here)\n",
                );
            }
        }
        Err(e) => {
            let _ = writeln!(out, "  cannot be read: {e}");
        }
    }

    for service in Adi::new().services() {
        out.push_str(&heading(&format!(
            "{} — {}",
            service.name(),
            service.label()
        )));
        let _ = writeln!(out, "program:     {}", service.program().join(" "));
        let _ = writeln!(out, "status file: {}", service.status_path().display());
        let _ = writeln!(out, "log:         {}", service.log_path().display());
        out.push_str(&supervisor_state(&service.label()));
    }
    out
}

/// What the OS supervisor says about one job — the state, the last exit code and the program it
/// would run, which between them separate "never installed" from "installed and crashing".
///
/// The job is addressed through [`crate::launchd`] rather than by a name built here: launchd
/// wants `gui/$UID/<label>` and systemd wants a sanitized `.service` file name, and a second
/// spelling of either would report on nothing while looking like it had.
fn supervisor_state(label: &str) -> String {
    #[cfg(target_os = "macos")]
    {
        block(&["launchctl", "print", &launchd::target(label)])
    }
    #[cfg(target_os = "linux")]
    {
        let unit = launchd::unit_path(label);
        let name = unit
            .file_name()
            .map_or_else(|| label.to_string(), |n| n.to_string_lossy().into_owned());
        block(&["systemctl", "--user", "status", "--no-pager", &name])
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        block(&["schtasks", "/Query", "/TN", label, "/V", "/FO", "LIST"])
    }
}

fn network_report() -> (String, Vec<String>) {
    let flavour = Flavor::current();
    let mut findings = Vec::new();
    let mut out = heading("the OS route for this install's names");

    let resolver = if cfg!(target_os = "macos") {
        format!("/etc/resolver/{}", flavour.domain)
    } else {
        format!("/etc/systemd/resolved.conf.d/adi-{}.conf", flavour.domain)
    };
    let _ = writeln!(out, "{resolver}");
    match fs::read_to_string(&resolver) {
        Ok(text) => out.push_str(&text),
        Err(e) => {
            let _ = writeln!(out, "  cannot be read: {e}");
        }
    }

    if cfg!(target_os = "macos") {
        out.push_str(&heading("what the resolver stack has loaded"));
        out.push_str(&block(&["scutil", "--dns"]));
    }

    out.push_str(&heading("does the resolver answer"));
    let port = flavour.resolver_port.to_string();
    let host = format!("app.{}", flavour.domain);
    let dig = block(&[
        "dig",
        "+time=2",
        "+tries=1",
        "@127.0.0.1",
        "-p",
        &port,
        &host,
        "A",
    ]);
    if !dig.contains("NOERROR") {
        findings.push(format!(
            "the resolver did not answer {host} on 127.0.0.1:{port} — see network.txt"
        ));
    }
    out.push_str(&dig);

    out.push_str(&heading("does the name resolve the way an app would ask"));
    out.push_str(&block(&["dscacheutil", "-q", "host", "-a", "name", &host]));

    out.push_str(&heading("what is listening"));
    out.push_str(
        "`lsof` runs unprivileged here, so it cannot see the front door: that is a root process, \
         and its socket on :80 does not belong to this user. An empty block below is therefore \
         not evidence that nothing is bound — the socket list after it is, and the HTTP probes \
         after that are the only evidence that any of it works.\n",
    );
    for port in [
        "80".to_string(),
        "443".to_string(),
        flavour.resolver_port.to_string(),
        crate::app::port().to_string(),
    ] {
        out.push_str(&block(&[
            "lsof",
            "-nP",
            &format!("-iTCP:{port}"),
            "-sTCP:LISTEN",
        ]));
    }
    out.push_str(&listening_sockets());

    out.push_str(&heading("does anything answer over HTTP"));
    let panel = format!("http://127.0.0.1:{}/api/health", crate::app::port());
    let front = format!("http://{host}/");
    for url in [&panel, &front] {
        let probe = block(&[
            "curl",
            "-sS",
            "-m",
            PROBE_TIMEOUT,
            "-o",
            "/dev/null",
            "-w",
            "%{http_code}",
            url,
        ]);
        if !probe.contains("200") {
            findings.push(format!("{url} did not answer 200 — see network.txt"));
        }
        out.push_str(&probe);
    }
    (out, findings)
}

/// Every listening TCP socket, and nothing else.
///
/// Filtered in here rather than through a pipe, and not only because [`proc`] does not run a
/// shell: the unfiltered output also lists every established connection, which names the hosts
/// this machine is talking to — not something to put in a file somebody is about to send to
/// somebody else.
fn listening_sockets() -> String {
    let argv: &[&str] = if cfg!(target_os = "macos") {
        &["netstat", "-an", "-p", "tcp"]
    } else {
        &["ss", "-lntp"]
    };
    let out = proc::run(argv);
    let listening: Vec<&str> = out
        .text
        .lines()
        .filter(|line| line.contains("LISTEN"))
        .collect();
    let body = if listening.is_empty() {
        format!(
            "(nothing listening, or the tool is absent — exit {})",
            out.status
        )
    } else {
        listening.join("\n")
    };
    format!("\n$ {} | grep LISTEN\n{body}\n", argv.join(" "))
}

fn update_report() -> String {
    let update = Adi::new().update();
    let mut out = heading("what the updater last did");
    out.push_str(
        &serde_json::to_string_pretty(&update.state())
            .unwrap_or_else(|e| format!("state could not be serialized: {e}")),
    );
    out.push('\n');

    out.push_str(&heading("what the release channel says right now"));
    match update.check() {
        Ok(check) => {
            let _ = writeln!(out, "installed:        {}", check.installed);
            let _ = writeln!(out, "latest:           {}", check.latest);
            let _ = writeln!(out, "update available: {}", yes_no(check.update_available));
            let _ = writeln!(out, "platform:         {}", check.platform);
            let _ = writeln!(out, "has artifact:     {}", yes_no(check.has_artifact));
        }
        // Not a failure of the report: a machine with no route out is exactly the kind that
        // gets one of these, and "could not reach the channel" is itself the answer.
        Err(e) => {
            let _ = writeln!(out, "could not be checked: {e}");
        }
    }
    out
}

fn update_findings() -> Vec<String> {
    let state = Adi::new().update().state();
    let mut findings = Vec::new();
    if let Some(error) = &state.last_error {
        findings.push(format!("the last update check failed: {error}"));
    }
    if state.last_outcome.as_deref() == Some("rolled-back") {
        findings.push(
            "an update was installed and rolled back — this machine is on the previous version"
                .to_string(),
        );
    }
    findings
}

fn store_report() -> String {
    let store = paths::support_dir();
    let mut out = heading("the store, one line per top-level entry");
    let _ = writeln!(out, "{}\n", store.display());
    match fs::read_dir(&store) {
        Ok(entries) => {
            let mut rows: Vec<String> = entries
                .flatten()
                .map(|entry| {
                    let name = entry.file_name().to_string_lossy().into_owned();
                    let meta = entry.metadata();
                    if meta.as_ref().is_ok_and(std::fs::Metadata::is_dir) {
                        let count = fs::read_dir(entry.path())
                            .map(|d| d.flatten().count())
                            .unwrap_or_default();
                        format!("  {count:>10} entries  {name}/")
                    } else {
                        let size = meta.map(|m| m.len()).unwrap_or_default();
                        format!("  {:>18}  {name}", adi_config::human_bytes(size))
                    }
                })
                .collect();
            rows.sort();
            out.push_str(&rows.join("\n"));
            out.push('\n');
        }
        Err(e) => {
            let _ = writeln!(out, "  cannot be read: {e}");
        }
    }

    out.push_str(&heading("the configuration files, verbatim"));
    out.push_str(
        "Only these. The store also holds secrets, the database, agent transcripts and the \n\
         front door's TLS keys, and none of those are ever opened by this collector.\n",
    );
    for relative in [
        "dns/adi-dns.toml",
        "dns/hive-frontdoor.yaml",
        "dns/frontdoor.toml",
        "dns/resolver.json",
        "hive/hive.yaml",
        "hive/status.json",
        "ports/registry.json",
        "update/config.toml",
    ] {
        out.push_str(&heading(relative));
        match fs::read_to_string(store.join(relative)) {
            Ok(text) => out.push_str(&text),
            Err(e) => {
                let _ = writeln!(out, "(not readable: {e})");
            }
        }
    }
    out
}

fn environment_report() -> String {
    let mut out = heading("the environment this collection ran in");
    out.push_str(
        "A service launchd starts at login gets none of this, which is itself worth knowing: an \n\
         ADI_* variable set only in a shell explains a CLI and an app that disagree about which \n\
         install they are talking to.\n\n",
    );
    let mut rows: Vec<String> = std::env::vars()
        .filter(|(key, _)| {
            key.starts_with("ADI_") || matches!(key.as_str(), "HOME" | "PATH" | "SHELL" | "LANG")
        })
        .map(|(key, value)| format!("{key}={value}"))
        .collect();
    rows.sort();
    out.push_str(&rows.join("\n"));
    out.push('\n');

    out.push_str(&heading("the resolved flavour"));
    out.push_str(
        &serde_json::to_string_pretty(Flavor::current())
            .unwrap_or_else(|e| format!("could not be serialized: {e}")),
    );
    out.push('\n');
    out
}

/// The tail of every ADI log on this machine, one file each.
///
/// The directory is read rather than the services asked, so a log belonging to something not in
/// the service registry — the root front door, a leftover from an older version — is collected
/// too. Those are the ones nobody thinks to ask for.
fn log_parts() -> Vec<Part> {
    let mut paths: Vec<PathBuf> = fs::read_dir(paths::logs_dir())
        .map(|entries| {
            entries
                .flatten()
                .map(|e| e.path())
                .filter(|p| {
                    p.extension().is_some_and(|e| e.eq_ignore_ascii_case("log"))
                        && p.file_name()
                            .and_then(|n| n.to_str())
                            .is_some_and(|n| n.starts_with("adi"))
                })
                .collect()
        })
        .unwrap_or_default();
    paths.sort();
    // The front door runs as root and logs outside the user's directory, so it is named rather
    // than found — and it is the log that explains a `.adi` name that resolves but never loads.
    if cfg!(target_os = "macos") {
        paths.push(PathBuf::from("/Library/Logs/adi-hive-frontdoor.log"));
    }

    paths
        .iter()
        .map(|path| {
            let name = path
                .file_name()
                .map_or_else(|| "log".to_string(), |n| n.to_string_lossy().into_owned());
            Part::new(format!("logs/{name}"), tail(path, LOG_TAIL_BYTES))
        })
        .collect()
}

fn crash_parts() -> Vec<Part> {
    let dir = paths::logs_dir().join("DiagnosticReports");
    let Ok(entries) = fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut crashes: Vec<(std::time::SystemTime, PathBuf)> = entries
        .flatten()
        .filter(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .to_ascii_lowercase()
                .starts_with("adi")
        })
        .filter_map(|entry| {
            let modified = entry.metadata().ok()?.modified().ok()?;
            Some((modified, entry.path()))
        })
        .collect();
    crashes.sort_by(|a, b| b.0.cmp(&a.0));
    crashes.truncate(CRASH_REPORTS_KEPT);

    crashes
        .iter()
        .map(|(_, path)| {
            let name = path
                .file_name()
                .map_or_else(|| "crash".to_string(), |n| n.to_string_lossy().into_owned());
            Part::new(
                format!("crash-reports/{name}"),
                tail(path, CRASH_REPORT_BYTES),
            )
        })
        .collect()
}

/// The unified log for every ADI process, which is the only place a launch that dies before it
/// can open its own log file leaves a trace.
fn unified_log_part() -> Option<Part> {
    if !cfg!(target_os = "macos") {
        return None;
    }
    let names = [
        "ADI", "ADI Dev", "adi-mono", "adi-dns", "adi-hive", "adi-app",
    ];
    let predicate = names
        .iter()
        .map(|n| format!("process == \"{n}\""))
        .collect::<Vec<_>>()
        .join(" OR ");
    Some(Part::new(
        "unified-log.txt",
        block(&[
            "log",
            "show",
            "--last",
            UNIFIED_LOG_WINDOW,
            "--predicate",
            &predicate,
            "--style",
            "compact",
        ]),
    ))
}

// MARK: findings — the collector's own reading of what it gathered

fn setup_findings(report: &Report) -> Vec<String> {
    let flavour = Flavor::current();
    let mut findings = Vec::new();
    if !report.setup.location_durable {
        findings.push(
            "the app is running from a disk image or a translocated copy, so nothing can be \
             installed — move it to Applications and open it from there"
                .to_string(),
        );
    }
    if !report.setup.dns_route {
        findings.push(format!(
            "the .{} route is not installed, so none of this install's names resolve",
            flavour.domain
        ));
    }
    if !report.setup.front_door {
        findings.push(format!(
            "the front door is not installed, so nothing answers .{} even once it resolves",
            flavour.domain
        ));
    }
    if report.setup.front_door && !report.setup.front_door_answering {
        findings.push(format!(
            "the front door is installed but nothing is answering on {}:80 — its plist is on \
             disk and launchd is not running it, so every .{} name resolves and then hangs \
             (a browser shows it as loading forever, never as refused). Reopening the app \
             retries this once; `adi-mono dns grant-network` is the same repair on demand",
            flavour.frontdoor_addr, flavour.domain
        ));
    }
    findings
}

fn service_findings(report: &Report) -> Vec<String> {
    let mut findings = Vec::new();
    if !report.services.is_empty() && !report.any_running {
        findings.push("no ADI service is running at all".to_string());
    }
    for service in &report.services {
        if service.enabled && !service.running {
            findings.push(format!(
                "{} is enabled but not running — its log is the place to look",
                service.name
            ));
        }
    }
    findings
}

// MARK: small helpers

fn heading(title: &str) -> String {
    format!("\n=== {title} ===\n")
}

fn yes_no(value: bool) -> &'static str {
    if value { "yes" } else { "no" }
}

/// A gate: something that has to be true for the install to work at all.
///
/// The shouted `NO` is the point. Summary.txt is read at a glance by somebody scanning for the
/// line that explains the whole ticket, and a lower-case `no` in a column of `yes`es does not
/// catch an eye that is skimming.
fn gate(value: bool) -> &'static str {
    if value { "yes" } else { "NO" }
}

/// Run `argv` and render it as a block: the command line, then what it said.
///
/// A command this OS does not have reports that rather than producing nothing, so a gap in the
/// report is never read as a clean result.
fn block(argv: &[&str]) -> String {
    let out = proc::run(argv);
    let body = out.text.trim_end();
    let body = if body.is_empty() {
        format!("(no output, exit {})", out.status)
    } else {
        body.to_string()
    };
    format!("\n$ {}\n{body}\n", argv.join(" "))
}

fn one_line(argv: &[&str]) -> String {
    let out = proc::run(argv);
    out.text.lines().next().unwrap_or("?").trim().to_string()
}

/// The last `bytes` of a file, starting at the first whole line.
///
/// Seeking rather than reading: the dashboards log reaches a gigabyte on a machine that has been
/// up for a while, and this runs on the main path of a button somebody just pressed.
fn tail(path: &Path, bytes: u64) -> String {
    let mut file = match fs::File::open(path) {
        Ok(file) => file,
        Err(e) => return format!("{}\n\n(not readable: {e})\n", path.display()),
    };
    let size = file.metadata().map(|m| m.len()).unwrap_or_default();
    let from = size.saturating_sub(bytes);
    if from > 0 && file.seek(SeekFrom::Start(from)).is_err() {
        return format!("{}\n\n(could not be seeked)\n", path.display());
    }
    let mut buffer = Vec::new();
    if let Err(e) = file.read_to_end(&mut buffer) {
        return format!("{}\n\n(not readable: {e})\n", path.display());
    }
    let text = String::from_utf8_lossy(&buffer);
    // A seek lands mid-line; that first fragment is noise, and dropping it is also what keeps a
    // split UTF-8 sequence from opening the file with a replacement character.
    let text = if from > 0 {
        text.split_once('\n').map_or("", |(_, rest)| rest)
    } else {
        text.as_ref()
    };
    format!(
        "{} — last {} of {}\n\n{text}",
        path.display(),
        adi_config::human_bytes(size.min(bytes)),
        adi_config::human_bytes(size)
    )
}

/// The moment of collection, in the two forms the report needs it: one for a filename and one
/// for a person.
struct Stamp {
    unix: u64,
    /// `YYYY-MM-DD HH:MM:SS`, UTC.
    human: String,
    /// `YYYYMMDD-HHMMSS` — sorts, and survives every filesystem.
    file: String,
}

impl Stamp {
    fn now() -> Self {
        let unix = adi_config::now_unix();
        let (year, month, day, hour, minute, second) = civil(unix);
        Self {
            unix,
            human: format!("{year:04}-{month:02}-{day:02} {hour:02}:{minute:02}:{second:02}"),
            file: format!("{year:04}{month:02}{day:02}-{hour:02}{minute:02}{second:02}"),
        }
    }
}

/// Split a Unix timestamp into UTC calendar fields — Howard Hinnant's `civil_from_days`, in
/// integer arithmetic, because this workspace carries no date crate and gaining one to stamp a
/// filename would be a poor trade.
fn civil(secs: u64) -> (i64, i64, i64, u64, u64, u64) {
    let days = i64::try_from(secs / 86_400).unwrap_or(0);
    let rest = secs % 86_400;
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    (year, month, day, rest / 3600, (rest / 60) % 60, rest % 60)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_credential_is_replaced_and_its_key_is_kept() {
        let text = "manifest_url = https://example.com\nauth_header = Bearer abc123\n";
        let out = redact(text);
        assert!(out.contains("https://example.com"), "{out}");
        assert!(!out.contains("abc123"), "{out}");
        assert!(out.contains("auth_header"), "{out}");
    }

    /// The signing chain is the one line in the report that says whether a bundle is genuine,
    /// and `Authority=` starts with the four letters a shorter marker list would have matched.
    #[test]
    fn the_code_signature_survives_redaction() {
        let line = "Authority=Developer ID Application: ADI (752556J5V6)";
        assert_eq!(redact(line).trim(), line);
    }

    #[test]
    fn an_empty_value_is_left_alone() {
        assert_eq!(redact("token =").trim(), "token =");
    }

    #[test]
    fn redaction_is_case_insensitive_and_covers_yaml_and_toml() {
        for line in [
            "API_KEY = live-1234",
            "password: hunter2",
            "  Private_Key: -----BEGIN",
        ] {
            let out = redact(line);
            assert!(out.contains(REDACTED), "{line} was not redacted: {out}");
        }
    }

    #[test]
    fn a_log_line_with_a_timestamp_is_not_mistaken_for_a_key() {
        let line = "2026-09-02T10:00:00Z INFO dns: answered app.adi";
        assert_eq!(redact(line).trim(), line);
    }

    #[test]
    fn the_stamp_is_a_real_utc_date() {
        // 2026-09-02 12:34:56 UTC.
        let (year, month, day, hour, minute, second) = civil(1_788_352_496);
        assert_eq!((year, month, day), (2026, 9, 2));
        assert_eq!((hour, minute, second), (12, 34, 56));
    }

    #[test]
    fn the_filename_stamp_sorts_and_has_no_separators_a_filesystem_dislikes() {
        let stamp = Stamp::now();
        assert_eq!(stamp.file.len(), 15);
        assert!(
            stamp.file.chars().all(|c| c.is_ascii_digit() || c == '-'),
            "{}",
            stamp.file
        );
    }

    #[test]
    fn an_explicit_file_destination_is_used_as_given() {
        let path = Path::new("/tmp/somewhere/report.zip");
        assert_eq!(destination(Some(path), "adi-report-1.0.0-x"), path);
    }

    #[test]
    fn an_explicit_directory_gets_the_default_filename() {
        let out = destination(Some(&std::env::temp_dir()), "adi-report-1.0.0-x");
        assert_eq!(
            out.file_name().and_then(|n| n.to_str()),
            Some(format!("adi-report-1.0.0-x.{ARCHIVE_EXT}").as_str())
        );
    }

    #[test]
    fn the_tail_of_a_short_file_is_the_whole_file() {
        let path = std::env::temp_dir().join("adi-diagnose-tail-test.log");
        fs::write(&path, "one\ntwo\nthree\n").expect("write the fixture");
        let out = tail(&path, LOG_TAIL_BYTES);
        assert!(out.contains("one"), "{out}");
        assert!(out.contains("three"), "{out}");
        let _ = fs::remove_file(&path);
    }

    /// A tail that started mid-line would open with a fragment, which reads as a corrupt log.
    #[test]
    fn a_truncated_tail_starts_at_a_line_boundary() {
        let path = std::env::temp_dir().join("adi-diagnose-tail-cut.log");
        fs::write(&path, "aaaaaaaaaa\nbbbbbbbbbb\ncccccccccc\n").expect("write the fixture");
        let out = tail(&path, 16);
        assert!(!out.contains("aaaaaaaaaa"), "{out}");
        assert!(out.contains("cccccccccc"), "{out}");
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn a_missing_file_is_reported_rather_than_dropped() {
        let out = tail(Path::new("/nonexistent/adi.log"), LOG_TAIL_BYTES);
        assert!(out.contains("not readable"), "{out}");
    }

    /// Every finding has to name something the reader can act on, or it is noise in the one
    /// section support reads first.
    #[test]
    fn setup_findings_name_the_gate_that_is_missing() {
        let report = Report {
            any_running: false,
            services: Vec::new(),
            setup: crate::SetupReport {
                location_durable: false,
                dns_route: false,
                front_door: false,
                front_door_answering: false,
                ready: false,
            },
        };
        let findings = setup_findings(&report);
        assert_eq!(findings.len(), 3);
        assert!(findings[0].contains("Applications"), "{findings:?}");
    }

    /// The half state this whole check exists for: every gate open, and the front door silent.
    /// It must read as a fault rather than as a healthy machine, because everything else about
    /// it — resolver, control panel, both plists — looks perfect.
    #[test]
    fn an_installed_front_door_that_answers_nothing_is_a_finding() {
        let report = Report {
            any_running: true,
            services: Vec::new(),
            setup: crate::SetupReport {
                location_durable: true,
                dns_route: true,
                front_door: true,
                front_door_answering: false,
                ready: true,
            },
        };
        let findings = setup_findings(&report);
        assert_eq!(findings.len(), 1, "{findings:?}");
        assert!(findings[0].contains("nothing is answering"), "{findings:?}");
        // The reader has to leave with the command, not just the diagnosis.
        assert!(findings[0].contains("grant-network"), "{findings:?}");
    }

    /// The same three gates open and a front door that answers is the ordinary machine, and it
    /// must stay silent — a findings section that fires on a healthy install is read by nobody.
    #[test]
    fn a_front_door_that_answers_is_not_a_finding() {
        let report = Report {
            any_running: true,
            services: Vec::new(),
            setup: crate::SetupReport {
                location_durable: true,
                dns_route: true,
                front_door: true,
                front_door_answering: true,
                ready: true,
            },
        };
        assert!(setup_findings(&report).is_empty());
    }
}
