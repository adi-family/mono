//! Thin subprocess helper — the engine drives the macOS toolchain (`curl`, `hdiutil`,
//! `codesign`, `shasum`, `plutil`) through this, mirroring `adi-core`'s `proc`.

use std::ffi::OsStr;
use std::process::Command;

/// Exit status + combined stdout/stderr of a finished subprocess.
#[derive(Debug)]
pub(crate) struct Output {
    /// The process exit code, or `-1` if it could not be launched.
    pub status: i32,
    /// stdout followed by stderr, lossily decoded as UTF-8.
    pub text: String,
}

impl Output {
    pub(crate) fn ok(&self) -> bool {
        self.status == 0
    }
}

/// Like [`run`] but with stdout kept separate — for commands whose stdout is parsed
/// (a fetched manifest, a checksum) and must not be polluted by stderr noise.
#[derive(Debug)]
pub(crate) struct Captured {
    pub status: i32,
    pub stdout: Vec<u8>,
    pub stderr: String,
}

impl Captured {
    pub(crate) fn ok(&self) -> bool {
        self.status == 0
    }
}

pub(crate) fn capture<S: AsRef<OsStr>>(argv: &[S]) -> Captured {
    let Some((program, rest)) = argv.split_first() else {
        return Captured {
            status: -1,
            stdout: Vec::new(),
            stderr: "empty argv".to_string(),
        };
    };
    match Command::new(program).args(rest).output() {
        Ok(out) => Captured {
            status: out.status.code().unwrap_or(-1),
            stdout: out.stdout,
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        },
        Err(e) => Captured {
            status: -1,
            stdout: Vec::new(),
            stderr: format!(
                "failed to launch {}: {e}",
                program.as_ref().to_string_lossy()
            ),
        },
    }
}

/// Run `argv` (program + args) to completion; `argv[0]` must be absolute or on `PATH`.
pub(crate) fn run<S: AsRef<OsStr>>(argv: &[S]) -> Output {
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
