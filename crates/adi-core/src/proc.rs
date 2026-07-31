//! Thin subprocess helpers — the one place that spawns external programs. Keeps
//! `launchd`/`dns` free of `std::process` plumbing, mirroring Swift's `Launchd.run`.

use std::ffi::OsStr;
use std::process::Command;

/// Exit status + combined stdout/stderr of a finished subprocess.
#[derive(Debug)]
pub struct Output {
    /// The process exit code, or `-1` if it could not be launched.
    pub status: i32,
    /// stdout followed by stderr, lossily decoded as UTF-8.
    pub text: String,
}

impl Output {
    #[must_use]
    pub fn ok(&self) -> bool {
        self.status == 0
    }
}

/// Run `argv` (program + args) to completion; `argv[0]` must be absolute or on `PATH`.
pub fn run<S: AsRef<OsStr>>(argv: &[S]) -> Output {
    run_with_env(&[], argv)
}

/// [`run`], with `env` layered **over** the inherited environment rather than replacing it.
///
/// The systemd back-end is what needs this: `systemctl --user` reaches the per-user manager over
/// the bus under `XDG_RUNTIME_DIR`, which a non-login context (a cron job, `ssh node adi up`) does
/// not set — so the supervisor supplies it instead of failing on an environment detail.
pub fn run_with_env<S: AsRef<OsStr>>(env: &[(&str, String)], argv: &[S]) -> Output {
    let Some((program, rest)) = argv.split_first() else {
        return Output {
            status: -1,
            text: "empty argv".to_string(),
        };
    };
    let mut command = Command::new(program);
    command.args(rest);
    for (key, value) in env {
        command.env(key, value);
    }
    match command.output() {
        Ok(out) => {
            let mut text = String::from_utf8_lossy(&out.stdout).into_owned();
            text.push_str(&String::from_utf8_lossy(&out.stderr));
            Output {
                status: out.status.code().unwrap_or(-1),
                text,
            }
        }
        Err(e) => Output {
            status: -1,
            text: format!(
                "failed to launch {}: {e}",
                program.as_ref().to_string_lossy()
            ),
        },
    }
}

/// Run a privileged command behind a single OS elevation prompt.
///
/// - **macOS:** `code` is a `/bin/sh` command line, run as root via `osascript … with
///   administrator privileges` (one Authorization prompt).
/// - **Windows:** `code` is a **PowerShell** script, staged to a temp `.ps1` and launched
///   elevated via `Start-Process -Verb RunAs` (one UAC prompt); the elevated exit code is
///   propagated back.
///
/// There is no Linux arm, on purpose. It is not that elevation is impossible there — it is that
/// there is no equivalent of *this* function: `osascript` and `RunAs` both raise a prompt the
/// desktop session owns, and Linux's counterparts (`pkexec`, `sudo` with an askpass) need an agent
/// a headless node does not have. `adi-core`'s Linux privileged steps therefore go through
/// `sudo -n` — which never prompts — and print what to run when that is refused; see
/// [`crate::dns`]. Gating this per `target_os` rather than `unix` is what keeps a Linux build from
/// compiling a call to an `osascript` that is not there.
#[cfg(target_os = "macos")]
pub fn run_admin(code: &str) -> Output {
    // Escape for AppleScript string literal: backslash first, then double-quote.
    let escaped = code.replace('\\', "\\\\").replace('"', "\\\"");
    let script = format!("do shell script \"{escaped}\" with administrator privileges");
    run(&["/usr/bin/osascript", "-e", &script])
}

#[cfg(windows)]
pub fn run_admin(code: &str) -> Output {
    let mut path = std::env::temp_dir();
    path.push(format!("adi-admin-{}.ps1", std::process::id()));
    if std::fs::write(&path, code).is_err() {
        return Output {
            status: -1,
            text: "failed to stage elevation script".to_string(),
        };
    }
    let file = path.to_string_lossy().replace('\'', "''");
    // Launch the staged script elevated, wait for it, and surface its exit code. `-PassThru`
    // yields the process so we can read `.ExitCode`; without RunAs there is no way to elevate
    // a child from an unprivileged parent.
    let launcher = format!(
        "$p = Start-Process -FilePath powershell.exe \
         -ArgumentList '-NoProfile','-ExecutionPolicy','Bypass','-File','{file}' \
         -Verb RunAs -Wait -PassThru -WindowStyle Hidden; exit $p.ExitCode"
    );
    let out = run(&[
        "powershell",
        "-NoProfile",
        "-NonInteractive",
        "-Command",
        &launcher,
    ]);
    let _ = std::fs::remove_file(&path);
    out
}
