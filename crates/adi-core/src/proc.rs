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

/// Run `argv` (program + args) to completion. `argv[0]` must be an absolute path or
/// resolvable on `PATH`. Accepts both `&str` and `String` elements.
pub fn run<S: AsRef<OsStr>>(argv: &[S]) -> Output {
    let Some((program, rest)) = argv.split_first() else {
        return Output {
            status: -1,
            text: "empty argv".to_string(),
        };
    };
    match Command::new(program).args(rest).output() {
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

/// Run a shell command as root behind a single macOS Authorization prompt, via
/// `osascript … with administrator privileges`. The GUI auth dialog is presented by
/// the OS to the session, so it appears even though we run headless with piped I/O.
pub fn run_admin(shell: &str) -> Output {
    // Escape for AppleScript string literal: backslash first, then double-quote.
    let escaped = shell.replace('\\', "\\\\").replace('"', "\\\"");
    let script = format!("do shell script \"{escaped}\" with administrator privileges");
    run(&["/usr/bin/osascript", "-e", &script])
}
